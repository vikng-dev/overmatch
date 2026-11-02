# Contact-pressure lane evidence (#147, #262)

Subject: the game's damage model; game state only. Crew are HP pools at stations.

The [proxy decision](penetration-proxy-2026-11-02.md) owns the accepted form. The owner's amended record and `cap-contact-sources-2026-11-02.md` §§1, 5 were read from main, read only. The lane's decision copy now matches the amended record.

## Decisions and boundaries

- Contact resistance is `min(f R, 4τ x sqrt(x/s))`, `x = t_n/d`, `s = R/(4τ)`, using the march reference's existing facts. The thin branch is a **StandIn** for plate capacity below one calibre. There is no new coefficient, data field, construction field or cap term.
- APCBC density is 6000 kg/m³. Every firing row is a bite, bounding density only from below; no capped premium has earned support. APCR's 16.5° slope penalty restates existing survey ratios in this charge's form, without new firing rows. AP and HE are unchanged.
- Keep the shipped mirror, `v sin θ` and coverage blend. Resume, cursors and the damage cam's plane solver are untouched. The oblique-through record still shows its point, so its aim stays; its caption now includes the repeated skids after the plate crossing.
- Backlog rows **ballistics-23, -25, -26, -27 and -28** closed in the initial law step. **Ballistics-24 remains** with the footprint/ricochet rebuild: unowned construction consumers still need the delivered bounce exchange route. No file named by that remaining row changes in this amendment.

## What a player sees

For the labelled 88 mm, 773 m/s reference-plate illustration, the 10 mm skid limit rises from 76.6374° (previous implementation, **MEASURED**) to 82.0193° (**DERIVED**); an 80° hit now bites. On thick reference plate the limit falls from 43.1974° (**MEASURED**) to 37.4756° (**DERIVED**); a 40° hit now skids. These are class illustrations, not vehicle pass conditions. Head-on still never skids and every preset pays exactly LOS head-on.

APCR spends more potency on a slope while travelling the same real chord. Conditional on a bite, the 120 mm normal-potency illustration defeats 29.2166 mm at 60° (**DERIVED**), versus 32.1539 mm previously (**MEASURED**); a 30 mm plate at that angle is consequently no longer defeated by this illustration. APCBC's charge is unchanged: at 30° the same illustration defeats 95.4906 mm (**DERIVED**), below a 100 mm plate that the former bend could defeat. APCR still flies on its pointed ballistic cap; its 4-crh contour is **Unknown** pending a dimensioned drawing.

## Field casts: MEASURED outcomes

Counts are resolved skids/bites in `PenetrationMarks`. Original means the dispatched pre-lane baseline; previous means the landed proxy before the firing-row amendment. All six amendment output digests and remaining pools are unchanged; only their density-bearing input digests change. The other 2003 records are unchanged. The 2009-record total is now `0x76a1cf451d2cf59e`.

| Field shot | Original skids/bites | Previous skids/bites | Amended skids/bites |
|---|---:|---:|---:|
| driver | 0/2 | 0/2 | 0/2 |
| rear | 0/2 | 0/2 | 0/2 |
| left-front | 0/1 | 0/1 | 0/1 |
| left-mid | 0/2 | 0/2 | 0/2 |
| turret-left | 1/3 | 0/2 | 0/2 |
| right-rear | 0/1 | 0/1 | 0/1 |

The amendment totals stay **0 skids / 10 bites → 0 / 10 (MEASURED)**. Against the original baseline they are **1 / 11 → 0 / 10 (MEASURED)**.

## Recorded scenarios: MEASURED outcomes

Bites here count recorded armour-volume crossings; skids count recorded reflections, including HP-box faces. Field-shot counts above include all resolved crossings. Every recorded verdict survives the amendment. No scenario is re-aimed for this amendment.

