# Round survey, four guns: construction, penetration against range and obliquity, and normalization (2026-11-02)

**Subject:** the game's damage model. How a round's construction decides whether it skids, bites, shatters or penetrates a plate, tested against twelve real rounds from four guns and three eras. Game state only.

## TLDR

1. **One schema? Almost.** The field set describes all twelve rounds, but five facts are missing:
   - a **hit length** (what meets the plate, windshield excluded), which ρ_e needs: M61, M62, 17-pdr APCBC, Pzgr. 39, BR-365;
   - a **blunted-nose** `NoseShape`: BR-365 (and NDRC's rounded-tip noses);
   - a **nested core** in a penetrator body: M735;
   - a **jacket or sheath** on a sub-calibre core: 17-pdr APDS, L28;
   - **cap hardness**, in the substances table: M61's hard cap against M51/M62's soft ones.
2. **Finer than the type? Yes.** Under the pressure-balance law on thick RHA, the full-calibre capped rounds skid at **23–45°** at the muzzle (DERIVED). Muzzle velocity drives most of that, but at equal speed the hit length still moves θc by **4–6°**. Cap mass (14–27 %) and hardness set different shatter thresholds inside "APCBC". The windshield's crh barely matters (2 % of 2,000-yd velocity); whether there is a windshield does (13 points of retained velocity and 6 m of drop for BR-365).
3. **The law needs a cap term.** With the bare law, the capped 37 mm M51 should skid at 55–62°, where NDRC Fig. 18 shows it perforating (DERIVED: q = 1.03–1.15).
4. **Normalization, direction.** Every WWII service round measured at service speed **loses more plate than line of sight predicts**: an effective turn *away* from the normal (β > 0), never the folk gain toward it. By construction (DERIVED from MEASURED ratios):
   - capped steel: +6 to +10°;
   - uncapped steel: +4° at low speed rising to +12° at the muzzle;
   - tungsten-carbide cores: +6° capped, +16° uncapped;
   - blunt APBC: +1 to +2° ([S], calculated);
   - long rods are the one class better than line of sight: −3 to −6°.
5. **The excess is mostly deformation, not geometry.** Hershey's 170 limits with *undeformed* 3-in shot follow line of sight below 30° and rise only past about 45° (NPG 2-46). So the service tables' +6 to +16° belongs to the shatter/deformation gate, and a geometric bend matters only near θc.
6. **Size (DERIVED).** A round with 120 mm of normal penetration defeats **104 mm at 30° and 60 mm at 60°** under line of sight; the measured class excess gives 85–97 and 31–49 mm.
7. **Critical? Yes, at slopes of 45° and steeper**, and in ranking tungsten rounds against capped steel: the sub-calibre lead falls from 60 % at normal to 22 % at 55° (NDRC p.190). Minor at 30°; negligible for long rods.
8. **Proposed bend law**, meeting all three §4 constraints: **β = (90° − θ)·qⁿ** away from the normal. q = R_eff sin θ / (ρ_e v² cos²θ) is the contact law's own pressure ratio; n ≥ 4 is Unknown (n ≥ 6.5 by Fig. 18 with the bare q). The bite-regime excess goes to the deformation gate.
9. **Source quality.** US data are primary (NDRC 2C3a and Fig. 18, HVAP45, TM 9-1901/1904, TM 43-0001-28). Soviet and British penetration figures are secondary, the Soviet ones calculated; 105 mm has no 0° anchor. Some 30° columns were computed from 0° by rule, which makes "normalization" read from them circular.

## 1. Labels, criteria, decisions

**Labels.** **MEASURED**: printed in a cited source ("chart" marks a reading, with its error). **DERIVED**: my arithmetic, inputs shown, run in a scratch script. **RECALLED**: unverified memory. **[S]**: secondary. Unit conversions are DERIVED. θ is obliquity from the normal; Soviet tables give the *meeting angle* (90° = normal, 60° = θ 30°).

**Criteria (convert nothing silently).**

- **US Army:** "CP: complete penetration, light visible through a crack" (US ETO memo of 24 May 1944, [S] transcription). Data Sheet 2C3a (Ordnance Department source) prints none; I read it as Army.
- **US Navy:** "the projectile just passes completely through the plate (Navy Limit)" (NDRC Data Sheet 2C3).
- **HVAP45:** none stated.
- **British:** no primary read. [S]: at least 20 % of the projectile through, 50 % of the time, changed in 1943. The deMarre formula, with a secant obliquity term, "is still used by the British" (NPG 2-46 p.2).
- **Soviet:** [S] Potapov: "theoretical and … calculated by method of ARTKOM", penetrated "if at least 75 % of a projectile's fragments" are behind the plate; CP = "80 % probability".

**Decisions.**

1. The 75 mm M3 had no service HVAP, so I used the **76 mm M1's M93** (M4(76)W Sherman) and its sibling **M62**, which HVAP45 compares on one gun.
2. 17-pdr AP and APC: no primary source; construction is RECALLED, penetration [S] calculated.
3. 105 mm: **L28A1** (TM 43: the M392 is the UK L36A1, and the L28A1 is "similar … except for its primer"), **M735** (no L64 data found), and **M393A2 HEP** for L35 HESH (equivalence RECALLED).
4. **Effective bend:** t(θ)/t(0) = cos(θ + β) at equal striking velocity, t the normal thickness defeated. β > 0 is away from the normal, the sign §4's limit (β → 90° − θ at θc) implies. The game's line-of-sight march has β = 0.

## 2. The four guns

### 2.1 US 75 mm M3 (and the 76 mm M1 for HVAP)

| Round | Construction (field set) | MV | Penetration (MEASURED unless noted) |
|---|---|---|---|
| **M72 AP shot** | Monobloc "solid hardened steel shot", "without armor-piercing cap or ballistic cap", tracer cavity only; nose **1.64 crh** (TM9-1901 par.92) or **1.5 crh** (TM9-1904 p.414): sources disagree. Boat-tail 9°15′. **Length 9.21 in** (L/D 3.12, DERIVED). Mass **6.32 kg** (DERIVED: complete round 18.80 lb − 4.86 lb of case, 1.90 lb charge and primer; TM9-1901, TM9-1904). Flight nose = hit nose; no carrier, filler or fuze. | 2,030 ft/s = 619 m/s | 1,000 yd, 0°: **3.1 in homogeneous, 2.6 in face-hardened** (TM9-1901 p.103). 2C3a (chart ±0.05 in), homogeneous BHN 250–300: **0°** 4.25 / 3.09 / 2.12 in; **30°** 3.14 / 2.44 / 1.76 in, at 100 / 1,000 / 2,000 yd. |
| **M61 APC** (APCBC in British usage) | Hardened forged steel body, **0.15 lb Explosive D**, BD fuze M66A1 (delay). AP cap "centrally hardened and peripherally toughened", crimped and soldered on. Die-cast aluminium windshield (TM9-1904 p.412). False ogive **7.17 crh** (TM9-1901) or 7.5 (TM9-1904). Length **13.22 in with windshield** (OCR; L/D 4.48, DERIVED). **14.96 lb = 6.79 kg**. Cap mass and cap nose: Unknown. | 619 m/s | 1,000 yd, 0°: **2.8 in homogeneous, 3.4 in face-hardened** (TM9-1901 p.102). 2C3a: **0°** 3.54 / 2.98 / 2.41; **30°** 2.84 / 2.38 / 1.96. The 2C3a and TM9-1901 figures at 1,000 yd disagree by 0.18 in. |
| **M93 HVAP** (76 mm) | "Aluminum body with an adjustable nose plug, a tungsten core, a steel base, a windshield, and a tracer" (HVAP45 p.1). **9.35 lb = 4.24 kg**. Core about **1.53 in** across, W/d³ ≈ 1.03, so about **1.67 kg** (DERIVED from the NDRC 2C6 nomogram, chart). Carrier kept. | 3,400 ft/s = 1,036 m/s | **20°**: 8.6 / 7.4 / 6.3 / 5.3 / 4.5 / 3.7 / 3.0 in; **30°**: 7.2 / 6.2 / 5.3 / 4.5 / 3.8 / 3.2 / 2.6 in, at 0–3,000 yd in 500-yd steps (HVAP45 p.3, homogeneous). |
| **M62 APCBC** (76 mm, the M93's sibling) | Hard steel body, soft steel cap "sweated on", aluminium windshield 7.05 crh, 7.00 kg (context note). | 2,600 ft/s = 792 m/s | **20°**: 4.4 / 4.2 / 3.9 / 3.7 / 3.4 / 3.1 / 2.8; **30°**: 3.8 / 3.7 / 3.5 / 3.3 / 3.0 / 2.8 / 2.6 (HVAP45 p.3). 2C3a **0°**: 5.21 / 4.48 / 3.66 at 100 / 1,000 / 2,000 yd (chart). |

MEASURED behaviour: M72 has "a greater tendency to ricochet on angled impacts and no protection against breaking of the nose on impact" (TM9-1904 p.415). The M61's cap "reduces greatly the possibilities of ricochet on highly angled impacts" (p.412).

### 2.2 Soviet 85 mm (D-5T / ZiS-S-53)

| Round | Construction | MV | Penetration, mm ([S] Potapov, ARTKOM-calculated, CP) |
|---|---|---|---|
| **BR-365** | "APBC, blunt tip" (1970 D-44 table, via Tank Archives [S]); blunt head under a ballistic cap. **9.2 kg**, length **289 mm with cap**, fuze MD-7 (Shirokorad via armedconflicts [S]). The listed 0.741 kg filler is implausible (8 % of mass against about 1 % for M61 and Pzgr. 39), so it is Unknown. | 792 m/s [S]; 800 [S] | **θ = 0°**: 105 / 100 / 92 / 85; **θ = 30°**: 90 / 85 / 78 / 72, at 500 / 1,000 / 1,500 / 2,000 m. |
| **BR-365K** | Sharp-tipped AP shell, uncapped [S]. **9.34 kg**, fuze MD-8 [S]. | 792 / 800 [S] | 0°: 108 / 102 / 90 / 82; 30°: 90 / 78 / 72 / 66. |
| **BR-365P** | APCR [S]; core Unknown; about 5.0–5.35 kg [S, conflicting]. | 1,050 m/s [S] | 0°: 140 / 118 (500 / 1,000 m); 30°: 100 / 85. |

These are calculated, not fired; their value is the *by-construction ordering* of the obliquity cost (§5). It agrees with GAU KA, 1942 (CAMD RF 81-12042-85, Tank Archives translation [S]): sharp tips "work better against homogeneous armour at 0 degrees", but lose to blunt ones as the angle grows, and at 30–45° the sharp shell "penetrates the armour 30–40 mm away from the place of impact". GAU KA wanted a blunted part "about 0.7 calibers long".

### 2.3 British 17-pounder

| Round | Construction | MV | Penetration |
|---|---|---|---|
| **AP** (uncapped shot) | Monobloc (RECALLED); no primary source read. | — | 0°: 175 / 147 mm at 500 / 1,000 m (Bird & Livingston p.60, **calculated** [S]). |
| **APC** | "Basic AP shell … not used in the Firefly" (Fletcher [S]). | — | none found |
| **APCBC** (marks 1/3; "Mk 8" not found as sourced) | **17 lb = 7.71 kg** (RAC Pamphlet No. 7, 1952, via Wikipedia [S]). Cap, windshield crh and filler: Unknown. | 2,950 ft/s = 899 m/s [S]; 2,900 [S] | **30°**: perforated ("PTP") **120 mm** homogeneous at **800 yd**, where the 90 mm M82 failed (Shoeburyness, 23 May 1944, US ETO memo, [S] transcription). 30°: 127 mm at 600 yd and **120 mm at 1,000 yd** (WO 219/2806 Appx G, via Osprey [S]). 30°: 120 mm "at 1000" (Lulworth table of 31 Mar 1944, Tank Archives [S]). 0°: 163 / 150 mm at 500 / 1,000 m (Bird & Livingston, calculated [S]). |
| **APDS Mk 1** | Launched mass **7 lb 11 oz = 3.49 kg** [S]. Tungsten-carbide core in a light steel sheath, four-piece light-alloy sabot (ra39-45 [S]). Core about **1.53 in, W/d³ ≈ 1.13, so about 4.05 lb (1.84 kg)** (DERIVED from the NDRC 2C6 nomogram's "British 17 pdr D.S." marks, chart). Carrier discarded. | 3,950 ft/s = 1,204 m/s [S] | 30°: **187 mm at 500 yd** (Bovington document via Wikipedia [S], "citation needed") **against 231 mm at 1,000 yd** (Jane's 1981–82 [S]): these contradict. 0°: 256 / 233 mm at 500 / 1,000 m (Bird & Livingston, calculated [S]). |

No British 0°/30° pair from one source was found, so no 17-pdr β is derived. Mixing sources (120 / 150) gives 0.80, which only echoes the inputs.

### 2.4 British 105 mm L7 / US M68

| Round | Construction | MV | Penetration |
|---|---|---|---|
| **L28A1 APDS** (≈ M392) | "Sheathed tungsten carbide core with tracer and a sabot … spin stabilized"; the sabot discards on "setback, centrifugal, and air pressure forces"; complete round 41 lb (TM43 p.2-71). Projectile 5.84 kg [S]. Core mass and size: Unknown. | **1,478 m/s** (TM43) | **120 mm at 60°** at 914 m (Wikipedia [S]) or 1,000 m (blog [S]). No 0° figure, so no β. L52: 130 mm at 60° at 1,000 m [S] against 120 mm at 60° at 1,800 m [S]: these conflict. |
| **M735 APFSDS** | "Steel-nickel body, which houses a tungsten core … fitted with an aluminum windshield and fin assembly"; aluminium sabot in three 120° sections; the sub-projectile "does not pick up the high rotation rate" (TM43 p.2-101). Rod mass, length and MV: not in TM43. | Unknown (1,501 m/s RECALLED) | Unknown |
| **M393A2 HEP** (≈ L35 HESH) | "Thin-walled cylinder with a relatively short ogive and a flat base"; **Comp A3, 6.6 lb**; BD fuze M578 (TM43 p.2-81). | 731.5 m/s (TM43, per context note) | Does not apply: chemical-energy potency is "constant regardless of range" (FM17-12). |

## 3. Test 1: one schema?

The field set is mass, caliber, length, flight_nose, hit_nose, body, cap, penetrator (mass, caliber, length, substance), carrier, burst/fuze. It covers M72, M93, BR-365K, BR-365P, the 17-pdr AP and M393 outright. Gaps, each with the round that needs it:

| Missing fact | Needed by | Why a law reads it |
|---|---|---|
| **Hit length**: what meets the plate, windshield excluded (or the windshield length) | M61, M62, 17-pdr APCBC, Pzgr. 39, BR-365 | ρ_e = m/(A·L) in the contact law. For M61, the full 13.22 in gives ρ_e = 4,574; an ASSUMED 3.3-calibre hit length gives 6,182, a **6° shift in θc** (§4). |
| **Truncated / blunted nose** (meplat diameter) | BR-365 ("blunt tip"; GAU KA's "blunted part about 0.7 calibers"); NDRC Table 3's rounded-tip noses | NDRC Table 3: a rounded tip "leads to lower shatter velocities at all angles". `Ogive`, `Cone`, `Cap` and `Flat` cannot state a blunted ogive. |
| **Nested core** in the penetrator body | M735 (tungsten core in a steel-nickel body) | The penetrator has two substances with different erosion and shatter facts. |
| **Jacket / sheath** on a sub-calibre core | 17-pdr APDS, L28 (and the NDRC 6-pdr DS model: jacket 0.23 lb on a 0.43 lb core) | It covers the core's nose. Either name it, or rule that `cap` carries it, as the context note proposed. |
| **Cap hardness scheme** (substance facts) | M61 (hard centre, tough periphery) against M51/M62 ("soft") | The shatter threshold, and soft caps' loss of grip at obliquity (Okun [S]: soft caps "torn free … at obliquities of only 15°"). |
| Fin or spin stabilisation (defer) | M735 against L28 | Impact yaw; no law reads it yet. |

## 4. Test 2: finer than the type?

**Skid angle (DERIVED).** The recommended law, sin θc = √(1+x²) − x with x = R/(2ρ_e v²), on thick RHA with R = a = 3.711 GPa. Length assumptions are labelled.

| Round (label) | ρ_e, kg/m³ (inputs) | θc at MV | θc at 0.8 MV |
|---|---|---|---|
| M72 AP | 6,113 (6.32 kg, 75 mm, 0.234 m) | 28.9° | 20.7° |
| M61 "APC" | 4,574 (full 0.336 m) / 6,182 (3.3 cal ASSUMED) | 23.4° / 29.1° | 16.2° / 20.9° |
| M62 APCBC | 6,104 (3.3 cal ASSUMED) | 38.8° | 29.8° |
| Pzgr. 39 APCBC | 4,764 (4 cal, repo) / 5,775 (3.3 cal) | 32.8° / 36.7° | 24.1° / 27.8° |
| 17-pdr APCBC | 6,723 (3.3 cal ASSUMED) | 45.0° | 36.2° |
| BR-365 APBC | 5,610 (0.289 m with cap) | 37.1° | 28.2° |
| WC cores (ρ 14,500 ASSUMED): M93 / BR-365P / 17-pdr APDS / L28 | 14,500 | 62.6° / 63.0° / 66.3° / 70.6° | — |
| Tungsten rod (ρ 17,500), 1,500 m/s | 17,500 | 72.6° | — |

Reading:

- Inside "APCBC", θc spans 23–45° at the muzzle: mostly v², but the hit length alone moves it 4–6° at equal speed. Mass, calibre, hit length and MV must be per-round.
- Tungsten cores bite 20–30° further than steel; the period's "APCR is poor on slopes" is a breakup cost (§5), not a skid.
- On a finite plate R_eff = min(R, 4τ t/d) raises θc: M61 on a 1-calibre plate gives 38.7° (DERIVED), so it bites at 30°, as the tables show.

**The capped round breaks the bare law.** NDRC Fig. 18 (37 mm M3 gun, point-blank, homogeneous BHN 260 ± 20) shows the capped M51 perforating up to 62°. With ρ_e = 6,568 (body + cap 0.835 kg over 4.63 in) at 890 m/s, the law's pressure ratio q at the measured perforations is 0.48 / 0.81 / **1.03 / 1.15** at 30 / 45 / **55 / 60°** (R_eff with t/d 1.79 / 1.28 / 0.93 / 0.75; DERIVED). So it predicts a skid at 55–62°, where the round perforated: θc comes out at 54.6° and 57.9° on those plates, 0.4–4° short (DERIVED). Raising R does not help, because the plug term 4τt/d caps R_eff on these thin plates. The contact law must read the cap.

**Shatter, inside one label (MEASURED).**

- Cap mass: 14 % of core (M51) against 27 % (an NDRC test cap, which raised the shatter velocity "from less than 2,400 fps to approximately 4,100 fps", NDRC pp.186–188).
- Hardness: a hard face over a soft core (TM9-1900), "centrally hardened" (M61), "soft" (M51, M62).
- Both must reach the shatter gate per round.
- Uncapped AP: BR-365K (sharp) costs +4–10° against BR-365 (blunt) +1–2° ([S], calculated); M72 costs +4–12°, depending on speed (§5).

**Drag and drop (DERIVED, game drag law, a `StandIn`).**

| Round, flight nose | k (1/m) | v kept at 1,000 / 2,000 yd | drop at 1,000 / 2,000 yd |
|---|---|---|---|
| 17-pdr APCBC, windshield 7 crh / 4 crh (Unknown) | 9.3e-5 / 1.07e-4 | 0.92 / 0.84 against 0.91 / 0.82 | 5.7 / 24.9 m against 5.8 / 25.6 m |
| BR-365, ballistic cap 4 crh / no cap 1 crh | 1.12e-4 / 2.07e-4 | 0.90 / 0.82 against 0.83 / 0.69 | 7.2 / 32.2 m against 7.9 / 38.6 m |
| M61 (7.17 crh) against M72 (1.64 crh), same gun | 1.01e-4 / 1.81e-4 | 0.91 / 0.83 against 0.85 / 0.72 | 11.8 / 51.7 m against 12.7 / 60.2 m |

Whether a round has a ballistic cap is visible in play; the windshield's crh is not. "Has a windshield" plus a nominal pointed ogive is enough resolution until a firing table says otherwise.

**Verdict.** "APCBC has this profile" is too coarse for skid (hit length, mass), shatter (cap mass and hardness) and bite (cap). It is adequate for drag (any windshield). The ordering is per-round mass, MV and hit length first, then per-round cap facts.

## 5. Test 3: normalization

### 5.1 Measured obliquity cost, by construction

β is DERIVED from MEASURED ratios through cos(θ + β) = t(θ)/t(0); positive is away from the normal.

| Construction | Source (criterion) | t(θ)/t(0) | β |
|---|---|---|---|
| Capped full-calibre steel, 37 mm M51, 890 m/s | NDRC Fig. 18 p.189 (chart ±0.05 in) | 10° 0.953; 30° 0.768; 45° 0.551; 55° 0.40; 60° 0.32 | +7.6, +9.9, +11.5, +11.4, +11.3 |
| Sub-calibre capped steel core, 1,253 m/s | same | 30° 0.75; 45° 0.52; 55° 0.40 | +11.4, +13.8, +11.2 |
| Jacketed WC core (6-pdr DS model), 1,234 m/s | same | 10° 0.944; 30° 0.717; 45° 0.50; 55° 0.33 | +9.3, +14.2, +14.9, +15.7 |
| WC cores, all calibres, 0° against 30° | NDRC 2C6 p.408 bands (90 % of shots; chart ±0.03) | capped ≈ 0.81; uncapped ≈ 0.70; centre ≈ 0.75 | +5.9; +15.6; +11.1 |
| Uncapped blunt monobloc, M72 | 2C3a (chart) | 30°, at 100 / 1,000 / 2,000 yd: 0.74 / 0.79 / 0.83 | **+12.4 / +7.8 / +3.9** (rises with speed) |
| Capped + windshield, M61 | 2C3a | 0.80 / 0.80 / 0.81 | +6.7 / +7.2 / +5.6 (flat: probably the rule below) |
| Capped + windshield, M62 | 2C3a 0° with HVAP45 30° | 1,000 yd 0.78; 2,000 yd 0.81 | +8.6; +6.3 |
| APCR M93 against APCBC M62, 20° → 30° | HVAP45 | 0.84–0.87 against 0.86–0.93; line of sight 0.922 | M93 pays 6–9 % beyond line of sight; M62 lies between 1 % better and 6 % worse |
| Blunt APBC / sharp AP / APCR, 85 mm | Potapov [S], **calculated** | 0.85–0.86 / 0.77–0.83 / 0.71–0.72 | +1–2 / +4–10 / +14 |
| US Army rule (all AP) | TM9-1901 p.30: "% lost": 20° 13, 30° 20, 45° 41 | 0.87 / 0.80 / 0.59 | +9.5 / +6.9 / +8.8 |
| US Navy 1932 formula, e/d₀ = 1 | NPG 2-46 eq. (3), F = 6(e/d − 0.45)(θ² + 2000) + 40000 | 0.78 / 0.58 / 0.38 at 30 / 45 / 60° (DERIVED) | +8.8 / +9.4 / +7.6 |
| Long tungsten rods, L/D ≥ 4, 0–75° | Lanz–Odermatt perforation equation, cos^m θ with **m = −0.224**, 99 tests, σ 3.3 % (longrods.ch) | 30° 0.894; 60° 0.584; 75° 0.350 | **−3.4, −5.7, −5.5** |

### 5.2 What the primary sources say about the turn

- **Away on entry, toward on exit.** "Transverse forces … tend to turn its axis away from the normal to the plate as the nose passes through the front face and toward the normal as it leaves the back … the axis of the hole usually makes a larger angle with the normal … the energy absorbed by the plate is greater than if the projectile had not turned" (NDRC p.174, spark shadowgraphs: 1.5 crh steel, 4.5 g, t = 0.19 cal, 60°, 950 ft/s).
- **Undeformed rounds keep to line of sight at low obliquity.** From "170 precise limit determinations with undeformed 3" monobloc projectiles", the limit-energy function "decreases, at obliquities less than 30°, with increase in obliquity and is … nearly proportional to cos θ". That means limit energy ∝ e·sec θ, i.e. line of sight. For e/d ≥ 0.5 it falls "at a slightly greater rate than in direct proportion to cos θ" (marginally better than line of sight). It "goes through a minimum at an obliquity near 45°, and increases … greater than 45°", because the high-obliquity plastic energy "increases more rapidly … because of projectile ricochet" (Hershey, NPG Report 2-46, pp.2, 6, 10, 18).
- **Rules, not measurements.** The US Army's 1944 conversion "once the perforation is known at normal impact, the perforation at any angle can be estimated" (TM9-1901 p.30) costs about +7–9°. The 1932 Navy formula agrees (DERIVED above). deMarre's obliquity term, sec^(≈5/7)θ on e^0.70 (exponent partly legible, NPG 2-46 eq. 1), makes thickness ∝ cos^1.02 θ, which is line of sight. So any table whose 30° column was computed from 0° by one of these carries the rule, not the round. The M61's flat 0.80 looks like that.
- **Toward-the-normal claims are qualitative.** The cap "reduces greatly the possibilities of ricochet" (TM9-1904) and "increases the biting angle" (NavPers 10797-A); neither gives a number. Okun [S] speaks only of noses "pushed strongly away from the plate" above the biting angle.

### 5.3 Reading: direction and size, by construction and speed

- **Direction.** Away from the normal for every full-calibre and tungsten-carbide round at WWII speeds. Toward it only for long rods: a finite-plate exit effect, consistent with NDRC's exit turn.
- **Size and cause.** An *intact* rigid round keeps to line of sight below about 30° (Hershey), so the service tables' +6 to +16° is mostly deformation and breakup. It is:
  - largest for uncapped tungsten carbide (+16°; "all cores tending to shatter or pulverize", BRL41 via the context note);
  - rising with speed for uncapped steel (M72: +4° → +12°);
  - smallest, and flat with range, for capped steel (+6 to +10°).
- **What the cap is.** "The cap normalizes" really means the cap halves the oblique loss: M61 +7° against M72 +12° at the muzzle; capped WC +6° against uncapped +16°. At 2,000 yd the uncapped M72 does better (+4°), as NDRC p.186 says: "the monobloc has the advantage at low velocities".

### 5.4 How much plate it changes (DERIVED)

t(θ) = t₀·cos(θ + β), for a round with t₀ = 120 mm at normal:

| β (class) | 30°: t/t₀, mm | 60°: t/t₀, mm |
|---|---|---|
| 0 (line of sight, the game today) | 0.866, 104 | 0.500, 60 |
| +6 (capped steel, low end) | 0.809, 97 | 0.407, 49 |
| +10 (capped steel at muzzle speed; M51) | 0.766, 92 | 0.342, 41 |
| +15 (jacketed or uncapped WC) | 0.707, 85 | 0.259, 31 |
| −5 (long rod) | 0.906, 109 | 0.574, 69 |

### 5.5 Proposed bend law

- **The law.** β = (90° − θ)·qⁿ, a turn away from the normal, applied at entry. Here q = R_eff sin θ / (ρ_e v² cos²θ) is the contact law's own pressure ratio (q ≥ 1 means a skid).
- **It meets §4 by construction:**
  - θ → 0 ⇒ q → 0 ⇒ β → 0;
  - v ≫ v_c ⇒ q → 0 ⇒ β → 0;
  - θ → θc ⇒ q → 1 ⇒ β → 90° − θ, continuous with the skid.
- **Inputs.** It reads only construction facts the field set already carries, plus the cap term (§4).
- **n (Unknown).** Hershey's line of sight below 30° and the minimum near 45° make the bend negligible for q ≲ 0.5 and appreciable by q ≈ 0.7–0.8, so n ≥ 4. If the bare law's q is used, M51 at 45° (q = 0.81, total measured excess 11.5°) caps the bend there: 45°·0.81ⁿ ≤ 11.5° needs n ≥ 6.5 (DERIVED). Settled by the NPG high-obliquity limits (Ballistic Summary Part IV).
- **The bite-regime excess is not a bend.** The +6 to +16° goes to the shatter/deformation gate the context note already recommends. It is read from construction: cap presence and mass, core substance (hardness, transverse-rupture strength), nose, and striking speed against the shatter velocity.
- **Rejected:** a bend tied to θc over the whole range, (90° − θc)·sin θ / sin θc or qⁿ with n ≈ 2.5. It fits M51 at 30° but overstates 45–60° by 3–16°, because Fig. 18's excess is flat (+8 to +11° from 10° to 60°). Also rejected: the energetic split tan(θ + β) = tan θ / √(1 − q), which gives M72 +20° at 2,000 yd against a measured +4°.
- **Long rods.** Their −3 to −6° is an exit-side effect: second-order, deferred until rods enter the slice.

### 5.6 Is it critical, and where?

Yes, in three places:

1. **Slopes of 45° and steeper**, where the class excess changes the plate defeated by 18–48 % against line of sight (§5.4): glacis plates at 55–60°.
2. **Tungsten rounds against capped steel on slopes**, where breakup erases most of the sub-calibre lead (NDRC p.190: +60 % at normal, +22 % at 55°).
3. **Near θc**, where the turn decides skid against bite.

It is not critical at small obliquities (≤ 10° of θ: under 5 % of plate), and not for long rods.

## 6. What would settle the open facts

- **Hit lengths and cap masses** (M61, M62, Pzgr. 39, 17-pdr APCBC, BR-365): the projectile drawings (US Ordnance; H.Dv. 481; a British drawing; the Soviet 85 mm ammunition manual).
- **Cap term in the contact law:** a measured critical angle for a capped round and the same body uncapped, at known v, θ and t/d. First look: NPG's 3-in capped-projectile reports (DTIC AD0310017, *Effect of Cap Design on 3-inch Projectile Performance*).
- **n in the bend law, and the bite/deformation split:** Hershey's *Ballistic Summary Part IV: the cap effect, and the obliquity effect for AP projectiles* (NPG 1946) and Part III; Part I's Table I gives Ø(θ) for undeformed shot.
- **A primary British 0°/30° pair** for 17-pdr APCBC and APDS, with its criterion: Ordnance Board proceedings Q1834 and 26433 (NDRC refs 46, 48), or CMHQ's *Armor Penetration Data, Armor Penetration Graphs* (1943, NDRC ref 34).
- **Fired Soviet 85 mm data** at 90°/60° meeting angles: NII-48 plating reports for BR-365, BR-365K and BR-365P.
- **105 mm:** L28/L52 normal-incidence penetration and core dimensions; M735 rod mass, length and MV (FSTC or BRL reports).
- **Chart readings** (2C3a, 2C6, Fig. 18): the tabulated OCO "Armor Perforation Curves" (NDRC ref 36) would tighten the ±0.05 in.

## Sources

Primary, read in this lane:

- **NDRC:** *Summary Technical Report of NDRC Div. 2, Vol. 1, Effects of Impact and Explosion* (1946): p.174 (oblique turn, Fig. 6); pp.184–190 (Table 3, cap effect, Fig. 18 and Table 4 p.189, p.190); Data Sheets 2C3 (Oct 1943, "Navy Limit"), **2C3a** (PTM 112, Apr 1945, p.404, page image), **2C6** (PTM 87, Jan 1945, p.408, page image); data-sheet notes p.~402. <https://archive.org/details/effectsofimpacte01unit>
- **TM9-1901:** *Artillery Ammunition* (1944): p.30 (obliquity rule, Table 1); par.87–92, pp.101–103 (M61, M72), page images. <https://archive.org/details/Tm9-1901>
- **TM9-1904:** *Ammunition Inspection Guide* (1944): pp.412–415 (M61, M72). <https://archive.org/details/TM9-1904>
- **HVAP45:** OCO, *Data for HVAP Ammunition for 76 mm and 3" Guns* (May 1945): pp.1–3. <https://www.bulletpicker.com/pdf/HVAP-Ammunition.pdf>
- **NPG 2-46:** A. V. Hershey, *Ballistic Summary Part I* (NPG, 1946): pp.2–3 (deMarre, Navy formula), 6, 10, 18. <https://archive.org/details/DTIC_AD0310022>
- **TM43:** TM 43-0001-28 (1994): M392 p.2-71, M393 p.2-81, M735 p.2-101. <https://archive.org/details/1994USArmyLargeAmmoDataSheetsTM43000128914p>
- **NavPers 10797-A** (1957), ch. 4, transcription. <https://eugeneleeslover.com/USNAVY/CHAPTER-4-B.html>
- **Lanz–Odermatt perforation equation**, the author's site (W. Odermatt). The original is Lanz & Odermatt, 13th Int. Symp. Ballistics (1992); the paper was not read. <http://www.longrods.ch/perfeq.php>

Secondary **[S]**:

- V. Potapov, Soviet tank-gun penetration table (ARTKOM-calculated). <https://losthistory.net/vif2/guns/defin_4.html>
- Tank Archives: "D-44 ballistic tables" (2015) <https://tankarchives.com/2015/10/d-44-ballistic-tables.html>; "Shell shape" (2013, CAMD RF 81-12042-85) <https://www.tankarchives.com/2013/11/shell-shape.html>; "Penetration: British edition" (2017) <https://www.tankarchives.com/2017/10/penetration-british-edition.html>
- Armedconflicts, UBR-365 (after Shirokorad 2000). <https://www.armedconflicts.com/topic/view/121900>
- US ETO memo, Shoeburyness firing of 23 May 1944, transcription. <https://wargaming.info/1998/us-army-1944-firing-test-no1/>
- Wikipedia, "Ordnance QF 17-pounder" (citing RAC Pamphlet No. 7 (1952), Bird & Livingston (2001) p.60, Fletcher (2008), Jane's). <https://en.wikipedia.org/wiki/Ordnance_QF_17-pounder>
- Osprey (WO 219/2806 Appx G). <https://www.ospreypublishing.com/ca/discover/osprey-publishing/machine-of-the-month/17-pdr-achilles>
- ra39-45, 17-pdr page (cited in the context note).
- 105 mm figures: <https://en.wikipedia.org/wiki/105%C3%97617mmR>, <https://voryshere.blogspot.com/2022/11/the-superior-tungsten-projectile.html>, <https://www.arrse.co.uk/community/threads/l11-chieftain-gun.208423/page-4>
- N. Okun: <https://www.combinedfleet.com/formula.htm>, <https://www.navweaps.com/index_tech/tech-055.php>
