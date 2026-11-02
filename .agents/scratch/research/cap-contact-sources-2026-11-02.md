Subject: the game's damage model; game state only.

# Skid or bite: firing rows against the contact law and the per-type presets (2026-11-02)

Scope follows the coordinator's redirect: Q1 and Q2 calibrate `contact_density` and `slope_penalty` per type (`penetration-proxy-2026-11-02.md`); Q3 dropped except as a density input; Q4 and Q5 unchanged.

## TLDR

- **Q1, cap term: settled, negative.** No capped/uncapped critical-angle pair exists in anything reached. The shortfall the survey read as "the contact law must read the cap" also hits uncapped rounds, so no cap term and no capped density premium has been earned.
- **Q2, density: narrowed.** NPG reduces every round on whole mass with no length; cavity and solid rounds differ by 4–10 % on F. No separate hollow column. Length is not tested by these rows.
- **Q3, hit lengths: dropped.** Inputs I needed are labelled ASSUMED (§3).
- **Q4, thin-plate term: narrowed, with a correction.** 4τ·t/d holds on mild steel (within 2° at t/d 1.6–1.9; at 2.6 it reads at least 1° high) and overstates plate capacity ×1.6 at t/d 1, ×2 at 0.5, ×3 at 0.25, ×7 at 0.1. Saturation follows R. Vayig & Rosenberg's form was not reached.
- **Q5, skid exit: narrowed.** A slide fits rigid cones below about 0.6 V50 (restitution about 0.15), but the exit swings to the entry angle at V50. No steel-on-steel retained speed was found; spheres on aluminium keep 0.14–0.81 of v sin θ. Treat v sin θ as an upper bound.
- **Presets:** see §1. Change the plug branch, merge capped into steel at 6,000, restate tungsten β as +16.5°. Tungsten ρ and blunt β have no support beyond what they already rest on.

## 1. Preset table

β here is the record's own form, `cos θ − sin θ tan β = t(θ)/t(0)`. The survey used `cos(θ + β) = t(θ)/t(0)`, which reads up to 4° lower on the same ratios, most at low θ (§6). Row counts cover both blocks of §8.

