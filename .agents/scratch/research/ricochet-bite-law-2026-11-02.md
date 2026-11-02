# Ricochet, bite and overmatch: published laws, measured angles, and the candidate (2026-11-02)

Subject: the game's damage model. Impact mechanics of a round meeting an oblique plate (skid, bite,
turn, punch through), for a game law; game state only.

**TLDR**
1. Every published closed-form ricochet law (Rosenberg 1989, Segletes 2004, the strength-medium sphere law, Vayig & Rosenberg 2021) turns on one group: the plate's resistance over the round's dynamic pressure, R/(ρ_p v²). The candidate shares that group, and its cos θc ∝ 1/v, ρ_p and R dependences agree with them.
2. The candidate departs from them in two places. It adds a dependence on mass per area (L/D) that deforming rods do not show, and it needs a nose bite depth n that the data put at 1.7 to 2.5 calibres for pointed steel cores. That n makes the 88 mm APCBC skid off thick plates head-on below about 76 reference-mm of potency (DERIVED). Its claimed "APCR skids sooner" is the reverse of what its own formula gives (68° against 41°, DERIVED).
3. On six measured rows, Segletes's simple target criterion, sin θc = √(1+x²) − x with x = R/(2ρ_e v²), comes closest without any fitted value. It lands within the measured bracket or 4° of it on the rigid-core rows, and 9 to 20° high on fast and deforming rods. The candidate needs a fitted n and still misses by 9 to 35° on those rods. The shipped tan law misses by 8 to 38° (DERIVED).
4. Thickness: no source supports a sharp "calibre ≥ 3 × thickness" threshold. A finite plate raises the critical angle continuously as V/V_BL rises (Vayig & Rosenberg 2023), and the thick-plate value is reached within about 1–3 calibres (DERIVED from plug shear: t/d = R/(4τ) ≈ 1.6 for the shipped RHA facts).
5. Recommended: one pressure-balance contact law. The round skids iff ρ_e v² cos²θ / sin θ < R_eff, with R_eff = min(R, 4τ t_n/d). It uses the march's own R, so ricochet_grip and overmatch_ratio go. Thin-plate overmatch, thick-plate saturation and speed dependence all emerge from it. A nose term and the normalization sign stay open Unknowns, each with what would settle it.

Labels: **MEASURED** (from a cited measurement), **DERIVED** (inputs shown; the arithmetic is reproducible from
the inputs, run here in a scratch script), **RECALLED** (mine, not found in a source read in this lane),
**ASSUMED** (an input I chose where no source gave it). Angles are obliquity θ from the plate normal
unless written as grazing angle α = 90° − θ. DTIC was down for maintenance on 2026-11-02; the DTIC reports
below were read from Internet Archive copies of the same PDFs.

## 1. Published models

### 1.1 Tate (1979), rigid-rotation of an eroding long rod

Primary: A. Tate, J. Phys. D 12 (1979) 1825–1829 (abstract only reached: "a simple model for the minimum
obliquity required to induce ricochet of a high speed long rod projectile from a thick target plate").
The formula as restated by Segletes, ARL-TR-3257 (2004) eq. 1 (secondary for Tate):

  tan³θc > (2/3) · (ρ_p V² / Y_p) · ((L² + D²)/(L D)) · V/(V − U)

ρ_p, Y_p rod density and strength, L/D rod aspect, U the rod's penetration speed (from a separate
penetration model, e.g. Alekseevskii–Tate). Assumptions: flat-nosed square rod. The interaction force is set
by Y_p, and the rest of the rod must rotate rigidly about its centre of gravity before the tip is fully
engaged. Groups: ρ_pV²/Y_p, L/D, U/V. In the hydrodynamic limit V/(V−U) = 1 + √(ρ_p/ρ_t) (DERIVED from
½ρ_p(V−U)² = ½ρ_tU²). That is the factor a hobbyist implementation uses; Cheng et al. (2019) print (1+ρ_p/ρ_t)
instead, a different restatement. Validity, per Segletes: it predicts no ricochet for a rigid rod (U = V
gives θc → 90°), and longer rods ricochet less, which contradicts the plastic-hinge observations of
Jonas & Zukas (1978) and Senf et al. (1981).

### 1.2 Rosenberg, Yeshurun & Mayseless (1989), deflection of the engaged tip

