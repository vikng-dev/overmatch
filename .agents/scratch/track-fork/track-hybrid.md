# track-hybrid: the lean boat on the ground, solver contacts for pieces

Subject: the video game's tank driving physics (the track belt, the suspension, their contact with
terrain, static obstacles and rubble); game state only. Lane `track-hybrid`, option D, from
`lane/track-lean` ("Merge branch 'lane/track-score' into lane/track-lean"). One session,
2026-11-08. Default resolution 97 × 3. No timing runs.

## First screen

**Card** (MEASURED, complete card: [97x3.md](track-hybrid-cards/97x3.md), JSON beside it). Lines 2
to 7 are byte-identical to lean 97 × 3 (`bay-1/.agents/scratch/track-fork/track-lean-session3-cards/97x3.md`):
terrain and fixed construction are the lean arm unchanged. Line 1 passes on all ten rows (the
lean card predates the force-balance reader, so only the format differs). Line 8:

| line 8 | lean 97 × 3 | solver, session 4 | hybrid |
|---|---|---|---|
| reaction ratio: pile 0.6 / pile 1.0 / layer | 0.582 / 0.562 / 0.012 | 0.9999 / 0.9999 / 0.970 | **0.960 / none / 0.969** |
| law check | not claimed | PASS | **FAIL**: pile 1.0 has no awake tick to read |
| support from pieces, share of weight | 1.00 / 1.00 / 1.00 | 0.33 / 0.48 / 0.13 | 0.098 / 0.000 / 0.125 |
| reached pile centre, s (0.6 / 1.0) | 8.23 / 5.80 | not reached / 6.80 | 7.92 / 5.66 |
| layer: hull rise (layer top 0.502 m), sink mean | 0.487 m, 23 mm | 0.536 m, 0 | 0.473 m, 28 mm |
| asleep | never (spring forces wake it) | the island sleeps | **the island sleeps** |

- **Line 4b, the walls; line 6, the curb; line 7, the ride:** identical to lean 97 × 3 (one-lane
  walls on the stop, friction vertical +54.5 / +54.7 kN, rise 40.6 mm; curb roll 0.298°; washboard
  7.36 m/s, pitch RMS 0.163°; kick passes). Segments meet only pieces, so no static obstacle sees
  them, and the solver arm's per-collider stiffness split cannot reach the curb.
- **Line 8 fails on the card's reader, not on the reaction law.** Fully on pieces, every cell's
  first surface is a piece, so the spring channel applies no force, nothing wakes the tank, and the
  tank and the pieces under it fall asleep together within about half a second: Avian's island
  sleep, the owner's "rubble is static when holding the tank up" through the engine's mechanism.
  A sleeping island builds no constraint, so the card's reader sees no support while it sleeps:
  12.5 % of the weight over 4 s is about 0.5 s awake (DERIVED), as the solver arm found on its
  layer. At full throttle the pile's tank was asleep for the whole parked window, so no ratio was
  read and the law check fails. The card's `belt_loaded_pieces_asleep_max` reads the arm's
  out-of-solver forces, which carry no piece here, so it reads 0 and is blind (the solver arm's
  finding). While awake the reaction is 0.96 to 0.97.

**Work** (MEASURED, `arm::segment_work_counts`, per tank per tick):

| case | segment colliders in the broad phase | broad-phase pairs | narrow-phase pairs | solver points | most manifolds with moving bodies in one step |
|---|---:|---:|---:|---:|---:|
| open ground, full throttle, s 5–10 | 36 | 0 | 0 | 0 | 0 |
| driving onto the pile, full throttle | 36 | 24.0 | 14.5 | 50.3 | 69 |
| parked on the pile (asleep) | 36 | 90 | 49 | 168 | 0 (asleep) |
| parked on the layer | 36 | 54 | 44 | 166 | 44 |

- 36 segments per tank (18 a side): only segments over suspension travel exist. 30 tanks put 1080
  segment proxies in the broad phase on open ground, filtered to zero pairs.
