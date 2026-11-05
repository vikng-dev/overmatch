# Core mechanisms: getting a full-speed ram inside 15.625 ms on 2 vCPU (Claude session)

Subject: the game's environment destruction model, game state only. I worked read-only: nothing was edited, built or run.

**Decisions I took.**
- Hosted figures use 1-vCPU prices as the bound for 2 vCPU. A second vCPU did not lower the cost per piece (MEASURED 18 vs 20 µs, `perf-2026-10-31/README.md`).
- Mac single-thread prices convert to hosted at ×3.7 (DERIVED in that file).
- The menu is ranked by the saving on the ram's worst tick.

**Two corrections to the brief's inputs**
- `owner-play-2026-11-03/ram/stats.txt` comes from the hosted run, not the Mac. Its `server.log` reports an AMD EPYC 7713 with `available_parallelism=1` and a quota of 1.00 CPU.
  - 32.0 ms is the median of the worst 5-second window (72 pieces awake). The worst single tick was 85.8 ms.
  - The server ran at 31–57 ticks a second for about 15 minutes (t=105–1050 s). For the last 13 of those minutes about 50 pieces stayed awake with no client or tank connected. All MEASURED.
- That run predates the no-live-joint early return in `fracture::weigh` (11-04) and the common construction runtime (11-05). Every hosted figure after the fix below is DERIVED. `lane/weigh-cost` is not merged, and its final code has not been priced.

**The five mechanisms as the code has them** (bold marks a correction to the first cut)
1. **Intake.**
   - **Each physics step's contacts become one combined deposit, not one deposit each** (`read_contacts`). Presses from sleeping bodies ride along but start nothing.
   - **Contacts carry no energy, so they never wear or crack a chunk directly.** They act only through the topple, weight and rest laws.
   - A standing house skips its weight check only when the press set is identical to last time. **A moving group never skips**: `run_laws` always counts it as loose.
   - MEASURED in a scripted ram: 116,129 law runs in 30.3 s (60 a tick), with zero state changes and zero splits.
2. **Structure check.** Your description is right, plus three facts:
   - **The weight check repeats whole passes until nothing fails, all inside one tick.**
   - **On a free group it copies every outside load onto every chunk as 7 points.** In the house collapse that came to 14.3 million loads.
   - **Its scratch memory is sized to the whole source house (1,122 chunks), even for a three-chunk piece.**
3. **Loose pieces.**
   - **There is one body per connected loose group, not one per chunk.** Each chunk has two hulls (one for contact, one for ballistics), both built at release.
   - **Each group carries a copy of the whole source house's law state.**
   - Pieces collide with terrain, vehicles, track sides and each other.
4. **Rest.** Avian's island sleep, plus four extra wakes the game adds. A sleeping piece costs about 0.4 µs a tick on the Mac (MEASURED, night-pricing item 4).
5. **Updates.**
   - **Only moved poses are sent**, so sleeping pieces send nothing. Each pose is 28 bytes, uncompressed, to every client.
   - **Sending runs outside the fixed tick.**
   - Clients already rebuild the standing house from the record.

**What players perceive.**
- From the structure check: the breach, what falls and when the roof goes, which decides cover and sight lines.
- From loose pieces: big pieces that block tracks and shots, the shape of the pile and the look of the fall.
- Not perceived: 60 re-checks a tick of pieces' inner seams that change nothing, settling below a centimetre, or where a hand-sized fragment lands.

**Today's ram, 72 pieces awake, 1 vCPU (DERIVED)**
- Before the fix, the MEASURED 32.0 ms splits into a base of 3.0, piece physics of 2.5 and our own code at 26.5.
- On the Mac, the fix cut our code to 0.30× per awake piece (105 → 31.9 µs, DERIVED in `slowdown-hunt/report.md`). Applied here, our code drops to 8.0 ms.
- On the Mac after the fix, law work takes about 1.5× the physics share of samples. That splits the 8.0 ms into about 4.7 ms of law runs and 3.3 ms of per-entity upkeep.
- That gives a median after the fix of about 13.5 ms.
- The worst tick adds a release spike. Before the fix it was MEASURED at 55–60 ms over the median (83.9 vs 22.3 ms, and 85.8 vs 27.3 ms). How that spike splits is unmeasured.
- ASSUMED from night pricing: most of the spike is the house's weight check. That check costs 4.4 ms for one change and 46 ms for a two-failure roof cascade (MEASURED 1.2 and 12.4 ms on the Mac, ×3.7).

## Menu, ranked by expected saving on the ram's worst tick

**1. A collapse in stages, on a house that stays coarse until touched** (structure check)
- **Today:**
  - One weight check finishes a whole cascade in one tick and re-walks all 1,163 chunks and 3,185 joints on every pass.
  - One pass costs 1.2 ms on the Mac for a single change (MEASURED) and about 4.1 ms per pass in the gables case (DERIVED: 12.4 ms over three passes). That is 4.4–15 ms hosted.
  - A whole house collapsing gave ticks of 17–43 s. The weigh-cost lane's exact rewrites still produced 160 s ticks, and its final code is unmeasured.