Proc. 11th Int. Symp. Ballistics, Brussels (not reached). As restated by Segletes eq. 2 (secondary):

  tan²θc > (ρ_p V² / H) · (V + U)/(V − U)

H the target resistance. The force acts on the rod mass actively engaging the target, not the whole rod
("bending rods"), so there is no L/D term. Segletes reports good prediction for L/D = 10 tungsten rods on
RHA at 600–1400 m/s and 55–75°. Validity: as Tate, it gives no ricochet for rigid rods (U = V).

### 1.3 Segletes (2004), plastic hinge at the contact

S. B. Segletes, *A Rod Ricochet Model*, ARL-TR-3257 (primary; journal version IJIE 32 (2006) 1403). The full
model is six simultaneous momentum-flux, force and moment balances, solved by algorithm. The simple
closed-form model drops the moment balance and assumes no rebound (α = 0) and an elastic target. Target term
(eq. 39–40): the target's force capacity H × footprint must turn the incoming momentum flux ρ_pAV²:

  sin θ / cos²θ ≥ ρ_p V² / H   ⇔   sin θc = −x + √(1 + x²),  x = H/(2ρ_pV²)

Rod term (eq. 42): sin θc ≥ (1 + Y_p/(ρ_pV²))⁻¹. Ricochet needs both (eq. 43, the max of the two).
H is Tate's resistance, H = σ(2/3 + ln(2E/(3σ))) (eq. 44, σ = σ_ULT). The model has no L/D term: the hinge is
steady state, so "it is never harder to ricochet a finite length rod" than an infinitely long one (§4.3).
Nose shape (§4.4): "a conical nose … is more prone to ricochet than a flat-nosed projectile, which can
'bite' into the target surface more readily". For a hard target relative to the rod, nose differentiation
"is perhaps lessened". Against 24 tests the simple form was the best predictor of the four models, and the
data ricocheted more readily than every model predicted. Segletes applies the simple form to the rigid
14.5 mm B32 core, which Tate and Rosenberg cannot treat at all.

### 1.4 Vayig & Rosenberg (2021–2023), rigid projectiles

Abstracts only (full texts paywalled; read via the publisher/BGU pages and the Semantic Scholar record):
- *Ogive-nosed rigid projectiles, semi-infinite metallic targets* (IJPS 13 (2022) 3–20, doi
  10.1177/20414196211037702). These are 3D simulations of CRH3 noses. The threshold ricochet angle (the obliquity
  at which the projectile ends moving parallel to the face) "depend[s] on a non-dimensional entity which is equal
  to the ratio between the target's resistance to penetration and the dynamic pressure exerted by the
  projectile upon impact". Trajectory changes "are shown to be related to the threshold ricochet angles", and
  agree with published experiments.
- *Spherical-nosed rigid projectiles* (Vayig, Ornai & Rosenberg, JDBM 8 (2022) 397–404): same subject; abstract not served.
- *Sharp-nosed rigid projectiles from metallic plates* (IJIE 178 (2023) 104621). For plates, the threshold angle is
  related to V/V_BL, the impact speed over the normal-incidence ballistic limit of the projectile/plate pair. The
  relation holds across nose shapes, aspect ratios and aluminium and steel plates, and matches 7.62 mm AP data on
  aluminium and steel. The functional form was not reached.

### 1.5 Skipping laws for spheres

- Birkhoff et al. (1944), AMP Memo 42.4M (not reached), via Johnson (1998) Part II and Hutchings (1976): a fluid
  half-space with pressure ½ρ(v cos β)² on the immersed surface. Critical grazing angle θc = 18°/√σ (σ = ρ_p/ρ_t).
  Johnson & Reid's numerical constant is 17.5°/√σ. Hutchings (IJMS 18 (1976) 243) uses Rayleigh's flat-lamina
  pressure law instead: θc = 0.302/√σ rad = 17.3°/√σ, and backspin raises it.