| Preset | Current | Rows support (DERIVED) | Rows | Recommendation |
|---|---|---|---|---|
| Capped ρ | 8,000 | With the §5 branch: 6,000 passes 47 of 50 rows; the 3 misses are t/d 0.16 at 140–237 m/s. With the accepted branch, no single value: ≥ 7,575 at t/d ≥ 0.65, and 8,900–23,300 at t/d ≤ 0.34 | 50, on 9 capped constructions: M51 4, M61 5, Type A-1 family 9, M51B2 model 4, Mk 11 model 3, M62 21, Mk 29-2 4 | **6,000 (merge with steel), with the §5 branch.** Without it keep 8,000, knowing it fails 25 of the 50, every one at t/d ≤ 0.34. M51's own m/(A·L) is 6,568. Nothing excludes 8,000 under the §5 branch: these rows are bites, so they only bound ρ from below |
| Uncapped steel ρ | 6,000 | 6,000. Solid core 6,241 from measured size; M79 5,933 (length ASSUMED) | 25 uncapped + 4 core thickness rows | **Keep.** With the §5 branch 9 of 25 pass outright; 16 miss by at most 5.6° (T33E7 1.6°, the rest at 184–381 m/s) |
| Tungsten core ρ | 14,000 | Nothing new. One chart row bounds it at ≥ 6,068 (saturated R, 1,234 m/s, 55°) | 0 new | **No support beyond what it already rests on.** Keep as `StandIn` |
| Capped β | +8° | +8.8° mean at ≥ 590 m/s (range +0.5 to +12.5); +0.6° mean below 590 m/s (range −8 to +9). Plus one bound, ≥ +13° at 60°, 840 m/s | 14 fast (M62 6, Mk 29-2 3, M51 5); 16 slow (M62 14, Mk 29-2 1, M61 1) | **Keep +8°.** Confirmed at game speeds by two rounds beyond M51. It overcharges a spent round at ≤ 45° |
| Uncapped sharp β | +8° | M79, all ≤ 431 m/s: −6 to −1 at 30°, −7 to +1 at 45°, +4 to +13 at 60°. Fast: T33E7 about +9 (one row, F₀ ASSUMED); M72 +4 to +14 (survey's rows) | 27 by-eye cells + 3 measured M79 rows + 1 fast | **Keep +8°.** Only one new row at speed. Slow rows show a shape a constant β cannot follow |
| Uncapped blunt β | +2° | Flat-nose Mk 29 is the only firing data: −6, −2, +6 at e/d 0.36; +1, +14 at e/d 0.66 (F₀ ASSUMED). It tracks M79 within 1° at e/d 0.36, so it does not show blunt beating sharp | 5, all ASSUMED F₀, and flat is not APBC | **No support beyond what it already rests on** (Potapov, calculated). No shipped user: leave it out of data until a blunt round ships |
| Tungsten core β | +15° | +16.9° mean of the same ratios in the record's form | 0 new; 6 re-derived | **+16.5°.** A definition correction, no new evidence: 1.6 mm at 30° and 3 mm at 60° on a 120 mm round |

**The second capped round.** M62 (15 rows) and M61 (4 rows) are measured capped rounds, and neither confirms 8,000. On plates t/d ≥ 0.65 they pass at 8,000 under the accepted branch, while on thinner plates they need 8,900–23,300. M79 uncapped on the same plates needs 10,000–12,000, so the demand tracks plate thickness and not the cap.

## 2. Method and labels

- **MEASURED:** striking speeds, F limits, plate data, as printed. **DERIVED:** computed by me, scripts below. **ASSUMED:** my input. **CLAIMED:** a source's unverified statement or theory. **BY EYE:** read off a chart, flagged where used.
- NPG's coefficient: F = M^½ V cos θ / (e^½ d), M in lb, V in fps, e and d in feet. DERIVED check: M79 at 15 lb, 1,400 fps, 0.73 in, 60° gives 43,968 against the printed 43,800. "The whole mass of the projectile is used in the functions" (NPG 1125, NPG 1211).
- **Law A (accepted):** R_eff = min(R, 4τ·t/d), R = 3.711 GPa, τ = 0.577 GPa (shipped RHA facts), preset ρ. No per-plate strength correction; Hershey reports no correlation of service-round F with tensile strength at high obliquity (96,000–159,000 psi, NPG 1125).
- **Law B (§5):** plug branch × √((t/d)/s), s = R/(4τ) = 1.61, ρ = 6,000.
- q = R_eff sin θ / (ρ v² cos²θ); q ≥ 1 predicts a skid. A perforation or a nose through the plate is a bite, so it needs q < 1. **ρ_min** is the density that makes q = 1 there. Most rows only bound the law from one side: a bite says nothing about where the skid begins.
- β from a row: find e₀ with F₀(e₀/d)·√e₀ = F(e/d, θ)·√e / cos θ (equal speed, equal round), r = e/e₀, tan β = (cos θ − r)/sin θ. Mass cancels.
- Scripts, standard library only: `/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/scripts/cap_rows.py`, `cap_extra.py`, `cap_extra2.py` and `cap_more.py` in the same directory.

**A correction to the repo's notes.** "Ballistic Summary Part IV" is NPG 1125 (high obliquity, AD0063448). The cap effect is Part III, NPG 1211 (AD0063985). The ricochet theory is Analytical Summary Part VI, NPG 1041 (AD0063447). The earlier note's title "Part IV: the cap effect" is wrong, and ADA955285 is Analytical Summary Part IV (NPG 9-46), a different report.

## 3. Inputs (Q3 reduced)

| Input | Value | Label | Used for |
|---|---|---|---|
| M79 | 15.00 lb, 3 in, solid with tracer cavity | MEASURED (NPG 16-43 p.2; TM 9-1904 p.448) | V from F |
| M79 hit length | 3.3 calibres, giving ρ_e 5,933 | ASSUMED; no length found | R_BL column only |
| M62 BS | 15 lb | ASSUMED; cancels in β, enters V as √M | V from F |
| 37 mm M51 | 0.835 kg over 4.63 in, ρ_e 6,568 | Survey §4 | Comparison |
| 7.62 core | 5.2 g, 6.2 mm, 27.6 mm, ρ_e 6,241 | MEASURED size (Madhu 2003), DERIVED ρ_e | Q4 |
| 90 mm T33E7 | 24.1 lb | ASSUMED from memory, not in the report; affects F₀-based β only, not q | β, V_BL |
| T33E7 construction | AP-T with windshield, no cap mentioned; classed uncapped | ASSUMED | Class |
| 6 in Comm Mk 27-7 | 105 lb, hooded, hollow | ASSUMED mass | V from F |

The 8.8 cm Pzgr. 39 and HE lengths and the S.m.K. dimensions were not sought after the redirect.

## 4. Q1 and Q2: cap and density

**What exists (all MEASURED, NPG 16-43, printed p.2, p.4, DTIC AD0310017).** Six caps on one Type A-1 body, plus an uncapped M79, on one plate. F as % of M79 under the same condition:

| Round, cap % of mass | 0°, e/d 0.65 | 0°, e/d 1.0 | 30°, 0.24 | 30°, 0.65 | 60°, 0.24 |
|---|---|---|---|---|---|
| 8 in Mk 11 model 3001, 6 % | 106+ | 101 | 101 | 98 | 96 |
| M51B2 model, hard (BHN 485), 11 % | 111 | 107 | 95 | 102 | 99 |
| M51B2 model, soft (BHN 210), 11 % | 109 | 107 | 109+ | 106 | — |
| Type A-1 Light, 10 % | 111 | — | — | 108 | 95 (from F; table OCR 91) |
| Type A-1 Regular, 12 % | 112 | 106 | 100+ | 110 | 91.5 |
| Type A-1 Heavy, 16 % | 116 | 109 | 104 | 113 | 95 |
| M61, 15 % | 116 | 112 | 103 | 114 | 86 |

- At 0° and low obliquity a cap costs about its mass: "Caps interfere ... in proportion to their relative weights ... not influenced by cap shape." NPG 1211 gives 0.50–0.75 % F per % cap mass at 0°. Full loss of the cap's energy would be 0.5 (DERIVED). This belongs to potency and the service tables already carry it.
- At 60° on thin plate a cap helps, by up to 14 %, more with mass, hardness and a blunt angular shoulder: M61 is "9% better" than Type A-1 Heavy at similar mass because its contour can "'bite' the plate better".
- An intact cap acts as contour alone: "The dynamics of the cap and body are then the same as the dynamics of a monobloc projectile with the same external contour" (NPG 1211 pp.16–18).
- The effect ends: shattered-cap ΔF/F is 0.000 at θ ≥ 60° and e/d ≥ 0.34 (M62, NPG 1211 Table VII p.28), and at θ ≥ 50° and e/d ≥ 0.46 (6 in Mk 35-5, Table VIII p.29).
- **Same body, ± cap:** only at about 0° (37 mm M51B2 at t/d 0.83: 53,500 against 48,900; 54,400 against 50,200; at 1.69: 57,200 against 52,300; Type A at about 0.97: 52,500 against 45,500). At 45–60° the Type A − cap rows sit on other plates: F 37,000 at 60°, e/d 0.26, TS 119,000, against Type A-1 Regular 40,000 at e/d 0.24, TS 130,300. Hence no isolated cap effect at obliquity, and my reading "Type A-Cap" = decapped Type A in NPG 1125 is an inference from NPG 1211's notation.
- **Skid rows for a cap:** one only. The soft M51B2 model ricocheted at 30°, 197 and 215 m/s, on a plate where the hard version perforated at 192 m/s (§8). Law A gives q 1.21 / 1.00 and law B 0.63 / 0.52, so law B misses them. It is a soft-cap, near-limit case; I add no term for it.

**The law against these rows (DERIVED, §8).**

- M79, uncapped, 60°, t/d 0.244, nose through at 402 m/s and perforated at 431 m/s: law A with ρ 6,000 gives q 2.02 and 1.75.
- T33E7, uncapped, 60°, t/d 0.847, V50 857 m/s: q 1.54.
- M61 at 378 m/s, same plate as M79: q 1.71 at ρ 8,000.

So the M51's shortfall is not a cap fact. Raising ρ cannot cure it: ρ_min runs 2,500–7,600 at t/d ≥ 0.65 (except T33E7's 9,214) and 8,900–23,300 for capped rounds, 10,000–21,000 for uncapped, at t/d ≤ 0.37. A density is a property of the round, and this demand follows the plate.

**Verdict, Q1.** The data do not support a cap term in the contact law. The cap's measured effect is on the limit at high obliquity, which in the proxy is β. On one plate at 60°, e/d 0.24, M79 reads +12.8°, the capped rounds +8.1 to +12.6° (DERIVED, F₀ = M79 Fig. 3, BY EYE, ASSUMED equal below e/d 0.3, supported by M62's F₀ of 30,000 against M79's 29,700 at e/d 0.16). That is 0–5° in the cap's favour on thin plate; not enough to split the class. The survey's line "The contact law must read the cap" should be retired.

**Verdict, Q2.**

- **Mass:** the whole mass. NPG's coefficient collapses filled, cavity and solid rounds on it.
- **Length:** not tested. F has no L, and no pair here varies L at fixed mass. With a per-type ρ the question goes away.
- **Cavity:** MEASURED, M62 BS against M79 on one plate (No. 017784, TS printed 111,000 and 113,000 on the two rows, 30°, e/d 0.33): 35,700 against 37,200, −4 %. MEASURED, 4 in Comm's deviation from standard F is 0 to −4 up to 60° and +3 to +4.5 at 70–75°, tracking an inertial parameter of 1.64 against M79's 2.01 (NPG 1125 Table I; the 75° entry is partly legible). BY EYE, 6 in Comm at 75°, e/d 0.2: F about 33,500 against M79's about 30,300: +10 %.
- **So** ±10 % on F is ±20 % on ρ·V² at most, inside plate scatter. No hollow column.