- **Proposed:**
  - (a) At most one failure stage per owner per tick. The next stage reads what the last one left, and the record carries each stage's tick.
  - (b) An element (house_1 has 55, MEASURED) stays one load node until a deposit touches it, or until a cheap check of its weakest inner section fails. Then it opens into its chunks. This is the rule its collider already follows.
  - (c) A free group's inertia comes from the total push and twist on the group, not from each outside load separately. The result is the same in exact arithmetic, but it takes one pass over the chunks instead of one per load.
- **Precedent:** Blast's per-frame iteration budget on a known graph; Red Faction's structural scan every few seconds; Battlefield removing whole parts.
- **Players would see:** a cascade of n stages takes n ticks. Ten stages take 0.16 s, against 0.49 s for a 1.2 m fall (DERIVED). Verdicts are otherwise unchanged.
- **Doctrine cost:** stages per tick is a `Resolution` number with a cost reason, and an unfinished cascade becomes a named game state. Rounding changes, but the laws run only on the server and no client derives from them. Owners still evaluate in parallel.
- **Saving (DERIVED):**
  - Law work on the release tick drops from 4.4–46 ms to one stage of about 1.1–3.8 ms. The coarse graph cuts per-pass scratch about 4× (from 4,348 nodes and joints to about 1,100, with about 6 touched elements ASSUMED).
  - The house collapse's 14.3 million copied loads drop to about 15 thousand, roughly 900× fewer.

**2. The structure check runs on events, not on contacts** (intake and structure check)
- **Today:** every contact on a group runs every law every tick: 60 runs a tick, none changing anything (MEASURED). That is about 4.7 ms of the median after the fix, and was 26.5 ms before it (DERIVED).
- **Proposed:**
  - Each owner keeps a safety margin taken from its own sections, using the same section law.
  - For a moving group, the margin is the smallest blow, press or spin that could fail its weakest seam. For a standing house, it is each section's slack from its last verdict.
  - A deposit inside the margin keeps the last verdict without running the laws. One beyond it runs the laws as today.
  - Contacts become a screen that turns real impacts into events, alongside shots and blasts.
- **Precedent:** The Finals' per-bond `Impulse > Baseline + BreakOffset` test feeding an incremental solve. None of the sources read runs the stress solve per contact (`hosting-research.md` §3).
- **Players would see:** nothing, as long as the margin is conservative. A slab still breaks on a hard landing, which matches the decision "walls still fracture on landing".
- **Doctrine cost:** verdicts do not change. But a wrong margin would skip a failure silently, the forbidden fifth outcome. So it ships with a test that runs the laws alongside the screen and refuses any skipped run that would have changed state.
- **Saving (DERIVED):** about 4.7 → 0.2 ms a tick, roughly 20× (60 screens at about 1 µs on the Mac, ASSUMED, ×3.7). It also removes the first law runs on new multi-chunk groups in the release tick.

**3. Gameplay-sized pieces on the server, smaller fragments made by clients** (loose pieces)
- **Today:** every loose group is a server body with two hulls per chunk, built at release.
  - A ram leaves 73–136 bodies (MEASURED), and a collapsed house has 962 fallen chunks.
  - Each awake body costs about 35 µs of physics and 45 µs of upkeep hosted (DERIVED).
- **Proposed:**
  - A group below one generic size class gets no server body. The class is a game `Unit`: big enough to stop a round or lift a track.
  - Its departure and launch speed go in the record. Each client flies it with gravity and the ground only, and it touches nothing that affects play.
  - Bodies above the class keep today's physics, with hulls prepared at load instead of built at every release.
- **Precedent:** Unreal Chaos's abandon level; Dagor's client debris (no collision, scripted gravity); World of Tanks' per-client detailed destruction; Siege's unreplicated small debris.
- **Players would see:** fine fragments land differently on each client and cannot be touched or hide anything. Cover and blocking come only from big pieces, in line with the provisional decision "fine presentation debris may fade".
- **Doctrine cost:** ADR-0037 gains a client-made layer with no gameplay effect, drawn at the shared delay. The size class must be one generic law, never set per building.
- **Saving (DERIVED):** linear in server bodies.
  - For the ram, ASSUMED half the groups fall below the class: physics 2.5 → 1.3 ms, upkeep 3.3 → 1.7 ms, and spawning at release halves.
  - For a whole house, physics would be about 34 ms if all 962 chunks moved as separate bodies. At an ASSUMED 40–100 gameplay bodies it is 1.4–3.5 ms.

