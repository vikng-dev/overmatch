# Fuze modes and the 8.8 cm Sprgr. L/4.5 (#181): lane notes

Subject: the game's damage model (round potency, fuzes, fragment cones, bursts), game state only.
Source keys refer to `.agents/scratch/rounds-research/kwk36-rounds-2026-10-30.md`.

## What landed

- **One fuze block in the round data** (`src/rounds/fuze.rs`): `position` (`Nose`/`Base`, data
  only), `trigger` (`SpendExit(arming_spend)` or `Impact(sensitivity_n)`), named `modes` with
  `delay_s`, `default_mode`. Refused at load: no modes, more than 256, a repeated, blank or
  undrawable name, a default naming no mode, a negative or non-finite number, a burst without a
  fuze or a fuze without a burst.
- **`Impact`** functions when the struck body's resisting force at contact reaches the
  sensitivity: the force law's dE/dx (J/m = N), the disc mean of the entrance walk's first
  encounter (`resolve::entry_force`). The ground (HeightGrid) stops a round dead, so its force is
  DERIVED unbounded and it functions every impact fuze. The delay is flight time from contact: a
  change of speed rescales the metres left, no exit or bounce restarts it.
- **The Pzgr. 39 ring** keeps its exact behaviour: a one-mode `SpendExit` fuze with the old values
  and provenance. `the_spend_exit_fuze_keeps_todays_bursts_and_doses` pins burst position, speed,
  wait and doses on three slabs; the pins were MEASURED on the pre-change code (0ac9c0b) with the
  same fixture and match bit for bit.
- **The Sprgr. L/4.5** (`assets/rounds/88mm_HE.round.ron`): nose A.Z. 23/28, `Impact` 30 N,
  `instant` 0 s (default), `delay` 0.1 s; the burst is today's ring and fragment cone fed by
  poured amatol 60/40.
- **Crew-picked mode**: V toggles the selected round type's mode when its fuze lists more than one
  (`TankCommand::toggle_fuze`; `WeaponLoad::fuze_modes`, one per type, empty until the first
  toggle). The HUD brackets a pickable mode: `8.8 cm Sprgr. L/4.5 [delay]`. The authority reads
  the mode at fire time and holds a shot whose mode the fuze does not list; `FireEvent.fuze_mode`
  carries it and the receiver binds its own round in it. PROTOCOL_REV 39.

## Data decisions

- Mass 9.30 kg: CITED TM53 p.445 (20.5 lb), chosen over Brit43's 20.3 lb (TM53 names the Kw.K. 36).
- Filler 0.9072 kg amatol 60/40: CITED TM53 p.445 (2 lb), chosen over Brit43's 1.9 lb.
- Muzzle speed 820 m/s: CITED Brit43, E9-369A, TME30 (2,690 ft/s = 819.9 m/s).
- Delay 0.1 s: CITED HEA12 p.8 (stamped v.(0,1)), chosen over E9-369A's 0.11 s.
- Sensitivity 30 N: CITED HEA12 p.8 graze spring, 2.8–3.0 kgf, as the owner took it in the
  interview (§ Fuze modes). Any solid body exceeds it by orders of magnitude.
- ASSUMED: casing steel (GunSteel), cavity diameter 0.065 m (DERIVED cavity 0.171 m), base parts
  0.3 kg, amatol density 1,600 kg/m³, amatol speed constant 2,400 m/s, the HE ogive nose (the
  APCBC's crh 4) and its crew line rate (the Pzgr. 39's 30).
- Tiger load-out, most carried first: 61 APCBC, 23 HE, 8 AP 40 of 92 (CITED Brit43 pp.91–92).
  APCR moved last: the range sandbox's test reads its last round as one with a penetrator.

## Gaps recorded, not fixed

- **#103**: a burst outside any tank doses unowned volumes only, never a tank. An HE round bursting
  on a tree or the ground beside a tank does nothing to the tank.
- **Ground ricochet**: the A.Z. 23/28 in delay on a flat impact against firm ground ricochets and
  bursts in the air (Fibel p.71). The ground here is a dead stop; the delay burst waits at rest.
- **Nose piece**: the ring law has every burst in flight send a nose piece on (DERIVED 4.41 kg for
  the Sprgr. from the ASSUMED casing). For a high-explosive shell that is a property of today's
  law, not a source.
- **Position** is data no law reads yet.
- The sandbox range fires every round in its default mode; it has no toggle.
- The mode can change after loading (it is read at fire time); whether it may is open.

## Evidence

Tests (all in the lib suite, 1395 passed):

- `ballistics::fuze_tests::an_impact_fuze_functions_at_contact_on_wood_and_on_armour`
- `ballistics::fuze_tests::the_ground_functions_an_impact_fuze_in_either_mode`
- `ballistics::fuze_tests::an_impact_delay_is_flight_time_from_contact`
- `ballistics::fuze_tests::the_spend_exit_fuze_keeps_todays_bursts_and_doses`
- `rounds::fuze::tests::a_fuze_block_resolves_or_is_refused_at_load`
- `shooting::tests::the_fuze_toggle_picks_the_mode_a_shot_fires_with`
- `net::shot_transport::tests::a_fire_event_carries_the_fired_fuze_mode`
- `crew_ui::tests::the_readout_names_a_pickable_fuze_mode`
- `guns::tests::every_round_reads_through_its_gun_bit_for_bit_as_pinned` (the Sprgr. pinned)

Stills: hidden, muted runs of the offline root (`OVERMATCH_MUTE=1 SPIKE_SIM_WINDOWED=1`,
`OVERMATCH_CAPTURE_SHOT=88mm_HE,<mode>,<tree|ground>@8`, each under the one build lock) at the
standing tree nearest the map's centre, the burst drawn over the scene (flight path cyan, every
fourth field direction orange, burst point white). The pen cam opens only for tanks, so it shows
none of these bursts. The PNGs live outside the repo, in the session scratchpad
(`lanes/181-stills/`). Bursts as logged (MEASURED):

| Still | Burst | Speed | Waited |
|---|---|---|---|
| `sprgr-instant-tree.png` | at the trunk's face, 0.10 m before its centre | in flight | 0 |
| `sprgr-delay-tree.png` | 78.9 m past the trunk's face, in the air | in flight | 0 |
| `sprgr-instant-ground.png` | on the ground | at rest | 0 |
| `sprgr-delay-ground.png` | on the ground | at rest | 0.1 s |

In each, the HUD reads `next: 8.8 cm Sprgr. L/4.5 [instant]` or `[delay]`.
