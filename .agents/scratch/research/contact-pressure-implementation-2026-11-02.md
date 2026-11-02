# Contact-pressure lane evidence (#147)

Subject: the game's damage model; game state only. Crew are HP pools at stations.

The lane replaces the angle gates with pressure contact and a straight, slope-charged bite.
The accepted form and preset basis remain in [the proxy decision](penetration-proxy-2026-11-02.md).

## Decisions and boundaries

- Density and slope penalty are required per-type stand-ins. No per-round construction field or separate cap term was added.
- The pressure, straight-bite and slope steps land atomically in `ballistics: replace angle gates with pressure contact (#147)`: the same entrance/transit seam, record and wire lose the old verdict together. Presets land first in `ballistics: load contact-pressure presets (#147)`; the APCR correction follows in `rounds: fly APCR on its ballistic cap (#147)`.
- The skid keeps its shipped mirror, `v sin θ` and coverage blend. Resume logic, playback cursors and the damage cam's plane solver were not changed. Camera changes are tests of the existing finite-speed/instantaneous-front contract.
- Backlog rows **ballistics-23, -25, -26, -27 and -28** close after verification. **Ballistics-24 remains**: `apply_bounce` does not supply the construction consumers' delivered-contact route. Both building and tree consumers read `ShellMarched.lines`; a generic contact-delivery design is separate from this decision/resistance replacement.

## What a player sees

- Thin plates can still turn a steep graze. For the labelled capped-steel fixture at 773 m/s and 88 mm contact calibre, a 10 mm reference plate skids past **76.6374° (MEASURED)**; the former ratio exempted it from skidding. A 45 mm plate now skids past **62.1381° (MEASURED)**, so a 70° hit skids where the former law could admit and defeat it (**DERIVED** from the former bend and potency).
- Slopes spend more potency while the bite stays straight. At 120 mm normal potency, a 30° reference plate limit changes from about 110 mm under the former bend (**DERIVED**, proxy note) to **95.4906 mm (MEASURED)** for APCBC. A 100 mm plate at that angle is consequently no longer defeated by this fixture (**DERIVED**). Head-on still costs exactly LOS for every preset.
- APCR flies on a pointed ballistic cap, retaining more speed downrange under the existing drag law; its hit nose and contacting core remain their own data. The 4-crh flight contour is **Unknown**, pending a dimensioned drawing.
- The sandbox no longer colours an overmatch verdict. The new records show different paths, doses and pool outcomes; the tables below preserve the measurements.

## Field casts: MEASURED before and after

Counts are resolved skids/bites from the real march's `PenetrationMarks`. The six fixture shots are unchanged. All six shot-output digests move; the other field-cast records stay pinned. The pin contains **2009 records (MEASURED)**, total `0xdcf0fc80e7642ab3`.

| Field shot | Before skids/bites | After skids/bites | Changed remaining pools (pool units) |
|---|---:|---:|---|
| driver | 0/2 | 0/2 | Ammo_L_0: 63835 → 53473; Ammo_L_1: 31994 → 34382; Breech: 142000 → 133475; Commander: 8744 → 14110; Engine: 271698 → 270036; Gunner: 1218 → 0; Transmission: 476245 → 476163 |
| rear | 0/2 | 0/2 | No remaining-pool change |
| left-front | 0/1 | 0/1 | No remaining-pool change |
| left-mid | 0/2 | 0/2 | No remaining-pool change |
| turret-left | 1/3 | 0/2 | Ammo_R_0: 51222 → 50791; Ammo_R_1: 50211 → 49760; Loader: 24205 → 24085 |
| right-rear | 0/1 | 0/1 | No remaining-pool change |

## Recorded scenarios: MEASURED before and after

Scenario bites below count recorded **armour-volume crossings**; HP-box crossings are not plates in a camera record. Skids count recorded reflections. These are a different scope from the field-shot all-crossing counts above. Verdicts are read from each record, never inferred from its header.