**4. Rest that ends** (rest)
- **Today:** island sleep. In the owner's run, about 50 pieces stayed awake for 13 minutes with nobody connected (MEASURED).
- **Proposed:** fix the wake handling in the vendored Avian copy, not add a game freeze rule. Today one contact wake resets every timer in its island, and pairs that are close but not touching stay active.
- **Precedent:** Box2D takes sleeping bodies and their non-touching pairs out of the solver.
- **Players would see:** piles stop moving sooner; impacts still wake them.
- **Doctrine cost:** two more vendor changes to carry through every Avian upgrade.
- **Saving (DERIVED):** nothing on the worst tick. After #2, about 50 awake pieces cost about 4 ms a tick, falling to about 0.1 ms once they sleep, and that lasts for minutes.
- **Your doubt about rubble becoming ground, on merit:** a ram leaves about 60 sleeping pieces at about 1.5 µs each, 0.09 ms in total (DERIVED). The idea only pays at city scale, or under tanks driving on rubble (+2.6 ms per tank hosted, DERIVED from night pricing). It does not win for this goal.

**5. A lean pose stream** (updates)
- **Today:** 28 bytes per moving body per tick to every client, sent outside the tick. That costs 0.2–0.6 ms hosted with one client (MEASURED).
- **Proposed:** compressed poses that send only changes, ordered by distance for each client, and nothing at all for #3's fragments.
- **Precedent:** The Finals went from about 400 to about 175 kbit/s with compression and change-only sending; Fiedler's snapshot work; Dagor filters by camera distance.
- **Players would see:** far pieces update less often, still at the one shared delay.
- **Doctrine cost:** ADR-0037's shared delay must cover the slowest update rate.
- **Saving (DERIVED):** about 0 on the ram's tick with one client. At 30 clients, it removes about three-quarters of a 5–9 ms send (`hosting-research.md` §6 upper bound).

**Rejected on merit**
- **Scripted flight on the server between contacts:** flight is a tiny share of awake time. A 1.2 m fall takes 0.49 s, against minutes on the ground (DERIVED).
- **One body per collapsing group, broken up by clients (your other doubt):** the server already moves each connected group as one body. Never splitting it would drop the decided landing fracture and leave a rigid slab where players see rubble, so cover and blocking would disagree with what they see. Its real savings are already in #2 and #3.

**Mechanism count: yes, five become four.**
- Contact intake stops being a mechanism of its own: it becomes #2's screen, which emits impacts alongside shots and blasts.
- The re-check of moving groups in their own moving frame stops running every tick and becomes an event.
- "An element is one thing until touched" becomes a single rule for both colliders and the load graph.
- Rest stays the physics engine's.
- The laws themselves do not change; what changes is how often they run and over how much of the house.
- The levers that tune per-contact checks (how often, how sensitive, further splitting, the contact path) disappear with them. Most of the 65 levers are engine and presentation defaults nobody needs to touch: they count inventory, not moving parts.

**Minimum combination: #1 + #2 + #3.** Two changes do not leave enough margin.

Worst tick, 1 vCPU, ms (DERIVED). X is spawning on the release tick, unmeasured. ASSUMED 2–6 ms: about 70 chunks × two hull builds at about 7 µs each on the Mac, ×3.7, plus inserting the new entities.

| Term | Today, after the fix | #1 + #2 | #1 + #2 + #3 |
|---|---:|---:|---:|
| Base (1 tank, 1 client) | 3.0 | 3.0 | 3.0 |
| Piece physics, 72 awake | 2.5 | 2.5 | 1.3 |
| Law runs, every tick | 4.7 | 0.2 | 0.2 |
| Per-entity upkeep | 3.3 | 3.3 | 1.7 |
| Law work on the release tick | 4.4–46 | ≤ 3.8 | ≤ 3.8 |
| Spawning on the release tick | X | X | X/2 |
| **Worst tick** | **17.9–59.5 + X** | **12.8 + X** | **10.0 + X/2** |

- #1 + #2 fit the 15.625 ms budget only if X is under 2.8 ms. All three fit while X is under 11 ms.
- A second vCPU is upside and is not counted.
- A whole house collapsing stays outside this table and needs its own price later.

**What to measure first**
- Run the scripted ram (`slowdown-hunt/evidence/ram.ron`) on current main, on the hosted 2-vCPU allocation, with per-tick counters:
  - on the release tick: weight passes and their steps, first law runs on new groups, hull building and spawning (X), and physics;
  - across the run: a histogram of group sizes.
- In the same run, add a shadow of #2's screen that counts what it would skip while the laws still run.
- **Kill #1's stage cap** if no release tick holds a multi-pass weight check.
- **Kill #2** if law runs take under 2 ms of the steady tick, or the screen skips under 90 %, or any skipped run would have changed state.
- **#3 in full becomes required** if X exceeds 2.8 ms. Its cheap half, preparing hulls at load, is worth trying first either way.