## 5. Q4: the thin-plate term

**Thick end, confirmed.** Gupta & Madhu's series, reached through Raguraman, Deb & Gupta 2008 (LAJSS 5, Table 1, p.230, test column, and text p.231) and Madhu et al. 2003 (repo note R1). Core ρ_e 6,241. Mild steel Y 327 MPa from the 20 mm plate, ASSUMED for all; a 1.46, H 2.19, τ 0.189 GPa (DERIVED).

| t mm, t/d | v m/s | Measured | 4τ, R=a | 4τ, R=H | 2τ, R=H | Law B, R=a | Law B, R=H | V_BL form |
|---|---|---|---|---|---|---|---|---|
| 10, 1.61 | 821 | 61.5° (test); perforated 45° at 790 | 60.0 | 60.0 | 68.5 | 61.2 | 63.9 | 60.4 |
| 12, 1.94 | 815 | ricochet 59°; perforated 45° at 808 | 57.1 | 57.1 | 66.3 | 57.1 | 60.0 | 57.3 |
| 16, 2.58 | 806 | ricochet 45° and 51°; perforated 30° at 818 | 56.7 | 52.0 | 62.5 | 56.7 | 53.0 | 50.3 |
| 20, 3.23 | 820 | 51° | 57.2 | 50.6 | 59.9 | 57.2 | 50.6 | — |

- 2τ, my earlier hypothesis, is rejected by 6–11°.
- Cross-check of the coefficient: ρ_e·V_BL² from the 0° residual speeds (V_BL² = v² − v_r², Lambert–Jonas p = 2 ASSUMED) is 1.196 / 1.445 / 2.222 GPa, against 4τt/d 1.218 / 1.462 / 1.949 (DERIVED). Agreement within 2 % at t/d 1.6–1.9 and 14 % at 2.6.
- **Saturation:** with R = a the law flattens at t/d 1.93 and misses the 20 mm plate by 6°. With R = H it flattens at 2.9 and lands on 50.6°. So saturation follows R. For the shipped RHA, 1.61 with R = a, or 2.42 with H (DERIVED). No RHA row above t/d 1.8 exists here, so 1.6 is neither confirmed nor refuted for armour steel.
- The 16 mm "ricochet at 45°" sits below the 20 mm plate's 51°. Both cannot be clean thresholds; I have that row only from the citing table.

**Thin end, corrected.** Hershey's standard M79 curves against STS (NPG 1125 Fig. 3, F at 0°, BY EYE ±1,000; e/d 0.65 and 1.0 from NPG 16-43). R_BL = ρ_e V_BL² is what the plate can hold against this round at its normal limit. Using ρ_e 5,933 ASSUMED:

| t/d | F₀ | V_BL m/s | R_BL GPa | 4τt/d GPa | R_BL / 4τt/d | Law B GPa | Capability law V_BL (low-speed limit) |
|---|---|---|---|---|---|---|---|
| 0.10 | 23,800 | 74 | 0.033 | 0.231 | 0.14 | 0.058 | 195 |
| 0.20 | 33,000 | 145 | 0.125 | 0.462 | 0.27 | 0.163 | — |
| 0.25 | 36,600 | 180 | 0.192 | 0.577 | 0.33 | 0.228 | ≈ 302 at 0.24 (measured ≈ 173) |
| 0.30 | 39,500 | 213 | 0.269 | 0.693 | 0.39 | 0.299 | — |
| 0.50 | 45,100 | 314 | 0.584 | 1.155 | 0.51 | 0.644 | 435 |
| 0.65 | 46,300 | 367 | 0.800 | 1.501 | 0.53 | 0.955 | — |
| 1.00 | 50,800 | 500 | 1.482 | 2.309 | 0.64 | 1.822 | 616 |

Local exponent of R_BL in t/d (DERIVED from those by-eye F₀): 1.9 over 0.1–0.2, 1.7 over 0.2–0.5, 1.3 over 0.5–1. deMarre's century-old V ∝ e^0.7 gives 1.4.

**Recommended change.**

`R_eff = min(f·R, 4τ·(t_n/d)·√((t_n/d)/s))`, `s = R/(4τ)` (1.61 for the shipped facts).

- It is unchanged at t/d ≥ s, so head-to-head on thick plate nothing moves.
- It adds one exponent and no scale. Label it `StandIn`, naming the missing law: plate capacity below one calibre, where a pointed round opens the plate rather than plugging it. The physical account of that fall-off is my reading, not a measured statement.
- It runs 10–30 % above R_BL over t/d 0.2–1 and 76 % above at t/d 0.1 (by-eye F₀), so q errs toward skid. That is where the uncapped misses in §8 come from. T33E7 misses by 1.6°.

Skid limit at 773 m/s (DERIVED):

| t | Accepted, ρ 8,000 | Accepted, ρ 6,000 | Law B, ρ 6,000 | Law B, ρ 8,000 |
|---|---|---|---|---|
| 10 mm (0.114) | 76.6° | 74.6° | 82.0° | 83.1° |
| 30 mm (0.341) | 67.1° | 63.7° | 71.9° | 74.3° |
| 45 mm (0.511) | 62.1° | 58.1° | 65.7° | 68.9° |
| 80 mm (0.909) | 53.5° | 48.5° | 53.5° | 58.0° |
| 115 mm (1.307) | 47.1° | 41.6° | 43.6° | 49.0° |
| 200 mm (2.273) | 43.2° | 37.5° | 37.5° | 43.2° |
| 45 mm at 500 m/s | 48.4° | — | 53.3° | — |

Calibre 88 mm on those rows. Note the cost: thick-plate θc falls 5.7° for capped rounds on merging to 6,000, and no row tests that. Unsettled item 1 below is the one that could restore a capped premium. How much it shows (DERIVED, β +8°): at the Law B, ρ 6,000 limit the charged chord is already 159 mm on 45 mm, 166 mm on 80 mm and 183 mm on 115 mm, so for a round of 120 mm normal potency the merge decides a skid against a stopped bite on those plates, not whether the plate is perforated.

**A form that needs no exponent, not recommended yet.** Writing R_eff = ρ_e V_BL² gives `cos²θc / sin θc = (V_BL / v)²`: ρ cancels, and θc depends on V/V_BL, the variable Vayig & Rosenberg 2023's abstract names. This is my form; theirs was not reached. DERIVED against rows:

- M51, t/d 0.75: 62.3°, perforated to 62° (F₀ 53,500 from M51B2 at t/d 0.83).
- T33E7: 59.0° against V50 at 60° (F₀ 52,000 ASSUMED).
- Gupta & Madhu: last column above.
- M79 Fig. 3, limit speed at 45° / 60° over V_BL: 1.25–1.27 / 2.09–2.39, against skid-line 1.19 / 1.86. q is 0.88–0.90 at 45° and 0.61–0.79 at 60°.
- 6 in Comm at 75°: 3.92–4.55 against 3.80, q 0.70–0.94 (BY EYE).