- The spring channel's open-ground work is lean's: 203.25 belt contacts, 406.5 force
  applications. On the pile it emits no contact (0 belt contacts against lean's 656 to 869) but
  keeps the exact convex piece path for shadowing: 1287 / 1280 / 546 slab checks, as lean.
- **Colours.** Avian 0.7 colours each manifold between two moving bodies into one of
  `DYNAMIC_COLOR_COUNT` = 20 colours that hold the tank once (`constraint_graph.rs`, 24 colours, 20
  dynamic, overflow last); the rest go to the single-threaded overflow colour. On the pile the
  tank reaches 69 such manifolds in one step and 44 on the layer (segments, stop, side plates and
  hull proxy together): at most 20 are coloured, the remainder solved serially. Debris-only
  segments do exceed the limit on a pile, but only there; on open ground they add nothing.

**Decisions.**
1. **Pieces shadow the ground** (changed from the brief's "the spring channel no longer sees
   pieces at all"). MEASURED first: with pieces out of the spring channel, the tank rode 0.137 m up
   on 0.1 m pieces, because the spring channel also pushed on the ground beneath them, two laws at
   one place (DERIVED for full coverage: rise `(δ₀ + h)/2` = 0.136 m). Now the spring channel keeps
   pieces in its footprint partition, so the first surface in each cell decides, and a cell whose
   first surface is a piece emits no force (`forces::collect_column_contacts`): the segments carry
   it. The Driver still perceives that support, so the gear choice reads the pile as ground.
2. **The hard limit is the existing outer-face stop** for pieces as for terrain. The vendored
   patch's own `travel` limit is deleted: one copy of the full-compression face. End wheels get no
   segment: the stop is hard there from first touch, as on terrain.
3. **One law per segment across pieces.** `share_segment_laws` (after the narrow phase) shares a
   segment's `k'L` over every point it touches across all its pairs, so two pieces under one
   segment do not double its stiffness.
4. **0.3 m segments kept** (`SEGMENT_M`, Resolution, below the 0.5 m piece size floor).
5. **The panic is not inherited.** The arm's only reader is the lean stop reader
   (`stop_contacts::by_owner`), which reads only the `ContactTransfers` snapshot; nothing looks a
   pair up in the live contact graph. The solver lane's fix ("the contact record carries the
   solver's friction") is not taken: this arm reads no friction coefficient.

## What the arm is

- **Terrain and fixed construction:** the lean arm unchanged.
- **Loose pieces** (a collider whose layers meet `Layer::TrackBelt`: construction pieces through
  `breakage::piece_layers`, the stress lever's fragments, the sandbox's rubble yard):
  - **Segments** (`src/track/belt_segment.rs`): the solver arm's slabs, cut from the same travel
    knots as the spring channel, outer face on the droop envelope, on `Layer::TrackBelt` with
    filter `Debris` only, hung on the hull with the stop at spawn (ADR-0014).
  - **Support:** the vendored compliant contact (`upstream/avian-compliant-contact.md`) with the
    spring channel's calibrated per-metre `(k', c')` (`TrackGear::segment_law`; the sandbox
    publishes its own calibration, `EnvelopeLaw::segment_law`) times the segment length.
  - **Normals:** the solver arm's cut-face rule (bisector of square faces).
  - **Drivetrain:** each segment carries the stop's `BeltFace`.

## Must 1: one support law

Over the ground and over a piece the belt has the same law. The per-metre stiffness and damping
are the spring channel's own calibration, read from one place. The envelope (outer face on the
droop envelope) comes from the same travel knots. The hard limit is the same stop collider, and at
the end wheels both have no spring band and the stop is hard from first touch. Shadowing gives
each cell one law. The law per metre does not grow with the number of pieces.

What still differs, none tuned:
- **Lateral coverage.** A segment gives a piece its whole width's stiffness; the spring channel
  gives a piece its covered share. MEASURED on `a_driving_tank_rides_onto_loose_pieces_through_its_belt_support`
  (pieces 0.728 of the shoe width, 0.1 m high): rise 0.119 m, against the share law's 0.073 m
  and 0.137 m before shadowing. DERIVED for this case: the segment's full width on the piece plus
  the ground strip beside it gives `(0.272 δ₀ + h)/1.272` ≈ 0.115 m at the rest deflection δ₀ ≈ 0.17 m.
  The fix would scale each manifold by its covered width, which the hook cannot see from its
  points for an edge or vertex contact. Not built.
- **No engagement ramp** in the patch (the spring channel ramps over the tank's
  `suspension.engage` depth).
- **Friction** on pieces is Avian's Coulomb at the pair's μ, not the boat's shear element, and no
  rolling resistance acts on a piece.
- **The layer sits 14 mm lower than lean's** (0.473 against 0.487 m), not examined.

## Must 2: one drivetrain reader

Each segment carries the stop's `BeltFace` (`bump_stop::spawn_track_colliders`), so the shared
hook (`stop_contacts::WorldContacts`) moves its surface at belt speed, and `stop_contacts::by_owner`
and `reactions` read its friction impulse into the transmission with the stop's, one tick late.
No second reader exists.

## Code size (counted lines, `git diff --numstat` against `lane/track-lean`)

| file | added | removed |
|---|---:|---:|
| `src/track/belt_segment.rs` (new; 186 non-comment lines) | 256 | 0 |
| vendored Avian patch, six files | 86 | 4 |
| `src/track/bump_stop.rs` | 24 | 1 |
| `src/track/forces.rs` (shadow skip, and a strip helper to stay under the line lint) | 22 | 11 |
| `src/track/rubble.rs` | 21 | 1 |
| `src/track/stop_contacts.rs` | 17 | 1 |
| `src/track/sim.rs` | 13 | 6 |
| `src/track_sandbox/belt.rs`, `mod.rs` | 16 | 2 |
| `src/lib.rs`, `src/track.rs`, `src/construction/breakage.rs`, `src/net/debris_stress.rs` | 12 | 3 |
| tests: `src/headless_test.rs`, `src/headless_test/track_scorecard/arm.rs` | 185 | 34 |

Game code: 467 lines added, 29 removed (the patch included; its upstream note is 35 more). **The piece path in `src/track/rubble.rs` is not
removed.** The exact convex footprint path stays, because it shadows the ground. The recipient
law (`piece_impulses`, its clamp, `explicit_recipients`, `contact_impulse`, `apply_pushes`, about
340 lines with the explicit branch, and their tests) no longer meets a loose piece. Its explicit
branch still serves another tank's hull under a belt. Whether anything still reaches the clamp
branch was not checked; if nothing does, it and its tests go when this arm is chosen.

**The sandbox's rubble yard** (`assets/dev/track_sandbox.ron`): its pieces are made with
`breakage::piece_layers`, so they meet the segments. The sandbox spawns its rig through
`spawn_track_colliders` and publishes its own segment law at each recalibration. The
`track_sandbox` tests pass. It was not driven (no windows).

## Left

1. **The card's reader on a sleeping island:** decide how a parked tank asleep on pieces counts
   (the score lane's reader), or whether a parked tank should stay awake.
2. **Lateral coverage** of a segment's law.
3. **Colour overflow on piles:** 69 moving-body manifolds against 20 colours.
4. **Delete the clamp branch** in `rubble.rs` if nothing else reaches it.