The oblique-through example was re-aimed from 55° to 30° to remain a through-shot. Its former aim produced a single skid under the new law (MEASURED in the stale-record guard). The mixed-outcome turret example moves from `z = -0.5 m` to `-0.6 m` (Fixture inputs), retaining breech/gunner knockouts and a reduced loader. The baseline header's reduced-loader claim was stale: its recorded loader pool was empty. The revised `shows` text follows the measured records, including the burst example's changed pools.

| Scenario | Before verdict; skids/armour bites | After verdict; skids/armour bites | Pools whose remaining units changed |
|---|---|---|---:|
| s10_gun_barrel_struck | Damaged; 0/0 | Damaged; 0/0 | 0 |
| s11_mg_barrel_struck | Damaged; 0/0 | Damaged; 0/0 | 0 |
| s12_apcr_through_crew | Penetration; 0/2 | Penetration; 0/2 | 7 |
| s13_ring_through_crew | Penetration; 0/2 | Penetration; 0/2 | 6 |
| s15_unarmed_overpen | Penetration; 0/2 | Penetration; 0/2 | 0 |
| s16_restart_at_rest | NonPen; 0/3 | NonPen; 0/3 | 0 |
| s1_through_crew | Penetration; 0/2 | Penetration; 0/2 | 7 |
| s2_stopped_mantlet | NonPen; 0/1 | NonPen; 0/1 | 0 |
| s3_ricochet | Ricochet; 1/0 | Ricochet; 1/0 | 0 |
| s4_spaced_then_stopped | NonPen; 0/5 | NonPen; 0/5 | 0 |
| s5_oblique_through | Penetration; 4/1 | Penetration; 0/1 | 0 |
| s6_overpen | Penetration; 0/2 | Penetration; 0/2 | 0 |
| s7_engine_shadow | Penetration; 0/1 | Penetration; 0/1 | 2 |
| s8a_mg_ricochet | Ricochet; 1/0 | Ricochet; 1/0 | 0 |
| s8b_mg_stopped | NonPen; 0/1 | NonPen; 0/1 | 0 |
| s9_breech_and_loader | Penetration; 6/5 | Penetration; 0/2 | 4 |

Counts sum to **12 → 2 skids** and **27 → 24 recorded armour bites (MEASURED)**. Equal counts do not imply equal geometry or dose. Generated records also lose the obsolete overmatch field.

Selected visible pool changes (MEASURED; pool units, not physical harm):

- s9_breech_and_loader: Ammo_R_0 64463 → 72000; Ammo_R_1 71461 → 70327; Commander 4740 → 24977; Loader 0 → 14047.
- s13_ring_through_crew: Ammo_L_0 63835 → 53473; Ammo_L_1 0 → 34382; Breech 142000 → 133475; Commander 8744 → 14110; Engine 271698 → 270036; Gunner 1218 → 0.

## Class proofs and note comparison

The tests read every preset from the registry and the reference facts from their owner. They cover head-on exactness, density/speed/strength trends, plug-shear saturation, thin-plate grazing limit, replica scaling and the closed-form decision across a fixture sweep. Saturation is **1.607022 calibres (DERIVED from the loaded `R/(4τ)`)**.

The slope proofs cover angle/penalty monotonicity, head-on LOS identity, the ceiling including grazing, soft entrance sharing, backing independence, charged work/state, real progress and the mounted flight clock. A mounted `decide` test proves thin-plate skid, thick-plate head-on bite and more potency spent along the same real chord. A kept-carrier skid test checks work and impulse against the same assembly mass.

| Labelled skid fixture | Note ° | MEASURED ° | Difference ° |
|---|---:|---:|---:|
| 10 mm, 773 m/s | 76.6 | 76.6374 | +0.0374 |
| 45 mm, 773 m/s | 62.1 | 62.1381 | +0.0381 |
| 80 mm, 773 m/s | 53.5 | 53.5360 | +0.0360 |
| Thick, 773 m/s | 43.2 | 43.1974 | -0.0026 |
| 45 mm, 500 m/s | 48.4 | 48.3687 | -0.0313 |