So on thin plate the measured limit runs just above the skid line from 45° to 75°, which fits Hershey's statement that plastic energy at high obliquity "increases more rapidly with obliquity because of projectile ricochet". It cannot ship as is: the game's capability law has no thin-plate fall-off, so its own V_BL reads ×2.6 / ×1.7 / ×1.4 / ×1.2 high at t/d 0.1 / 0.24 / 0.5 / 1 (table above; DERIVED, 15 lb, 3 in), which returns it to about 4τt/d. That gap is a potency matter outside this lane and worth its own look.

**Hershey's own bound (CLAIMED, theory).** NPG 1041 pp.37–38, for M79: cavitation above cot⁻¹((0.271 e/d)^½), complete penetration impossible above cot⁻¹((0.088 e/d)^½). DERIVED check: e/d 0.03 gives 84.8° and 87.1°, matching his Table VII. That is 81.7° at e/d 0.24 and 75.6° at 0.75. Law B stays below it at 773 m/s (82.0° against 84.3° at t/d 0.114), so it does not bind.

## 6. Slope penalty rows

All DERIVED from MEASURED F unless marked. The F₀ of M62 is its own series at e/d 0.162, 0.334, 0.653, 1.01, 1.408 (NPG 1211 pp.50–56, verified upright for p.56), extrapolated above 1.408, ASSUMED; rows with e₀/d above that are marked †.

**Capped, M62 BS against its own 0° series:**

| θ | e/d | F | V m/s | r | β |
|---|---|---|---|---|---|
| 30 / 40 / 50 / 60 | 0.163 | 27,300 / 27,000 / 30,300 / 29,800 | 125 / 140 / 187 / 237 | 0.936 / 0.807 / 0.561 / 0.436 | −8.0 / −3.6 / +6.1 / +4.3 |
| 30 / 40 / 50 / 60 | 0.33 | 35,700 / 38,400 / 39,100 / 43,900 | 234 / 286 / 347 / 501 | 0.898 / 0.751 / 0.629 / 0.391 | −3.7 / +1.3 / +1.1 / +7.2 |
| 20 / 30 / 40 / 45 | 0.65 | 52,700 / 50,700 / 48,400 / 53,200 | 447 / 466 / 502 / 599 | 0.955 / 0.878 / 0.756 / 0.550 | −2.6 / −1.4 / +0.9 / +12.5 |
| 20 / 30 / 40† / 45† | 1.01 | 54,200 / 52,000 / 52,100 / 55,000 | 571 / 594 / 673 / 769 | 0.923 / 0.862 / 0.701 / 0.566 | +2.9 / +0.5 / +5.7 / +11.3 |
| 20† / 30† | 1.39–1.40 | 57,100 / 55,300 | 704 / 743 | 0.894 / 0.827 | +7.7 / +4.4 |
| 29.8 / 45 | 0.675, one plate (X18305) | 49,500 / 48,300 (0°: 52,000) | 461 / 551 | 0.833 / 0.595 | +4.0 / +9.0 (shape scaled, ASSUMED) |
| 60 | 0.649 | ≥ 53,000, not perforated | ≥ 840 | ≤ 0.30 | ≥ +13 (F₀ ASSUMED) |

**Capped, others.** Mk 29-2 (M62 shape scaled to its 57,500 at e/d 0.663, ASSUMED): 30° / 0.659 / 515 m/s −6.0; 45° / 0.663 / 639 m/s +7.7; 30° / 1.011 / 699 m/s +7.4; 40° / 1.011 / 779 m/s +8.5. M61 (own 0° rows): 30° / 0.647 / 473 m/s −1.4.

**Re-derived in the record's form from the survey's ratios** (form A = record, form B = survey; chart values per survey §5.1):

| Round | θ: form A / form B |
|---|---|
| M51 capped, 890 m/s | 10: +10.4/+7.6; 30: +11.1/+9.8; 45: +12.4/+11.6; 55: +12.0/+11.4; 60: +11.7/+11.3 |
| Sub-calibre capped steel core, 1,253 m/s | 30: +13.1/+11.4; 45: +14.8/+13.7; 55: +12.0/+11.4 |
| M61 service table | 30: +6.4 to +7.5 / +5.9 to +6.9 |
| M62 service table | 30: +6.4 to +9.8 / +5.9 to +8.7 |
| M72, 100 / 1,000 / 2,000 yd | 30: +14.1, +8.6, +4.1 / +12.3, +7.8, +3.9 |
| Jacketed WC core, 1,234 m/s | 10: +13.2/+9.3; 30: +16.6/+14.2; 45: +16.3/+15.0; 55: +16.6/+15.7 |
| WC band: capped / uncapped / centre | 30: +6.4 / +18.4 / +13.1 (form B +5.9 / +15.6 / +11.4) |
| Potapov, calculated: blunt / sharp / APCR | 30: +0.7 to +1.8 / +4.1 to +10.9 / +16.3 to +17.3 |
| M93, 20° to 30° ratio 0.84–0.87 | +13.5 to +19.8 (form A) |
| Army rule; Navy 1932 (rules, not firings) | 20–45: +7.5 to +11.5; 30–60: +7.9 to +10.2 |

**Uncapped sharp, M79 standard curves** (NPG 1125 Fig. 3, BY EYE ±1,000, against STS at 115,000 psi):

| e/d | 30° r, β | 45° r, β | 60° r, β |
|---|---|---|---|
| 0.10 | 0.917, −5.9 | 0.777, −5.7 | 0.418, +5.4 |
| 0.15 | 0.922, −6.4 | 0.794, −7.0 | 0.445, +3.7 |
| 0.20 | 0.921, −6.3 | 0.792, −6.8 | 0.419, +5.3 |
| 0.25 | 0.914, −5.4 | 0.775, −5.5 | 0.368, +8.7 |
| 0.30 | 0.907, −4.7 | 0.760, −4.3 | 0.345, +10.1 |
| 0.35 | 0.903, −4.3 | 0.731, −1.9 | 0.328, +11.2 |
| 0.40 | 0.886, −2.3 | 0.709, −0.2 | 0.318, +11.9 |
| 0.45 | 0.884, −2.1 | 0.691, +1.3 | 0.307, +12.6 |
| 0.50 | 0.871, −0.6 | 0.693, +1.1 | 0.302, +12.9 |

Measured M79 rows: 30°, e/d 0.65, F 44,300: r 0.866, β 0.0; on plate 694385 (TS 130,300), 30°, e/d 0.243, F 34,300: −4.4; 60°, F 43,800: +12.8. So below about 430 m/s an intact pointed monobloc beats line of sight up to 45° and pays past it, as Hershey says. T33E7, 60°, 857 m/s: F 46,768 (DERIVED), +7.6 to +10.9 for F₀ 58,000–54,000, ASSUMED.