- With a strength term (Johnson 1998 Part II, after Soliman, Reid & Johnson 1976): the pressure becomes
  p = ½ρv² + K, "K plays the same role as does R_T, the target plate resistance in Tate's analysis". The
  result is θc² ≈ (1/10)(ρ_t/ρ_p)(1 + 2K/(ρ_t v̄²)) − 4ag/v̄². The printed equation is only partly legible; the
  factor 2 on K is DERIVED from his stated pressure law. In a strength-dominated medium, θc² ≈ K/(5ρ_p v̄²):
  the grazing critical angle falls as speed rises, the same group as §1.2–1.4.
- MEASURED (Soliman, Reid & Johnson, IJMS 18 (1976) 279): off water, θc rises with speed toward 18°/√σ. Off dry
  sand, θc falls with speed, but there is a cut-off of 29° for steel (σ 7.8), 51° for duralumin (σ 2.7) and 26°
  for lead (σ 11.2), all at about 380 ft/s; this fits 82°/√σ. The exit angle is about 0.81 × the entry angle
  (water and sand).

### 1.6 What the models share

| Model | v | ρ_p | target R | L/D | T/D | nose |
|---|---|---|---|---|---|---|
| Tate 1979 | tan³θc ∝ ρ_pV² | yes | via U only | longer harder | none (thick) | flat only |
| Rosenberg 1989 | tan²θc ∝ ρ_pV²/H | yes | 1/H | none | none | flat only |
| Segletes 2004 simple | sin/cos² = ρ_pV²/H | yes | 1/H | none | none | none (§4.4 qualitative) |
| Sphere, strength medium | α² ∝ K/(ρ_pv²) | yes | K | (sphere) | none | sphere |
| Vayig & Rosenberg | f(R/ρv²); plates g(V/V_BL) | yes | yes | via V_BL | via V_BL | via V_BL |
| Candidate | cos²θc = 2R·min(nd,t)/(ρ_p L_m v²) | yes | R | 1/L_m | min(nd, t) | n |

L_m = m/(ρ_pA). Near grazing, every row except Tate gives cos²θc ∝ R/(ρ_pv²) (DERIVED).

## 2. Measured critical angles

| # | Round (construction) | v (m/s) | Plate | t, t/d | Measured | Source |
|---|---|---|---|---|---|---|
| R1 | 7.62 mm AP hard-steel core, ogival, 6.2 × 27.6 mm, 5.2 g, ~900 VPN tip, rigid (no significant deformation) | ~820 | mild steel, 140–145 VPN, yield 327 MPa | 20 mm, 3.2 | θc = 51° (critical ricochet: enters, exits the struck face). Below θc and above the perforation angle the core embeds | Madhu, Bhat & Gupta, DSJ 53 (2003) 147 |
| R2 | same | ~820 | aluminium, 30–35 VPN, yield 135 MPa | 10–40 mm, 1.6–6.5 | perforated at every angle to 60°: θc > 60° | same |
| R3 | 14.5 mm B32 (41 g hardened steel core, brass jacket, 63.5 g); modelled as a 7850 kg/m³ cylinder 66.5 × 12.45 mm | 538 / 583 / 595 / 619 | Al, BHN 110 (H = 2.03 GPa) | 44.5 mm, 3.6 | 30° partial penetration (538); ricochet at 45° (583), 50° (596), 55° (619), 60° (595): θc in (30°, 45°) near 560 m/s | Segletes, ARL-TR-3257, Table 2 |
| R3' | same | ~1000 | same | same | perforation at 45° and 50°, ricochet at 55° and 60°: θc ≈ 52.5° | same |
| R4 | W alloy rod, L/D 15, 102.3 × 6.81 mm, 65 g, hemispherical nose, RC 35.5 | 907 / 937 | Al, BHN 190 (H = 3.16 GPa) | 50.8 mm, 7.5 | perforation at 40° (907), ricochet at 50° (937): θc in (40°, 50°) | Segletes, Table 3 |
| R4' | same | 1079 / 1088 / 1293–1402 | same | same | 55° ricochet after delving (1079), 65° ricochet (1088); 65° perforation (1295), 70° partial penetration without ricochet (1293): θc > 70° near 1300 m/s | same |
| R5 | W alloy rod 11 × 90 mm (L/D 8.2), 17.8 g/cm³ | 1113.8 / 1179.7 / 1207.9 / 1225.0 | homogeneous armour steel, yield 900 MPa | 30 mm at 60°, 2.7 | ricochet / not penetrated / embed / perforation: θc = 60° at V between 1114 and 1180 m/s. The crater depth at critical ricochet is about t/3 | Cheng et al., LAJSS 16 (2019) e233, Table 1 |
| R6 | steel spheres σ 7.8 (water, sand) | to ~90 | water; dry sand σ 2.7 | — | water: θc → 18°/√σ = 6.4° grazing at speed. Sand: cut-off 29° grazing at 380 ft/s | Soliman, Reid & Johnson 1976 |