| 120 mm normal-potency fixture | Note plate mm | MEASURED plate mm | Difference mm |
|---|---:|---:|---:|
| APCBC, 30° | 95.5 | 95.4906 | -0.0094 |
| APCBC, 60° | 45.4 | 45.3946 | -0.0054 |
| APCR, 30° | 87.8 | 87.8461 | +0.0461 |
| APCR, 60° | 32.1 | 32.1539 | +0.0539 |

The last row rounds to 32.2 mm at one decimal; the note says 32.1. Every result remains inside the note's measured class bands: 85–97 mm at 30°, 31–49 mm at 60°.

## Every authored number and its job

| Values introduced or relocated | Job and owner |
|---|---|
| APCBC density 8000 kg/m³, slope penalty 8° | **StandIn**, `assets/ballistics/round_types.ron`: deformation/shatter proxy; M51 contact basis, no separate cap term without evidence |
| APCR density 14000 kg/m³, slope penalty 15° | **StandIn**, same table: tungsten-core survey and class slope loss |
| AP and HE density 6000 kg/m³, slope penalty 8° | **StandIn**, same table: uncapped-steel survey and class slope loss |
| Slope-charge ceiling 5 | **Resolution**, `mechanism::SLOPE_CHARGE_CEILING`: keeps the charge finite near θ + β = 90° |
| APCR flight ogive 4 crh | **Unknown**, same type table: cap shown by TM 9-1985-3 Fig. 427 and TTT No. 24; a dimensioned Pzgr. 40 cap drawing settles the contour |
| Gravity 9.81 m/s² | **StandIn**, `physical_laws.ron`: unchanged uniform Earth-like field pending a shared environment owner |
| Air density 1.225 kg/m³ | **StandIn**, same file: unchanged one sea-level atmosphere pending weather/altitude |
| Cavity inertia 1.0 | **StandIn**, same file: unchanged approximate fit pending material-specific calibration |
| World floor -100 m | **Unit**, same file: unchanged world boundary measured in game metres |
| Inline proof speeds, lengths, densities, factors, time offsets, tolerances and sweep sizes; chosen scenario aim coordinates | **Fixture**, the tests/recorder own these inputs; they set no shipped physical law |

The law's 4 and √3, head-on unity and π/8 drag algebra are exact form constants, stated beside their laws. Protocol revision **77** and the wire hash are compatibility identifiers, not law magnitudes. Generated record numbers and the tables here are **MEASURED** outputs; stated algebraic thresholds are **DERIVED**.

## Gates and line accounting

All builds/tests used `scripts/serial-cargo`, `dev_ui,bitprobe`, the machine lock and unattended headless mode. No captures or frame/tick pricing runs were taken. Formatting, CI clippy, the required walk/camera/field/recording/headless targets and `doc_citations`, `ui_ascii`, `net_boundary`, `bake::` guards passed for the contact commit. The APCR correction additionally re-records its scenarios and passes 667 affected registry, gun, ballistics, camera and guard tests. The shipped-map regression now asserts terrain/armour contact independently of a tank's HP outcome; that focused regression and the final whole-tree guards pass 45 tests. CI clippy passes after this test refinement.

Protocol revision is **77**; `WIRE_TYPES_HASH` is `0x17afc7e1d850a2fa`, re-pinned from the tripwire after removing the record/wire verdict.


MEASURED net physical Rust lines against the dispatched baseline, including comments/blanks: **18 production lines removed; 10 test lines added**. Standalone test modules and `cfg(test)` item spans are counted as tests, the rest as production. Generated RON records and prose are excluded. Line count is evidence, not a target.