**Blunt.** Flat-nosed Mk 29 (uncapped, windshield), F₀ ASSUMED: at e/d 0.66 taken as 0.95 × M79 from the ≥ 44,000 row, at e/d 0.36 as 0.85 × M79 per NPG 19-44. 30° / 0.660 / 422 m/s +1.3; 45° / 0.662 / 604 m/s +13.9; 30° / 0.362 / 244 m/s −5.8; 45° / 0.357 / 281 m/s −2.1; 60.2° / 0.361 / 419 m/s +6.1. NPG 1125's nose series at 45°, e/d 0.244 (MEASURED, one plate) runs the other way from a blunt discount: hemisphere > 35,600; Type A-1 contour 33,500; 3- and 4-calibre ogive 30,100, with no 0° rows to reduce them.

**Reading.** β rises with speed for both steel classes, and a constant cannot hold that; the proxy's "Given up" list already accepts it. At game speeds the rows confirm the values, and the slow rows mean a spent round pays more than it should at ≤ 45°, up to about 8 % of plate.

## 7. Q5: the skid exit

**Rigid cones on steel (NWC TP 5607, Fig. 11 p.14; .50 cal, 4340 Rc 53, on 4130, T/d 0.5, entry 30° from the normal, zero yaw). BY EYE ±2°, ±15 fps. Restitution e DERIVED, assuming tangential speed kept.**

| Nose half-angle | (V fps, exit from normal, e) |
|---|---|
| 30° | 280, 75°, 0.15; 330, 75°, 0.15; 500, 75°, 0.15; 515, 74°, 0.17; 600, 80°, 0.10; 765, 83°, 0.07; 945, 75°, 0.15; 1,075, 55°, 0.40 |
| 60° | 485, 81°, 0.09; 640, 70°, 0.21; 775, 48°, 0.52; 905, 50°, 0.48; 895, 45°, 0.58; 1,125, 27°, 1.13 |
| 90° | 515, 80°, 0.10; 530, 80°, 0.10; 550, 69°, 0.22; 590, 65°, 0.27; 865, 68°, 0.23; 860, 59°, 0.35; 915, 35°, 0.82 |

V50 at T/d 0.5, 30°, MEASURED: 1,422 fps (30° half-angle) and 1,232 fps (60°). "The ricochet obliquity appears to approach the impact obliquity as the impact velocity approaches the ballistic limit velocity"; the authors find exit angle independent of nose. Retained speed was not measured there.

**Steel spheres on aluminium (ADA125330, Figs 7 and 13, pp.23, 29; 6.35 mm on 12.7 mm, BHN 143–163, t/d 2; flash X-ray). BY EYE ±15 m/s, ±3°; V_BL about 1,040 m/s from the 0° curve, BY EYE. Measured exit speed and angle; components DERIVED. A sphere and soft plate: not a fixture for a pointed steel round.**

| Entry | v m/s | Exit speed | Exit from normal | Exit / v | Tangential kept | Exit / (v sin θ) | Restitution |
|---|---|---|---|---|---|---|---|
| 60° | 235 | 165 | 70° | 0.70 | 0.76 | 0.81 | 0.48 |
| 60° | 400 | 250 | 70° | 0.62 | 0.68 | 0.72 | 0.43 |
| 60° | 480 | 290 | 68° | 0.60 | 0.65 | 0.70 | 0.45 |
| 60° | 660 | 335 | 62° | 0.51 | 0.52 | 0.59 | 0.48 |
| 60° | 840 | 335 | 58° | 0.40 | 0.39 | 0.46 | 0.42 |
| 60° | 1,000 | 310 | 52° | 0.31 | 0.28 | 0.36 | 0.38 |
| 60° | 1,110 | 255 | 47° | 0.23 | 0.19 | 0.27 | 0.31 |
| 45° | 200 | 105 | 58° | 0.53 | 0.63 | 0.74 | 0.39 |
| 45° | 310 | 155 | 58° | 0.50 | 0.60 | 0.71 | 0.37 |
| 45° | 400 | 170 | 55° | 0.42 | 0.49 | 0.60 | 0.34 |
| 45° | 540 | 165 | 47° | 0.31 | 0.32 | 0.43 | 0.29 |
| 45° | 660 | 145 | 43° | 0.22 | 0.21 | 0.31 | 0.23 |
| 45° | 790 | 140 | 18° | 0.18 | 0.08 | 0.25 | 0.24 |
| 45° | 890 | 85 | 18° | 0.10 | 0.04 | 0.14 | 0.13 |

Text p.21: "The exit angle for striking speeds well above the ballistic limit approaches asymptotically to an angle equal to the obliquity"; "the effect of increasing the impact angle is to increase the values of the ricochet speed and the ricochet limit."

**Other.** Hershey (CLAIMED, theory): less than 4 % speed lost at the upper limit, e/d 0.03. Spheres off water and sand (repo note R6): 0.81 × grazing entry angle.

**Verdict.** Direction: "slide along the face" holds for rigid pointed rounds below about 0.6 V50, with e ≈ 0.15 rather than 0: 5° off the face at 60° entry, 15° at 30° (DERIVED), hardly visible. Near V50 the exit swings to the entry angle, in rounds and spheres alike. Speed: v sin θ is an upper bound, measured only on spheres in soft plate. **No change now:** keep e = 0 and v sin θ as a `StandIn` naming the missing law. No simple form is earned from one sphere chart; do not fit one.

## 8. Fixture rows

t/d = e/d. q_A: law A at preset ρ (uncapped 6,000, capped and hollow 8,000). ρ_min: law A. q_B, θc_B: law B at ρ 6,000. All q, ρ_min, θc DERIVED; v MEASURED for shots, DERIVED from MEASURED F and mass for F rows. Plate TS in ksi. Each F row is a limit: a perforation at the stated speed, so it needs q < 1.