Not tabulated, but worth a fetch:
- Gupta & Madhu, IJIE 12 (1992) 333: the same 7.62 core on mild steel 10, 12, 16, 20 and 25 mm (t/d 1.6–4), with obliquity raised until ricochet at each thickness. This is the T/D series that would settle §3; it was not reached, and only the 20 mm value is in the open DSJ paper.
- Goldsmith & Finnegan, IJIE 4 (1986) 83: 12.7 mm hard steel, conical or blunt, ~820 m/s, on Al 1.78–24.4 mm and mild steel to 19 mm, at 0–50°. Not reached.

The Seidl et al. LS-DYNA paper's "θc = 30° NATO" for 7.62 × 51 on 8 mm 350 HB plate was seen only in a search snippet, and its angle convention was not verified. Excluded.

## 3. Thickness against calibre, saturation, and overmatch

- MEASURED trend (Hershey, NPG Report 2-46, 1946; 3" AP M79 and others against STS and Class B, e/d 0.004–2.0).
  Hershey's limit-energy function is built from Thompson's F coefficient, which carries v_L cos θ. It "decreases,
  at obliquities less than 30°, with increase in obliquity and is, in fact, nearly proportional to cos θ". That
  means the limit energy ∝ t·sec θ, i.e. line of sight (DERIVED from the report's own statement that cos θ
  proportionality equals hole volume). It "goes through a minimum at an obliquity near 45°, and increases …
  at obliquities greater than 45°", because "the plastic energy in a limit impact at high obliquity …
  increases more rapidly with obliquity because of projectile ricochet". For e/d < 0.5 the data "are not
  consistent enough to justify the assumption of different obliquity effects for thin plate and thick plate."
- MEASURED (NPG Report 19-44, 1944, 3" 13 lb projectiles on homogeneous plate). Flat-nosed shells perforate at
  about 15% below the limits of capped (Mk 29-2) and monobloc (M79) AP up to e/d 0.5 and 60°, "particularly
  marked … [for] thin plate (e/d of 0.5 or less) at high obliquities (45°–60°)". Above e/d 0.65 they deform or
  shatter. Nose shape matters most on thin plates at high obliquity.
- Vayig & Rosenberg (2023, abstract): for plates the threshold angle is a function of V/V_BL. A thinner plate
  has a lower V_BL and so a higher θc.
- Cheng et al. (2019): at critical ricochet, the crater depth is about one third of the 30 mm plate (≈ 0.9 d).
  The ricochet is decided within about one calibre of depth. They also find that the semi-infinite models (Tate,
  Rosenberg, Segletes) "can not be applied to the ricochet situation of finite-thickness plate".
- Secondary search summaries only, not verified: Dong et al., J. Ballistics 26 (2014) 73, find in simulation that
  the thickness effect weakens as thickness grows. One study of tungsten fragments on aluminium and one of
  titanium plates find that a thicker plate gives a smaller θc.
- Overmatch. No source read gives a calibre-to-thickness threshold at which ricochet stops. NavPers 10797-A
  (1957) sizes AP for "about one-caliber thickness" and common shell for "1/3 to 1/2 caliber". Okun (secondary,
  combinedfleet.com) says ricochet is "essentially impossible above about 75–80° even for thin plates when
  pointed projectiles are used", which means thin plates still deflect grazing hits. The physical counterpart
  of WoT's 3× is continuous. A plate thinner than its own resistance length cannot present full resistance
  before it plugs out. With plug shear τπdt against the punch area πd²/4, the plate's normal pressure capacity
  is 4τt/d, so R_eff = min(R, 4τt/d) (DERIVED, τ = Y/√3 von Mises). Full resistance, the semi-infinite limit,
  is reached at t/d = R/(4τ). With the shipped RHA facts (a = 3.711 GPa, Y = 1.0 GPa, τ = 0.577 GPa) that is
  1.61 calibres. For R1's mild steel with Tate's H it is 2.9 (DERIVED). At WoT's threshold t/d = 1/3,
  R_eff ≈ 0.21 R, which raises θc but does not reach 90° (§6).

## 4. Normalization (the entry bend)

- Two opposite turns are documented.
  - Away from the normal, toward the free surface: rigid, sharp-nosed rounds in thick targets. The Vayig &
    Rosenberg threshold is defined by the path ending parallel to the face. Segletes's "delving ricochets" take
    an "arcuate trajectory beneath the surface". In Cheng et al. the rod "deflects" after reaching ~55% of the
    plate depth.
  - Toward the normal: blunt or capped noses, and plugging of thin plates. This is RECALLED and in Okun-type
    secondary naval texts; no primary source read here states it. NavPers 10797-A says only that the cap
    "increases the biting angle; that is, the angle at which the projectile will penetrate instead of
    ricocheting".
- Magnitude: per Goldsmith & Finnegan (1986), as summarised by Madhu et al. (2003, secondary), "at higher impact
  velocities, there is little change in the angle of flight of projectile during the penetration process."
- Constraints a future bend law must meet (DERIVED from the above). As θ → θc the bend → (90° − θ), continuous
  with the skid. As v ≫ v_c or θ → 0 the bend → 0. Its sign is set by the asymmetric (moment) part of the
  contact force, which no energy or pressure balance contains. A purely energetic bend (remove v cos θc from the
  normal component, keep the tangential) satisfies the limits but overstates the turn, by 16° for the 88 at 30°
  on 80 mm (DERIVED). It treats the whole normal-speed loss as asymmetric. Rejected.

## 5. Low speed on thick hard plates: rebound or embed

- Elastic-plastic restitution: e ≈ √(1 − (v_s/v)²) near the sticking speed, and e ∝ v^(−1/4) above the yield
  speed (Thornton & Ning 1998, via Güttler et al., arXiv:1204.0001; secondary). At ordnance speeds on steel,
  ρv²/Y ≈ 0.3–5 for 200–800 m/s (DERIVED, ρ 7850, Y 1 GPa). That is deep in the plastic regime, so a rebound
  carries little speed (RECALLED: e of order 0.1).
- MEASURED (Madhu et al. 2003): 7.62 AP cores at 805–810 m/s on two-layer mild steel (20+16, 25+12 mm, thicker
  plate in front) "penetrated to about 28–29 mm and then rebounded back". With the thin plate in front (10+20)
  they embedded at 28.9 mm. So rebound after partial penetration happens, and the stress-wave configuration
  decides between rebound and embed.
- MEASURED bands between skid and perforation: R1 (embed between the perforation angle and 51°), R3 (partial
  penetration at 30°, 538 m/s), R5 (not penetrated at 1180 m/s, embedded at 1208 m/s at 60°).

## 6. The candidate against the literature

Candidate: skid iff ½mv²cos²θ < R·A·min(nd, t), i.e. cos²θc = 2R·min(nd, t)/(ρ_pL_m v²).

Agrees:
- Its group is R/(ρ_pv²), and near grazing cos²θc ∝ R/(ρ_pv²), as in Rosenberg, Segletes and the strength-medium sphere law.
- θc rises with v and ρ_p and falls with R.
- It is invariant under replica scaling (DERIVED).
- The min(nd, t) form has the right qualitative finite-plate structure: θc rises as the plate thins, saturates once t > nd, and reaches 90° only as t → 0. In the thin branch cos θc = v_t/v, a V/V_BL law like Vayig & Rosenberg 2023's.

Disagrees, or is unsupported:
1. **Mass per area.** cos²θc ∝ 1/L_m makes long rods arbitrarily hard to deflect. Segletes's hinge and Rosenberg
   have no length term, and the rod rows confirm it: even at maximal s = t the candidate gives 62.5° for R4 at
   920 m/s (measured 40–50°) and 69–72° for R5 (measured 60°) (DERIVED, §7).
2. **Bite depth n.** It is free, and the data want it large and speed-dependent. R1 needs n = 2.54 with R = a, or
   1.69 with Tate's H. R3 needs 1.75 < n < 2.24 near 560 m/s, but n ≥ 3.6 (s = t) at 1000 m/s, and even then
   gives 54.0° (H) or 61.2° (a) against a measured 52.5° (DERIVED). The candidate's cos²θc ∝ v⁻² is steeper
   than R3 shows (d ln cos²θc / d ln v ≈ −0.94 between 570 and 1000 m/s, DERIVED from the bracket midpoints).
3. **Large shells.** With the shipped facts, the 88 mm Pzgr 39 at 500 m/s (52.5 reference-mm) skids at every
   obliquity, head-on included, from any plate thicker than its nose-length bite (n = 0.866, 76 mm). So in the
   candidate a slowed round never embeds in thick armour; R1, R3 and R5 all show embed bands. Untested for
   P < b, but no source supports it.
4. **Thin branch.** It imposes normal-component perforation (limit energy ∝ sec²θ) at all obliquities.
   Hershey measures line of sight (sec θ) below 30°, so the candidate over-skids marginal hits at 20–45°:
   by up to +15% potency at 30° and +41% at 45° against the LOS rule. The truth at 45° is bounded at ≤ +73%
   (DERIVED from Hershey's "∝ cos θ to 30°, minimum near 45°").
5. **Flat nose.** n → 0 gives b = 0, and the round never skids at any angle. NPG 19-44 records flat noses
   deforming above e/d 0.65, and Segletes's argument is about biting, not immunity.
6. **APCR.** "APCR skids on slopes sooner than APCBC" is the reverse of its own formula. The Pzgr 40 core
   (1.93 kg, 35.7 mm, 930 m/s) bites to 68.2° against 41.3° for the Pzgr 39 on thick RHA (DERIVED, R = a,
   n = 0.866). The period observation that APCR does poorly on slopes (RECALLED) is a core-shatter effect,
   and §3 of the design doc omits shatter.

## 7. Computed critical angles on the measured rows (DERIVED)

How each column was computed:
- Inputs as in §2. ASSUMED: R1's core density is 7850 and its ogive is ψ = 3 (N 0.1065, nose 1.66 d); mild steel E is 205 GPa. R3's nose factor N = 0.15 is Segletes's k_T, which gives ψ = 2.09 and a nose of 1.36 d. R5's nose is assumed flat.
- R = a is the repo's cavity-expansion strength term (2/3)(1+ln(E/(3(1−ν)Y)))Y. H is Tate's σ(2/3+ln(2E/3σ)).
- "Shipped" is tan θc = 9.42·N·P/t, with P the Poncelet depth in the plate's own material.
- "Pressure" is the recommended law (§8) with ρ_e = Segletes's homogeneous density. R1 is also given with ρ_e = m/(A·L) = 6241 kg/m³, from the core's 27.6 mm length.
- My Segletes column reproduces his own 61.5° for R3 at 1000 m/s, which validates the implementation.

| Row | Measured | Shipped | Candidate (n = nose length, R = a) | Candidate n needed | Pressure, R = a | Pressure, R = H (= Segletes simple) | Tate / Rosenberg |
|---|---|---|---|---|---|---|---|
| R1 7.62 core, 20 mm mild steel, 820 | 51° | 59.3° | 59.4° (R = H: 51.4°) | 2.54 (a), 1.69 (H) | 60.6° (ρ_e 6241: 57.3°) | 54.5° (ρ_e 6241: **50.6°**) | 67.6° / 73.2° |
| R2 7.62 core, 40 mm Al, 820 | > 60° | 64.8° | 71.1° | — | 71.0° | 67.0° | 90 / 90 (rigid) |
| R3 B32, 44.5 mm Al, 538 | > 30° | 59.3° | 56.5° | < 2.24 (H) | 48.1° | 40.4° | 90 / 90 |
| R3 B32, 583 | ≤ 45° | 63.1° | 59.4° | > 1.75 (H) | 51.0° | 43.6° | 90 / 90 |
| R3' B32, 1000 | ≈ 52.5° | 79.4° | 72.7° | unreachable (s = t: 54.0° H) | 66.5° | 61.5° | 90 / 90 |
| R4 WA L/D 15, 50.8 mm Al, 920 | 40–50° | 87.8° | 84.3° | unreachable (s = t: 62.5° H) | 68.6° | 64.1° | 80.8° / 73.7° |
| R5 WA 11 × 90, 30 mm RHA, 1150 | 60° | 88.7° | 90° (flat nose) | s = t: 69.2° (Rt 4.437) | 68.4° | 63.7° | 79.1° / 72.8° |

Critical speed at 60° for R5, measured 1114–1180 m/s (DERIVED):
- Segletes simple (target and rod terms): 740 m/s. Cheng et al. print 310/451 m/s; I could not reproduce those values.
- Candidate with s = t: 716 m/s (R = a), 815 m/s (Rt 4.437 GPa).
- Rosenberg, simplified: 288 m/s.
- Tate: ≈ 30 m/s.

Reading:
- On the rigid-core rows (R1–R3 near 560 m/s), the pressure law with Tate's H lands inside the measured bracket or within 4° of it, with nothing fitted.
- Every law overshoots fast and deforming rods (R3', R4, R5), as Segletes found. Those are hinge and delving regimes, outside the game's rigid march.
- The shipped law is worst on every row.
- The rows prefer R ≈ H ≈ 1.5 a. That bears on the march's own Unknown resistance; it is not a reason to add a second number.
- I also tested a geometric burial depth b(θ) = ℓ cos θ + ½d sin θ in the candidate. The rows rejected it (2–5° more bite-prone than n = ℓ/d).

## 8. Recommended game law

One contact law (DERIVED proposal; provisional):

  **skid iff ρ_e v² cos²θ / sin θ < R_eff**  ⇔  θ > θc,  sin θc = √(1 + x²) − x,  x = R_eff / (2ρ_e v²)
  **R_eff = min(f·R, 4τ·t_n/d)**

Inputs:
- ρ_e = m/(A·L): the mean density of what meets the plate over its length. This is a construction fact (mass, calibre, length), and length is a new round datum. For a solid core it is the core material's density.
- v: the speed at the face.
- R and τ = Y/√3: the reference steel's strength term a and shear flow stress, already material facts. f is the face's material factor.
- t_n: the crossing's steel-equivalent thickness along the normal.
- d: the calibre of what meets the plate.
- A round that does not skid bites and marches LOS as now.

What emerges:
- θc rises with speed and density and falls with plate strength.
- A spent round skids at modest angles; head-on it never skids, and a too-slow round stops at the face, with an optional small rebound (§5).
- Thickness stops mattering past t_n/d = R/(4τ) (1.61 calibres for the shipped RHA).
- A thin plate cannot hold its full resistance, so θc → 90° as t_n → 0 (overmatch). A thin plate still turns grazing hits.
- The law is invariant under replica scaling.

What it deletes: ricochet_grip, overmatch_ratio, and the per-type nose term in θc.

Illustration (DERIVED, shipped RHA facts; ρ_e ASSUMED 7850 for solid steel, or 4764 for a 4-calibre Pzgr 39):

| Round | t | Shipped | Candidate | Pressure (7850 / 4764) |
|---|---|---|---|---|
| 88 APCBC, 773 m/s | 10 mm | 90.0° | 74.2° | 76.5° / 72.7° |
| | 30 mm | 84.6° | 61.9° | 66.9° / 60.6° |
| | 45 mm | 81.9° | 54.7° | 61.9° / 54.5° |
| | 80 mm | 75.8° | 41.3° | 53.2° / 44.2° |
| | 115 mm | 70.0° | 41.3° | 46.8° / 37.0° |
| | thick | 0° | 41.3° | 42.8° / 32.8° |
| 88 APCBC, 500 m/s | 45 mm | 72.7° | 26.8° | 48.0° / 38.3° |
| | thick | 0° | 0° (skids head-on) | 25.5° / 17.1° |
| Pzgr 40 core (WC ASSUMED 14.5 g/cm³), 930 m/s | thick | 0° | 68.2° | 59.6° |

Unknowns, each with what would settle it:
- **ρ_e for hollow capped shells.** None of the rows test it, and the main game case depends on it (the ±8° spread above). Settle with WW2 sloped-plate firing records of AP shell at known v, θ and t/d, for example Hershey's Ballistic Summary Part IV (NPG 1946, "the cap effect, and the obliquity effect for AP projectiles"), whose DTIC copy is a truncated scan.
- **The plug-capped thickness term.** It is DERIVED. Settle with Gupta & Madhu 1992 (θc at t/d 1.6–4) and the functional form in Vayig & Rosenberg 2023.
- **A nose term.** The data here do not support one of the hit-nose factor's size. Settle with Vayig & Rosenberg 2022 (CRH3) against Vayig, Ornai & Rosenberg 2022 (spherical) at equal R/ρv².
- **The skid exit.** The sources range from a slide along the face (Segletes, α = 0) to 0.81 × the entry angle (spheres in water and sand). Neither supports the shipped mirror at v sin θ. A restitution-zero slide is the simplest default.
- **The bend's sign and moment term** (§4).
- **Deforming rods.** Add Segletes's rod term sin θc ≥ (1 + Y_p/(ρv²))⁻¹ only when rods enter the slice.

## Sources

Read in this lane:
- Segletes, S. B. *A Rod Ricochet Model*, ARL-TR-3257 (2004). https://apps.dtic.mil/sti/pdfs/ADA426125.pdf (read from the Internet Archive copy)
- Cheng, C. et al. "Oblique penetration of tungsten alloy rod to finite-thickness metal plate", LAJSS 16(9) e233 (2019). https://www.lajss.org/index.php/LAJSS/article/download/5724/2803/18682
- Madhu, V., Bhat, T. B., Gupta, N. K. "Normal and oblique impacts of hard projectile on single and layered plates", DSJ 53(2) 147–156 (2003). https://publications.drdo.gov.in/ojs/index.php/dsj/article/view/2139
- Khan, W. U., Ansari, R., Gupta, N. K. "Oblique impact of projectile on thin aluminium plates", DSJ 53(2) 139–146 (2003), for its summaries of Goldsmith & Finnegan 1986 and Gupta & Madhu 1992. https://publications.drdo.gov.in/ojs/index.php/dsj/article/download/2138/1119
- Madhu, V. PhD thesis front matter, IIT Delhi (1993). https://ir.iitd.ac.in/bitstreams/fcd14780-6fca-4b5f-a8ef-2adef7910265/download
- Johnson, W. "Ricochet of non-spinning projectiles … Part I", IJIE 21 (1998) 15–24. https://blog.espci.fr/pse1/files/2018/03/ricochet_johnson1.pdf
- Johnson, W. "… Part II", IJIE 21 (1998) 25–34. https://www3.eng.cam.ac.uk/~hemh1/dambusters/ricochet_johnson2.pdf
- Soliman, A. S., Reid, S. R., Johnson, W. IJMS 18 (1976) 279–284. https://www3.eng.cam.ac.uk/~hemh1/dambusters/ricochet_johnson3.pdf
- Hutchings, I. M. IJMS 18 (1976) 243–247. https://www3.eng.cam.ac.uk/~hemh1/dambusters/calcs/ricochet_hutchings1.pdf
- Hershey, A. V. *Ballistic Summary Part I*, NPG Report 2-46 (1946). https://apps.dtic.mil/sti/pdfs/AD0310022.pdf (Internet Archive copy)
- NPG Report 19-44 (1944), flat-nosed 3" projectiles. https://apps.dtic.mil/sti/pdfs/AD0309992.pdf (Internet Archive copy)
- *Naval Ordnance and Gunnery* Vol. 1, Ch. 4, NavPers 10797-A (1957), transcription. https://eugeneleeslover.com/USNAVY/CHAPTER-4-B.html
- Güttler, C. et al. arXiv:1204.0001 (secondary, for Thornton & Ning). https://ar5iv.arxiv.org/html/1204.0001
- Okun, N. (secondary). https://www.combinedfleet.com/formula.htm

Abstracts or metadata only:
- Tate 1979 (doi 10.1088/0022-3727/12/11/011)
- Lee, Lee & Shin 2002 (doi 10.1088/0022-3727/35/20/331)
- Vayig & Rosenberg 2022 (doi 10.1177/20414196211037702)
- Vayig & Rosenberg 2023 (doi 10.1016/j.ijimpeng.2023.104621). https://cris.bgu.ac.il/en/publications/on-the-ricochet-of-sharp-nosed-rigid-projectiles-from-metallic-pl/
- Vayig, Ornai & Rosenberg 2022 (doi 10.1007/s40870-022-00341-y)