| Scenario | Original skids/armour bites | Previous skids/armour bites | Amended skids/armour bites | Amended verdict | Pools changed by amendment |
|---|---:|---:|---:|---|---:|
| s10_gun_barrel_struck | 0/0 | 0/0 | 0/0 | Damaged | 0 |
| s11_mg_barrel_struck | 0/0 | 0/0 | 0/0 | Damaged | 0 |
| s12_apcr_through_crew | 0/2 | 0/2 | 0/2 | Penetration | 7 |
| s13_ring_through_crew | 0/2 | 0/2 | 0/2 | Penetration | 0 |
| s15_unarmed_overpen | 0/2 | 0/2 | 0/2 | Penetration | 0 |
| s16_restart_at_rest | 0/3 | 0/3 | 0/3 | NonPen | 0 |
| s1_through_crew | 0/2 | 0/2 | 0/2 | Penetration | 0 |
| s2_stopped_mantlet | 0/1 | 0/1 | 0/1 | NonPen | 0 |
| s3_ricochet | 1/0 | 1/0 | 1/0 | Ricochet | 0 |
| s4_spaced_then_stopped | 0/5 | 0/5 | 0/5 | NonPen | 0 |
| s5_oblique_through | 4/1 | 0/1 | 11/1 | Penetration | 0 |
| s6_overpen | 0/2 | 0/2 | 0/2 | Penetration | 0 |
| s7_engine_shadow | 0/1 | 0/1 | 0/1 | Penetration | 0 |
| s8a_mg_ricochet | 1/0 | 1/0 | 1/0 | Ricochet | 0 |
| s8b_mg_stopped | 0/1 | 0/1 | 0/1 | NonPen | 0 |
| s9_breech_and_loader | 6/5 | 0/2 | 0/2 | Penetration | 0 |

The amendment changes totals **2 skids / 24 armour bites → 13 / 24 (MEASURED)**; the original baseline was **12 / 27 (MEASURED)**. Only two generated scenario records change: `s5_oblique_through` retains its plate crossing and cone, then records 11 skids between the ammunition box and inner hull face; `s12_apcr_through_crew` retains its topology with different doses. These repeated skids are reported, not removed through an aim change or a new direction law.

APCR remaining-pool changes (MEASURED; pool units):

- s12_apcr_through_crew: Ammo_L_0 68446 → 68428; Ammo_L_1 64784 → 64747; Breech 140365 → 140356; Commander 26952 → 26936; Engine 323346 → 323282; Gunner 23902 → 23871; Transmission 478456 → 478453.

The APCR record still has the driver station knocked out, with gunner and commander reduced. The other scenario captions retain their recorded points. The original lane's oblique-through and mixed-outcome examples were re-aimed before this amendment; git retains those fixture changes.

## Class proofs and record comparison

Tests read every preset from the registry and reference facts from the march's owner. They prove head-on exactness, density/speed/strength trends, replica scaling, grazing as thickness vanishes, and agreement with the closed-form critical sine across a generic fixture sweep. The replacement plate proof checks strict increase up to the reference knee, continuity across it, saturation above it, and capacity below plain plug shear below it. A separate sweep proves the skid limit falls as thickness rises. The plain-branch illustration pins are deleted; firing rows stay outside tests.

The mounted `decide` fixture moves its thin-plate graze from 80° to 85° (Fixture), retaining the thin-plate skid / thick head-on bite assertion. It still proves greater potency spent along the same real slope chord and real flight clock. Named mutants include the plain plug branch, a discontinuous knee, a reversed thickness trend, a fixed skid angle, ignoring the slope preset and charging travelled length. Head-on slope proofs reject `cos(θ+β)` without `cos β`.

The departing-round camera proof samples after one model length has cleared the final bend, using the authored model length and recorded last-leg speed. A fixed delay assumed all departures were fast; the slow reflected record exposes that false assumption. The same class proof still rejects a clamped nose, a hidden departing body and a trail ending at the last recorded node. No cursor or plane-solver code changes.

The reference knee is **1.607022 calibres (DERIVED)** from the loaded `R/(4τ)`. The following are analytical **DERIVED** evaluations from the shipped facts and presets, not firing-row test assertions.

| Labelled skid illustration | Amended note ° | DERIVED ° | Difference ° |
|---|---:|---:|---:|
| 10 mm, 773 m/s | 82.0 | 82.0193 | +0.0193 |
| 45 mm, 773 m/s | 65.7 | 65.6996 | -0.0004 |
| 80 mm, 773 m/s | 53.5 | 53.4888 | -0.0112 |
| Thick, 773 m/s | 37.5 | 37.4756 | -0.0244 |
| 45 mm, 500 m/s | 53.3 | 53.3505 | +0.0505 |

| 120 mm normal-potency illustration | Amended note mm | DERIVED mm | Difference mm |
|---|---:|---:|---:|
| APCBC, 30° | 95.5 | 95.4906 | -0.0094 |
| APCBC, 60° | 45.4 | 45.3946 | -0.0054 |
| APCR, 30° | 86.2 | 86.1502 | -0.0498 |
| APCR, 60° | 29.2 | 29.2166 | +0.0166 |