| Round | Construction | Plate | v m/s | θ | t/d | Outcome | q_A | ρ_min | q_B | θc_B | Source |
|---|---|---|---|---|---|---|---|---|---|---|---|
| M79 | uncapped solid | 130 | 402 | 60.0 | 0.244 | 1 in, broke, nose through | 2.02 | 12,095 | 0.79 | 63.3 | NPG 16-43 p.11 |
| M79 | | 130 | 416 | 59.9 | 0.244 | 3/4 in, broke | 1.87 | 11,230 | 0.73 | 64.1 | p.11 |
| M79 | | 130 | 431 | 59.8 | 0.244 | perforated | 1.73 | 10,384 | 0.67 | 65.0 | p.11 |
| M79 | | 130 | 431 | 60.0 | 0.244 | perforated, VR 1,037 fps | 1.75 | 10,480 | 0.68 | 65.0 | p.11 |
| M79 | | 130 | 192 | 29.8 | 0.243 | perforated, VR 45 | 1.68 | 10,078 | 0.65 | 38.4 | p.10 |
| M79 | | 123 | 408 | 30.0 | 0.647 | perforated, VR 160 | 1.00 | 5,986 | 0.63 | 39.2 | p.13 |
| M79 | | 102 | 289 | 44.0 | 0.370 | F 34,700 | 2.29 | 13,767 | 1.10 | 42.1 | NPG 1125 T.III |
| M79 | | 102 | 330 | 49.0 | 0.372 | F 36,100 | 2.30 | 13,820 | 1.11 | 47.1 | T.III |
| M79 | | 104 | 381 | 52.5 | 0.370 | F 38,800 | 2.10 | 12,576 | 1.01 | 52.4 | T.III |
| M79 | | 115 | 256 | 60.0 | 0.164 | F 32,100 | 3.34 | 20,057 | 1.07 | 59.1 | T.III |
| M79 | | 109 | 200 | 46.5 | 0.236 | F 28,800 | 3.48 | 20,870 | 1.33 | 40.9 | T.III |
| M79 | | 106 | 266 | 54.0 | 0.235 | F 32,800 | 2.99 | 17,945 | 1.14 | 51.7 | T.III |
| M79 | | 110 | 365 | 61.0 | 0.236 | F 37,000 | 2.54 | 15,246 | 0.97 | 61.4 | T.III |
| M79 | | 125 | 188 | 45.0 | 0.203 | F 30,000 | 3.12 | 18,749 | 1.11 | 43.0 | T.III |
| M79 | | 125 | 229 | 52.8 | 0.203 | F 31,200 | 3.25 | 19,527 | 1.16 | 50.3 | T.III |
| M79 | | 125 | 307 | 60.0 | 0.203 | F 34,600 | 2.88 | 17,263 | 1.02 | 59.7 | T.III |
| M79 | | 130 | 424 | 60.0 | 0.245 | F 43,500 | 1.82 | 10,922 | 0.71 | 64.5 | T.III |
| 90 mm T33E7 | AP-T, windshield | BHN 321 | 857 | 60.0 | 0.847 | V50 (2,813 fps), "break-up and rolling" | 1.54 | 9,214 | 1.11 | 58.4 | AD151466 p.4 |
| M61 DA-301 | capped, 15 %, blunt angular, 14.2 lb | 130 | 362 | 59.8 | 0.245 | 1 in, nose through | 1.85 | 14,791 | 0.96 | 60.4 | NPG 16-43 p.11 |
| M61 DA-301 | | 130 | 378 | 60.1 | 0.243 | perforated, VR 484 | 1.71 | 13,667 | 0.89 | 61.8 | p.11 |
| M61 DA-301 | | 130 | 204 | 29.8 | 0.243 | perforated, VR 169 | 1.11 | 8,894 | 0.58 | 40.9 | p.10 |
| M61 DA-301 | | 123 | 469 | 30.0 | 0.647 | perforated, VR 81 | 0.56 | 4,519 | 0.48 | 44.7 | p.13 |
| Type A-1 Heavy DA-304 | capped, 16 %, pointed, 14.65 lb | 130 | 420 | 59.1 | 0.243 | perforated, VR 864 | 1.29 | 10,337 | 0.67 | 64.5 | p.11 |
| Type A-1 Heavy DA-304 | | 130 | 198 | 30.2 | 0.241 | perforated | 1.19 | 9,534 | 0.62 | 40.0 | p.10 |
| Type A-1 Regular 267805 | capped, 12 %, 13.9 lb | 130 | 418 | 60.0 | 0.244 | perforated | 1.40 | 11,161 | 0.73 | 64.3 | p.11 |
| Type A-1 Light DA-303 | capped, 10 %, 13.9 lb on this plate | 130 | 422 | 60.1 | 0.242 | perforated | 1.37 | 10,940 | 0.71 | 64.6 | p.11 |
| M51B2 model DA-302 hard | capped, 11 %, BHN 485, 13.6 lb | 130 | 440 | 59.9 | 0.245 | perforated | 1.26 | 10,047 | 0.65 | 65.4 | p.11 |
| M51B2 model DA-302 hard | | 130 | 192 | 29.7 | 0.243 | perforated | 1.25 | 9,965 | 0.65 | 38.5 | p.10 |
| M51B2 model DA-302 hard | | 123 | 469 | 30.0 | 0.647 | perforated, VR 612 | 0.56 | 4,519 | 0.48 | 44.7 | p.13 |
| M51B2 model DA-302 soft | capped, 11 %, BHN 210 | 130 | 197 | 30.3 | 0.241 | 1/8 in, **ricochet** | 1.21 | 9,701 | 0.63 | 39.8 | p.10 |
| M51B2 model DA-302 soft | | 130 | 215 | 30.0 | 0.241 | 1/4 in, **ricochet** | 1.00 | 7,990 | 0.52 | 43.3 | p.10 |
| M51B2 model DA-302 soft | | 123 | 461 | 30.0 | 0.647 | perforated, VR 318 | 0.58 | 4,675 | 0.49 | 44.1 | p.13 |
| Mk 11 model 3001 | capped, 6 %, 14.5 lb | 130 | 412 | 59.9 | 0.244 | 1-3/4 in, nose through | 1.43 | 11,446 | 0.74 | 63.9 | p.11 |
| Mk 11 model 3001 | | 130 | 196 | 30.5 | 0.242 | perforated, VR 30 | 1.25 | 9,964 | 0.64 | 39.4 | p.10 |
| Mk 11 model 3001 | | 123 | 455 | 30.0 | 0.647 | perforated, VR 724 | 0.60 | 4,814 | 0.51 | 43.5 | p.13 |
| M62 BS | capped, 13.9 %, notched, windshield | 108 | 140 | 40 | 0.163 | F 27,000 | 2.63 | 21,042 | 1.12 | 37.8 | NPG 1211 p.51 |
| M62 BS | | 108 | 187 | 50 | 0.163 | F 30,300 | 2.49 | 19,912 | 1.06 | 49.0 | p.51 |
| M62 BS | | 108 | 237 | 60 | 0.163 | F 29,800 | 2.91 | 23,272 | 1.24 | 56.9 | p.51 |
| M62 BS | | 111 | 234 | 30 | 0.332 | F 35,700 | 1.17 | 9,362 | 0.71 | 36.9 | p.54 |
| M62 BS | | 113 | 286 | 40 | 0.337 | F 38,400 | 1.30 | 10,403 | 0.79 | 44.5 | p.54 |
| M62 BS | | 112 | 347 | 50 | 0.336 | F 39,100 | 1.49 | 11,958 | 0.91 | 51.6 | p.54 |
| M62 BS | | 113 | 501 | 60 | 0.336 | F 43,900 | 1.34 | 10,724 | 0.82 | 62.7 | p.54 |
| M62 BS | | 112 | 466 | 30 | 0.654 | F 50,700 | 0.58 | 4,642 | 0.49 | 44.1 | p.53 |
| M62 BS | | 112 | 502 | 40 | 0.653 | F 48,400 | 0.82 | 6,548 | 0.70 | 47.0 | p.53 |
| M62 BS | | 112 | 599 | 45 | 0.654 | F 53,200 | 0.75 | 5,962 | 0.63 | 53.2 | p.53 |
| M62 BS | | 113 | 609 | 50 | 0.652 | F ≤ 49,300, broken | 0.94 | 7,521 | 0.80 | 53.9 | p.53 |
| M62 BS | | 96 | ≥ 840 | 60 | 0.649 | F ≥ 53,000, not perforated | 0.92 | 7,360 | 0.78 | 63.4 | p.53 |
| M62 BS | | 108 | 594 | 30 | 1.011 | F 52,000 | 0.55 | 4,413 | 0.58 | 40.8 | p.50 |
| M62 BS | | 107 | 673 | 40 | 1.012 | F 52,100 | 0.71 | 5,651 | 0.75 | 45.7 | p.50 |
| M62 BS | | 107 | 769 | 45 | 1.010 | F 55,000 | 0.70 | 5,578 | 0.74 | 50.6 | p.50 |
| M62 BS | | 111 | 743 | 30 | 1.40 | F 55,300 | 0.49 | 3,902 | 0.61 | 40.1 | p.56 |
| 37 mm M51 | capped, 14 % of core, 0.835 kg | BHN 260 | 890 | 30 | 1.79 | perforated (chart) | 0.39 | 3,123 | 0.52 | 43.1 | survey §4, NDRC Fig. 18 |
| 37 mm M51 | | BHN 260 | 890 | 45 | 1.28 | perforated (chart) | 0.66 | 5,278 | 0.79 | 49.5 | same |
| 37 mm M51 | | BHN 260 | 890 | 55 | 0.93 | perforated (chart) | 0.84 | 6,751 | 0.86 | 57.4 | same |
| 37 mm M51 | | BHN 260 | 890 | 60 | 0.75 | perforated (chart; to 62°) | 0.95 | 7,575 | 0.86 | 62.0 | same |
| 6 in Comm Mk 27-7 | hooded, hollow | STS, Class B | 219 | 75 | 0.04 | F 27,000 BY EYE | 3.46 | 27,667 | 0.73 | 77.2 | NPG 1125 Fig. 2 |
| 6 in Comm Mk 27-7 | | | 269 | 75 | 0.06 | F 27,000 BY EYE | 3.46 | 27,667 | 0.89 | 75.8 | Fig. 2 |
| 6 in Comm Mk 27-7 | | | 360 | 75 | 0.10 | F 28,000 BY EYE | 3.22 | 25,726 | 1.07 | 74.5 | Fig. 2 |
| 6 in Comm Mk 27-7 | | | 609 | 75 | 0.20 | F 33,500 BY EYE | 2.25 | 17,972 | 1.06 | 74.6 | Fig. 2 |

