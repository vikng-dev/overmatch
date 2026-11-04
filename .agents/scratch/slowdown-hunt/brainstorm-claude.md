# Brainstorm (Claude session): mass destruction on a two-core server

Subject: the game's environment destruction model, game state only. This is a read-only session, independent of the Codex session (`brainstorm-codex.md`); nothing was edited, built or run. Labels: MEASURED, DERIVED or ASSUMED. Mac (M4) prices convert to hosted with the repo's DERIVED 3.7× ratio. The Mac beds used all ten cores, so the hosted prices likely err low.

**Bottom line.** The per-piece rubble model can't fit "a city with mass destruction beside 30 tanks" on 2 vCPU: it needs about 13 times the free budget (DERIVED). The biggest cost is tanks driving on rubble, not the collapse peak, and the hosting research never priced that.

## Budget

- **H, the free budget.** The tick budget is 15.625 ms. 30 tanks with no destruction run at p50 14.4–16.3 ms, 102% CPU of 2 vCPU (MEASURED, #186). If #186 spreads the non-physics 62% perfectly across both cores, H ≈ 5 ms (DERIVED; the perfect split is ASSUMED).
- **Hosted unit prices** (DERIVED from the night-pricing beds):
  - one awake piece held by a tank: ~43 µs per tick;
  - one sleeping piece: ~1.6 µs per tick;
  - one tank driving over a 120-piece carpet: ~2.6 ms per tick;
  - the house weight check before M9: ~4.4 ms.
- **Scenario S (ASSUMED instance of the bar):** 5 houses release at once at ~150 pieces each, 20 houses are down, and 10 tanks are on rubble.

  | Term | Cost |
  |---|---|
  | Falling: 750 × 43 µs | ≈ 32 ms |
  | Resting: 3,000 × 1.6 µs | ≈ 4.8 ms |
  | Tanks on rubble: 10 × 2.6 ms | ≈ 26 ms |
  | **Total** | **≈ 63 ms against H ≈ 5 ms** |

  Even the quiet state (20 houses down, one tank on rubble) costs 7.4 ms.
- **Law runs on moving groups are nearly all wasted.** 111,975 contact-triggered law runs over 47 s of shelling gave 1 verdict and 1 split. 116,129 over 30 s of ramming gave none (MEASURED, slowdown-hunt counters, steady-state windows).

## Ideas, ranked by saving per unit of risk

1. **The server settles each release as one body; clients break it up (the owner's seed).**
   - **Mechanism.** A connected loose group leaves as one body with a coarse hull. The server runs no laws on it and never splits it. Clients break it up as presentation, from the construction and ledger they already hold.
   - **Saving.** Falling cost shrinks by R/P (bodies per house over pieces per house), and moving-group law runs go to zero. Five houses at once fit only if R ≤ ~23 per house (DERIVED).
   - **Costs.**
     - It reverses "one assessment path" for moving groups.
     - Server slab and client fragments disagree about blocking until rest; idea 2 closes that.
     - Client breakup is a second simulation, the line ADR-0037 draws.
     - Fine debris differs per client unless it is deterministic (the parry risk).
     - About 750 cosmetic bodies cost client CPU against the presets.
   - **Test.** A throwaway variant where `Piece` bodies skip laws and splits in the collapse, shell and ram recipes. Count R against P, and compare tick and awake counts.
   - **Precedent.** Chaos cluster replication; Dagor and World of Tanks keep detailed debris client-side.
2. **Rubble becomes ground.**
   - **Mechanism.** A landed or sleeping group leaves the physics world. Its volume, adjusted by a packing fraction, joins a heap layer in the terrain field: a 2.5-D height and material grid that slumps to its angle of repose.
     - Tracks read the heap through the terrain field they already sample.
     - Rounds pass through it at rubble bulk density.
     - A tank pushes it by a plow law (passive earth pressure).
     - Clients draw bricks on the heap surface, deterministically placed.
     - A group stays a body while its own bonds span a heap cell (a material fact, not a size rule).
   - **Saving.** Resting cost goes to ~0. Tanks on rubble fall from 26 ms to a terrain lookup plus the plow law (ASSUMED tens of µs per tank). Scenario S's steady state drops from ~31 ms to ~1 ms (DERIVED on that assumption).
   - **Costs.**
     - It reverses the decided rule that rubble rests by Avian's island sleep.
     - It holds no overhangs or small voids, and no single-brick nudging.
     - It is the first gameplay terrain change.
     - It needs three sourced laws (repose angle, porosity, passive pressure), and plow edits replicate as cell diffs.
   - **Test.** In `headless_test::loose_pieces_priced_per_tick`, swap the 120-piece carpet for the same volume as a static heap (no plow yet). Run 1, 2 and 4 tanks against the carpet at HEAD. It's real if the carpet stays ~0.7 ms on the Mac per tank, scales linearly, and the heap costs ≤ ~0.03 ms.
   - **Precedent.** Battlefield's deliberate rubble piles (sourced). From memory: Company of Heroes ruins, Spintires terrain deformation.
3. **Falling without bodies.**
   - **Mechanism.** A group touching nothing at release flies a closed-form arc, with one shape cast to find its landing. It joins the heap, or becomes a body on meeting matter or a tank. Clients work out the arc themselves, so there is no pose stream.
   - **Saving.** The ~32 ms falling term drops to about 0.
   - **Costs.** Falling pieces never collide with each other, so it needs idea 2.
   - **Test.** Count the share of awake piece-ticks with no contact. If it is small, this folds into idea 2.
   - **Precedent.** Dagor's scripted-gravity client debris.
4. **Standing constructions stay data until touched.**
   - **Mechanism.** One static compound collider per map sector; a construction gets its element entities on first touch.
   - **Saving.** Unknown: at most ~4.6 ms at 200 houses, possibly ~0 at the shipped map's 74.
   - **Test.** A tankless hosted A-B-B-A with 0, 74 and 148 houses. Drop the idea below ~5 µs per house.
5. **Clients reproduce debris motion bit for bit; the server sends causes and corrections.**
   - **Saving.** Send cost only, unmeasured; upper bound +5.2–8.7 ms.
   - **Costs.** The parry determinism risk returns, along with snaps and client CPU. It matters only if ideas 1–3 are rejected.
   - **Test.** Measure send time with real clients first.
6. **Forecast each collapse once, on the idle core, and play it back kinematically until something interferes.**
   - **Saving.** It smooths one or two collapses but falls behind at five (DERIVED).
   - **Costs.** A second Avian world (support unverified), and a hard handback to a live body.
7. **A player's machine runs the forecast, a second client checks its hash, and the server adopts the result.**
   - **Costs.** It conflicts with ADR-0024 and ADR-0037 and opens a new trust boundary.
   - **Test.** Replay a release on a client-class background core; drop it below ~5× real time.
8. **Time dilation as the declared outcome when the tick overruns.**
   - **Saving.** None. It makes the falling-behind case deliberate and fair; the collapse bench already shows unannounced 0.18× and 0.634× windows.

## Test first

1. **Idea 2's price.** It decides whether the bar is reachable at all: if rigid rubble costs ~2.6 ms hosted per tank, no collapse change saves a destroyed city.
2. **Idea 1's server half.** It is a deletion variant on existing recipes, its measured near-zero verdict yield says it changes little behaviour, and its R/P count also prices idea 3.

Run both as Mac before/after comparisons first, then once on the hosted allocation.

## Considered and dropped

- The display delay as compute slack (it reduces to "delay the failure").
- A separate debris world on the second core (one tick of contact lag recreates the one-sided exchange).
- Coarse laws on the server with clients working out the chunk detail (needs a non-physical refinement law).
- Welding sleeping islands into compounds (already in the design and the hosting research).

## Decisions taken

- Scenario S is this session's own instance of the bar.
- The seed means one body per connected loose group, never welding unconnected matter.
- H assumes #186 splits the work perfectly across both cores.
- Laws on standing constructions are left to M9. Rerun `the_weight_check_priced_per_change_and_after_a_long_wall_shot` at HEAD to see whether ~4.4 ms still holds.

Main sources: `.agents/scratch/perf-2026-11-02/night-pricing.md`, `.agents/scratch/slowdown-hunt/evidence/counters.json`, `.agents/docs/design/construction-destruction.md`.