Two differences are reported to the owner: the 45 mm / 500 m/s result rounds to 53.4°, while the note says 53.3°; APCR at 60° is **1.7834 mm below** the note's displayed **31–49 mm** class band (**DERIVED**). Its 30° result remains inside the **85–97 mm** band. The owner's selected law and 16.5° preset stand; no retuning hides either difference.

## Every authored number and its job

| Values | Job and owner |
|---|---|
| APCBC density 6000 kg/m³, penalty 8° | **StandIn**, `round_types.ron`: deformation/shatter proxy; surveyed bites give a lower bound, no separate capped premium |
| APCR density 14000 kg/m³, penalty 16.5° | **StandIn**, same table: core density proxy; survey ratios restated in the charge's form, no new firing rows |
| AP and HE density 6000 kg/m³, penalty 8° | **StandIn**, same table: unchanged uncapped-steel and slope proxies |
| Settlement pair: t/d ≥ 1.6, θ ≥ 40°, speed ≥ 600 m/s | Evidence criterion in APCBC's **StandIn** provenance, from the owner's record; no simulation gate |
| Slope-charge ceiling 5 | **Resolution**, `mechanism::SLOPE_CHARGE_CEILING`: finite charge as θ + β approaches 90°; unchanged |
| APCR flight ogive 4 crh | **Unknown**, `round_types.ron`: cap present in TM 9-1985-3 Fig. 427 and TTT No. 24; contour awaits a dimensioned drawing |
| Gravity 9.81 m/s²; air density 1.225 kg/m³; cavity inertia 1.0 | **StandIn**, `physical_laws.ron`: unchanged environment and cavity priors relocated in the initial law step |
| World floor -100 m | **StandIn**, same file: finite play boundary until maps author vertical play bounds |
| Inline speeds, lengths, densities, angles (including the 85° mounted graze), factors, neighbourhoods, tolerances and sweep counts; scenario aim coordinates | **Fixture**, tests/recorder inputs; no firing rows or physical tuning gates |

The law's 4, √3 and square root, and head-on unity are exact form, not new authored magnitudes. `s` is derived from existing facts. The compatibility identifiers are owned by `protocol::PROTOCOL_REV` and `protocol::WIRE_TYPES_HASH`; this amendment changes no wire type. Field hashes are generated compatibility pins. Outcome tables are **MEASURED**; analytical illustrations and thresholds are **DERIVED**.

## Review disposition

| Reviewer/source | Finding | Disposition |
|---|---|---|
| Firing-row research, #147 / #262 | Plain plug capacity too high on thin plates; capped density premium unsupported; APCR penalty uses the survey's other definition | Owner's amended form and presets implemented; class proofs replace numerical row pins |
| Lane self-review, five lenses | Stale APCR flight prose and tests pinning preset illustrations | Prose follows the cap; law tests are generic and read their data owners; no independent-review claim |
| Required camera gate | Fixed sample delay asserted body alignment before a slow departure cleared its last bend | Test samples derive clearance from model length and record speed; playback and plane solver unchanged |
| Analytical comparison with amended note | One rounding difference and APCR's 60° class-band mismatch | Reported above; owner-selected law unchanged |

Existing independent-review tables in the design doc are unchanged; this table records the amendment's research disposition and lane self-review.

## Gates and measured line accounting

`cargo fmt --all --check` and CI clippy (`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`) pass. The final unblessed batch passes **673 tests**, with **12 ignored** tools/pricing beds: `ballistics::`, `pen_cam`, `field_casts`, `damage_cam`, `recording_tests::`, `headless_test`, `rounds::`, plus whole-tree guards `doc_citations`, `ui_ascii`, `net_boundary`, `bake::`. The departing-body regression also passes in isolation. No unresolved test failure remains.

All builds/tests use `scripts/serial-cargo`, `dev_ui,bitprobe`, the machine lock and unattended headless mode. No captures, timing/pricing runs or screen-taking game runs. The scenario and field records are regenerated on macOS arm64; a normal unblessed run verifies their pins.

MEASURED physical Rust line counts, comments/blanks included, standalone test modules and `cfg(test)` spans separated: this amendment adds **11 production lines** and removes **8 test lines**. Across the whole lane against the dispatched baseline, **7 production lines are removed net and 2 test lines are added net**. Generated RON, pins and prose are excluded. Line count is evidence, not a target.