**Second block.** Same columns. Mk 29-2 and the flat Mk 29 use 13 lb, ASSUMED. Flat Mk 29 is listed at ρ 6,000, and counted in no class because flat is not a shipped nose.

| Round | Construction | Plate | v m/s | θ | t/d | Outcome | q_A | ρ_min | q_B | θc_B | Source |
|---|---|---|---|---|---|---|---|---|---|---|---|
| M79 | uncapped solid | 118 | 203 | 45 | 0.236 | F 30,100 | 3.10 | 18,625 | 1.19 | 41.6 | NPG 1125 T.III |
| M79 | | 118 | 266 | 53 | 0.236 | F 33,500 | 2.83 | 16,982 | 1.08 | 51.6 | T.III |
| M79 | | 122 | 216 | 45 | 0.244 | F 31,400 | 2.85 | 17,115 | 1.11 | 43.0 | T.III |
| M79 | | 107 | 289 | 60 | 0.200 | F 32,900 | 3.18 | 19,093 | 1.12 | 58.3 | T.III |
| M79 | | 118 | 184 | 45 | 0.206 | F 29,200 | 3.30 | 19,791 | 1.18 | 41.8 | T.III |
| M79 | | 118 | 307 | 60 | 0.202 | F 34,700 | 2.86 | 17,164 | 1.01 | 59.8 | T.III |
| M79 | | 119 | 418 | 60 | 0.264 | F < 41,300 (a bite at this speed) | 2.02 | 12,116 | 0.82 | 62.7 | T.III |
| M79 | | 111 | 301 | 60.2 | 0.202 | F > 33,800 (no perforation yet; not counted) | 3.02 | 18,126 | 1.07 | 59.2 | T.III |
| M62 BS | capped | 108 | 125 | 30 | 0.163 | F 27,300 | 2.00 | 16,010 | 0.85 | 33.3 | NPG 1211 p.50 |
| M62 BS | | 112 | 447 | 20 | 0.656 | F 52,700 | 0.37 | 2,939 | 0.31 | 42.4 | p.53 |
| M62 BS | | 108 | 571 | 20 | 1.014 | F 54,200 | 0.35 | 2,778 | 0.37 | 39.2 | p.50 |
| M62 BS | | 111 | 704 | 20 | 1.386 | F 57,100 | 0.31 | 2,503 | 0.39 | 38.2 | p.56 |
| M62 BS | | 107 | 461 | 29.8 | 0.675 | F 49,500 | 0.61 | 4,840 | 0.52 | 42.8 | p.56 |
| M62 BS | | 107 | 551 | 45 | 0.673 | F 48,300 | 0.90 | 7,233 | 0.78 | 49.6 | p.56 |
| Mk 29-2 | capped | 115 | 515 | 30 | 0.659 | F 52,000 | 0.48 | 3,824 | 0.41 | 47.7 | p.52 |
| Mk 29-2 | | 115 | 639 | 45 | 0.663 | F 52,500 | 0.66 | 5,306 | 0.57 | 55.0 | p.52 |
| Mk 29-2 | | 109 | 699 | 30 | 1.011 | F 57,000 | 0.40 | 3,183 | 0.42 | 47.1 | p.50 |
| Mk 29-2 | | 109 | 779 | 40 | 1.011 | F 56,200 | 0.53 | 4,209 | 0.56 | 51.1 | p.50 |
| Type A-1 Regular 267805 | capped | 130 | 380 | 60.0 | 0.242 | 5/8 in, no ricochet | 1.68 | 13,441 | 0.87 | 61.9 | NPG 16-43 p.11 |
| Type A-1 Light DA-303 | capped | 130 | 414 | 60.0 | 0.244 | 1-1/2 in, no ricochet | 1.42 | 11,392 | 0.74 | 64.0 | p.11 |
| Type A-1 Heavy DA-304 | capped | 130 | 197 | 29.6 | 0.243 | 2 in, no ricochet | 1.18 | 9,464 | 0.61 | 39.4 | p.10 |
| Type A-1 Heavy DA-304 | | 130 | 200 | 29.7 | 0.243 | 1-7/8 in, no ricochet | 1.15 | 9,231 | 0.60 | 40.0 | p.10 |
| Type A-1 Regular 267805 | | 130 | 198 | 30.2 | 0.241 | 1-3/4 in, no ricochet | 1.19 | 9,505 | 0.61 | 40.0 | p.10 |
| M51B2 model DA-302 hard | capped | 130 | 190 | 30.6 | 0.241 | 1-3/4 in, no ricochet | 1.32 | 10,575 | 0.68 | 38.3 | p.10 |
| M61 DA-301 | capped | 123 | 438 | 30.0 | 0.647 | 2-1/4 in, no ricochet | 0.65 | 5,183 | 0.55 | 42.1 | p.13 |
| M51B2 model DA-302 soft | capped | 123 | 442 | 30.0 | 0.647 | 2-3/4 in, no ricochet (soft: not counted) | 0.64 | 5,090 | 0.54 | 42.4 | p.13 |
| Mk 29 flat | blunt, windshield | 118 | 244 | 30 | 0.362 | F 33,200 | 1.56 | 9,382 | 0.74 | 36.0 | NPG 1211 p.50 |
| Mk 29 flat | | 118 | 281 | 45 | 0.357 | F 31,500 | 2.46 | 14,739 | 1.16 | 42.2 | p.50 |
| Mk 29 flat | | 118 | 419 | 60.2 | 0.361 | F 32,800 | 2.78 | 16,682 | 1.32 | 56.1 | p.50 |
| Mk 29 flat | | 115 | 422 | 30 | 0.660 | F 42,600 | 0.95 | 5,698 | 0.61 | 40.0 | p.52 |
| Mk 29 flat | | 115 | 604 | 45 | 0.662 | F 49,700 | 0.99 | 5,921 | 0.63 | 53.2 | p.53 |

