# Track design committed: session report

Subject: the video game's tank driving physics (the belt's support and its contact with loose
pieces); game state only. The owner's order of 2026-11-08 turned the experiment into the one
shipped path. Numbers are MEASURED on the scorecard unless labelled DERIVED.

| step | commit subject | files | + | − |
|---|---|---|---|---|
| 1 resolution | fix the belt's contact resolution at 48 stations by 3 lateral points | 12 | 51 | 500 |
| 2 the card's seams | the scorecard reads the one track design directly | 4 | 107 | 334 |
| 3 piece law | remove the custom piece reaction law | 11 | 184 | 1548 |
| 4 dead paths | audit only; two wording edits went with step 5 | 0 | 0 | 0 |
| 5 one support law | a belt segment's stiffness follows the shoe face a piece covers | 5 | 185 | 45 |

**The boundary (step 3), one rule from construction facts: the layer a collider was built on.** On
`Layer::Debris` (a construction's fallen pieces and debris stress fragments, at any mass): a solver
body that meets the belt's segments. The sampled support keeps it in the footprint partition, so a
covered cell takes no ground support and the Driver perceives it, and applies no spring force.
Every other solid (standing construction, map blocks, another tank's hull; terrain is the field):
the sampled support carries the belt and the solid takes the whole opposite reaction. A heavy
dynamic chunk lands on the Debris side, because every fallen group's contact collider is built on
that layer (`breakage::piece_layers`). The only dynamic body left on the other side is a hull
(DERIVED stability on a 50 t hull at 64 Hz: `dt² k / m + 2 dt c / m` about 0.2, limit 4).

**The law (step 5).** The spring channel's law `k'` per metre of belt holds over the shoe's width
`W`, so it is `k'/W` per square metre of face. A segment-piece pair takes `k'/W × A`, damping
likewise, with `A` the piece's silhouette on the segment's outer face, clipped to the face. Full
coverage gives `k' × length`, the old law and the sampled support's; a pair covering none has no
contact. A segment's pairs are capped at its whole law, so stacked silhouettes cannot multiply the
support. How it holds: derived from the contact's geometry, nothing tuned; a tilted piece's
silhouette overstates its area until it embeds, and the force still grows from zero with
penetration. The driven rise over 0.1 m pieces at 0.728 coverage is **0.079 m**, against 0.119
before and the share law's 0.073 (DERIVED).

## Before and after

Steps 1 to 3 claim no behaviour change. Card against the 48 × 3 baseline: **0 differences** on
geometry, slope, turning, acceleration, climbing, walls, trenches, curb and ledge, ride, rubble,
heap and work. Only the dropped columns differ (`belt_channel_share_of_that`,
`belt_loaded_pieces_asleep_max`, line 8c's `belt_channel_share_of_piece_support`).

Step 5 moves only pieces, on purpose. Every other line stays at 0 differences. Observations, before
to after:

| line | hull rise (m) | sink mean (m) | parked piece speed max (m/s) |
|---|---|---|---|
| layer under the parked tank | 0.483 to 0.475 | 0.021 to 0.041 | 0.007 to 0.007 |
| pile ahead, 0.6 throttle | 0.891 to 0.877 | 0.147 to 0.157 | 0.016 to 0.054 |
| pile ahead, 1 throttle | 0.833 to 0.798 | 0.122 to 0.115 | **0.047 to 1.068** |

| loose heap | rise max (m) | nose up (deg) | pieces moved |
|---|---|---|---|
| 0.6 throttle | 0.185 to 0.104 | 5.93 to 3.42 | 17 to 23 |
| 1 throttle | 0.223 to 0.113 | 6.60 to 4.50 | 21 to 38 |

The pile at 1 throttle ends with one piece at 1.07 m/s in the parked reading; not examined.

## Left for the next session

- **Step 4's one finding, not done.** `BeltContact.stiffness` and `damping`, with `support_stiffness`,
  have no shipped reader since the custom piece law went; only tests read them (`forces.rs`,
  `sim.rs`, `headless_test.rs`). The fields go, and the tests must read the damping bound from
  `support_load`'s inputs instead. Nothing else in `world_contact/` or `forces.rs` is dead: every
  terrain and overlay path has a shipped caller, and no inner-face stop exists.
- **A missing test.** The first step-5 run inverted the shoe bounds on the left track (mirrored
  `grip_columns`), emptied that side's piece contacts, and passed every test; the card showed it.
  Fixed with min/max; a both-sides test belongs with the lens pass.
- The lens pass over what this session edited, including `Surface` becoming `pub(super)`.
- Backlog row `load-29` names `src/tank/spawn.rs`, which step 1 edited: verify and close it. No other
  row names a file this session edited.
- `net::ram::tests::tracked_ram_sweep_opens_within_two_ticks_and_accounts_for_momentum` fails
  ("the solver resolved standing matter no cut judged"). It fails at 97 stations too, so it
  predates this session. Probably the outer-face stop is outside the sweep's cast set (DERIVED, not
  confirmed).
- Two fixture tolerances moved with 48 stations (ripple, not law): `contact_law_shipped_ground_speeds`
  spare under `weight * 0.0005` (108 N), the rolling-fade crawl within `ROLLING_FADE_SPEED * 5e-3`
  (0.26 %).