Tally, both blocks (DERIVED). Capped, excluding soft caps and the ≥ 840 m/s bound: 50 rows; law A at 8,000 fails 25, all at t/d ≤ 0.34; law B at 6,000 fails 3. Uncapped, excluding the F > 33,800 row: 25 rows; law B at 6,000 fails 16, worst −5.6°. Flat Mk 29: law B fails 2 of 5, by 2.8° and 4.1°.

Not run, MEASURED F (NPG 1125 T.III, Type A − cap, mass not printed, TS ksi): 123, 60.5°, 0.064, 31,000; 117, 45°, 0.408, 35,000; 50°, 0.411, 37,300; 123, 45°, 0.209, 29,000; 122, 52°, 0.210, 34,000; 124, 59°, 0.215, > 34,000; 119, 44.5°, 0.263, 31,500; 60°, 0.261, 37,000; 59°, 0.262, 37,000; 105, 45°, 0.124, 32,500; 145, 45°, 0.130, 32,500; 145, 44°, 0.173, 33,100.

Also not run: NPG 16-43 perforations at higher speed than the rows above, which test the law less hard; NPG 1211 pp.51–52 and 56, which repeat the cap series (those on p.56 read from an inverted page and unverified); M62 at TS 115,000 (20°, 0.663, 51,500; 30.5°, 49,600); Mk 29-2 at TS 123,000 (29°, 0.661, 53,500); and R2–R5 in `ricochet-bite-law-2026-11-02.md` §2.

## 9. Unsettled, and what would settle each

1. **Capped against uncapped on thick plate (t/d ≥ 1.6, θ ≥ 40°, ≥ 600 m/s).** No row at all. It is the only thing that could justify a capped density above steel's 6,000. Settle with a capped/uncapped pair at those conditions: NPG e/d ≥ 1.5 limits, or British Ordnance Board proceedings (NDRC refs 46, 48).
2. **Saturation on armour steel, and R = a against H.** Settle with a rigid hard core on RHA at t/d 2–4, angle raised to ricochet.
3. **The thin-plate exponent.** My F₀ are by eye. Settle with a numeric M79 0° series at e/d 0.1–0.5 (NPG 2-46 or the source reports behind Fig. 3), not reached.
4. **Gupta & Madhu 1992's own tables,** especially 16 mm at 45°, and MS3's yield. Read IJIE 12 (1992) 333; paywalled.
5. **Vayig & Rosenberg 2023's form.** IJIE 178, 104621; abstract only.
6. **Tungsten ρ and β.** Settle with fired WC-core rows at ≥ 60°, with an outcome and a core size: OCO "Armor Perforation Curves" or Watertown firing records.
7. **Blunt β.** Settle with fired BR-365 against BR-365K at 30° and 60° (NII-48), or a Western APBC series with its own 0° row.
8. **Retained speed on steel.** Settle with flash-X-ray rows of a steel round off steel. Also ADA030268 and AD0479824 below.
9. **Soft-cap ricochet at 197–215 m/s,** unexplained by any form here. Settle with other soft-cap rows at ≤ 30°.
10. **Inputs:** M79 length, T33E7 mass and drawing 75-2-545, M62 BS mass.
11. **Capability law on thin plate** (§5): outside this lane. A potency law without F's fall-off reads V_BL ×1.2–2.6 high below one calibre.

## Sources

Read directly (DTIC scans via archive.org; apps.dtic.mil does not resolve here):

- NPG Report 16-43, *Effect of Cap Design on 3-inch Projectile Performance*, 14 Dec 1943, AD0310017: p.ii–iii, 2, 4–7, appendix pp.9–14.
- Hershey, NPG Report 1125, *Ballistic Summary Part IV*, 23 May 1955, AD0063448: pp.8–9, Tables I, III, Figs 2, 3.
- NPG Report 1211, *Ballistic Summary Part III*, 1955 (distribution statement dated 6 June), AD0063985: pp.16–18, 28–29, Table XIV pp.50–59.
- Hershey, NPG Report 1041, *Analytical Summary Part VI*, AD0063447: pp.37–38, Table VII.
- Ipson, Recht & Schmeling, NWC TP 5607, Dec 1973, AD0916925: pp.5, 6, 14, 29, 30.
- *Ricochet and Penetration of Steel Spheres Impacting Aluminum Targets*, ADA125330: Figs 7, 13, text p.21.
- APG *Plate Performance Test of the T33E7*, Oct 1957, AD151466: pp.3–4 (poor scan; 2,813 and 2,785 fps read from the page image, not the OCR).
- TM 9-1904 p.448.
- Raguraman, Deb & Gupta, LAJSS 5 (2008) 225–235: Table 1, p.231: https://lajss.org/index.php/LAJSS/article/download/160/151/167

Reached in part or second-hand:

- Vayig & Rosenberg 2023: abstract only: https://cris.bgu.ac.il/en/publications/on-the-ricochet-of-sharp-nosed-rigid-projectiles-from-metallic-pl/
- Gupta & Madhu 1992: abstract, and the test values LAJSS reprints.
- Madhu, Bhat & Gupta 2003: values per the repo note; DSJ PDFs do not resolve here.
- Report M.7000 A/11 No.1, via a forum thread (CLAIMED, about 3 % uncapped penalty at 40–60°, graphs not read): not used.

Given up, after attempts: ADA030268 (archive.org timed out on four resumes, file unusable); AD0479824 (archive.org returned an error page); s40997 (no PDF). Downloaded but unread: AD0781841, AD0310002, ADA955285 past its title.
