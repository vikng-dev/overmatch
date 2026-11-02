# Round-type profiles by construction (2026-11-02)

**Subject:** the game's damage model. This note covers how armour-piercing and other tank round types are built, and what each construction feature does when it meets a plate, so the game can define each round by its construction and let its behaviour emerge. Game state only. Where crew come up, they are HP pools at stations.

## TLDR

1. A round type is a bundle of construction facts: what meets the plate (the full-calibre body, a cap, or a sub-calibre core of a stated diameter and substance), the nose that meets the plate, the nose that flies (a windshield is flight only and breaks up on impact), any carrier and whether it is kept or discarded, and any filler and fuze. The names AP, APC and APCBC are national labels for these bundles: US "APC" (M61, M62) is British and German APCBC. A law should read the bundle, not the name.
2. The sources' order of effect: **shatter** is the big switch. An uncapped steel nose shatters at 20–45° obliquity above about 2,000–2,500 ft/s, and a shatter can double the energy needed to perforate. A cap prevents shatter: a cap of 27 % of core weight raised one shatter velocity from under 2,400 to about 4,100 ft/s, at every angle from 0 to 60°. The cap costs energy when the round would not have shattered anyway. Against face-hardened plate the cap is worth it at every velocity (76 mm at 1,000 yd: AP M79 defeats 3.0 in of face-hardened plate, APC M62 defeats 4.7 in, and both defeat 4.5 in of homogeneous plate).
3. APCR/HVAP against APCBC from the same gun (MEASURED, US 76 mm): APCR penetrates about twice as much at the muzzle (8.6 against 4.4 in at 20°). It holds 53 % of its speed at 3,000 yd (firing table) and its penetration falls to parity with APCBC by 2,500–3,000 yd. It pays more for obliquity than APCBC does. The Allies' German 88 mm estimates show the APCR losing its lead between 1,000 and 1,500 yd at 30°. APDS discards its carrier at the muzzle, so it flies dense and keeps its lead at range.
4. The owner's carrier-body question: the WWII empirical practice scales penetration by the core alone (NDRC data sheets). So the game's current choice (fly whole, meet the plate as the core) is the sourced first-order law, and no strip stage is needed for penetration. The sources blame APCR's oblique weakness on the **core breaking under transverse load**, not on the body yawing it. Core shatter therefore deserves its own generic shatter gate, and a yaw stage does not. The body's roughly 74 % of the energy (Pzgr. 40, DERIVED) belongs at the face as a crater, not behind it.
5. Two of today's table rows are contradicted by primary sources. First, the 8.8 cm Pzgr. 40 has a **ballistic cap** (TM 9-1985-3 Fig. 427; TTT No. 24), so APCR should fly on a pointed nose. With a 4-crh flight nose the game's own drag law reproduces the Tigerfibel sight rule; today's 1-crh nose does not. Second, the "AP" bullet is a **jacketed steel core** (a kept carrier), not a monobloc. Recommended field set: §7.

## 1. Scope, labels, sources

**Labels.** **MEASURED** means a number printed in a cited source; for a chart, it is a reading of the printed chart. **DERIVED** means computed by me, with the inputs shown. **RECALLED** means mine, not found in a source read for this note. **[S]** marks a secondary source. Unit conversions are DERIVED: 1 lb = 0.4536 kg, 1 ft/s = 0.3048 m/s, 1 in = 25.4 mm, 7,000 gr = 1 lb.

**Read in the repo (read-only):** `assets/ballistics/round_types.ron`, `src/rounds.rs`, the five files in `assets/rounds/`, `src/spec.rs::NoseShape`, the force law in `src/ballistics.rs`, `assets/ballistics/angle_laws.ron`, `src/substances.rs::MaterialFacts`, the bite-law candidate note, and `.agents/scratch/rounds-research/kwk36-rounds-2026-10-30.md` (for its source keys).

**Primary sources read** (full keys in §9). Each OCR text was searched and page images were checked where the OCR failed.

- **TM9-1900:** TM 9-1900 *Ammunition, General* (1945).
- **TM9-1901:** TM 9-1901 *Artillery Ammunition* (1944).
- **TM9-1904:** TM 9-1904 *Ammunition Inspection Guide* (1944).
- **TM9-1907:** TM 9-1907 *Ballistic Data, Performance of Ammunition* (1948).
- **TM53:** TM 9-1985-3 *German Explosive Ordnance* (1953).
- **NDRC:** *NDRC Summary Technical Report, Div. 2, Vol. 1: Effects of Impact and Explosion* (1946), ch. 6 by C. W. Curtis and the Weapon Data Sheets.
- **BRL41:** BRL report by J. Leeder, *Armor Piercing Bullets with Sintered Carbide Cores* (Nov 1941).
- **HVAP45:** Office of the Chief of Ordnance, *Data for HVAP Ammunition for 76 mm and 3" Guns* (May 1945).
- **TTT24:** *Tactical and Technical Trends* No. 24 (1943).
- **TM30-410:** TM 30-410 *Handbook on the British Army* (1942).
- **FM17-12:** FM 17-12 *Tank Gunnery* (1961).
- **TM43:** TM 43-0001-28 *Army Ammunition Data Sheets* (1994).
- **BRL-TR-3236** (1991).

## 2. Today's table, and where it meets the sources

`round_types.ron` holds four rows. Each row has a flight nose, a hit nose (`NoseShape`, calibres) and a carrier (`None`, `Kept`, `Discarded`). The rows are HE, APCBC, APCR and AP (a rifle bullet). The force law reads mass, calibre and the hit-nose factor `N`. Drag reads the flight nose and the flying body.

| Row (today) | What the sources say |
|---|---|
| APCBC: flight `Ogive(4)`, hit `Ogive(1)` | The flight nose is the windshield, a false ogive of **7.05–8.96 crh** in the US examples: M62 7.05, M61 7.17, M51 8.96, 57 mm M86 7.21 (TM9-1901 data blocks, MEASURED). The hit nose is the **cap's nose** over a body ogive. Steel AP bodies are typically **1.25–1.5 crh** (NDRC p.185, "a relatively blunt nose, 1.25- to 1.5-caliber radius, is usually chosen"); US monoblocs are 1.52 (M74), 1.64 (M72) and 1.68 (M79) crh (TM9-1901, MEASURED). The Pzgr. 39's own figures remain Unknown. |
| APCR: flight `Ogive(1)` "no ballistic cap" | **Contradicted.** The 8.8 cm Pzgr. 40 drawing labels a **ballistic cap** over the forebody, plus air spaces, the tungsten-carbide core and a tracer (TM53 p.440, Fig. 427, MEASURED; labels partly legible). TTT24: "a mild-steel body, a light-alloy or plastic ballistic cap, and a cemented, tungsten-carbide core". The US M93 has "an aluminum windshield" (TM9-1900 par.100c(6)). |
| APCR carrier `Kept`, penetrator block 1.93 kg / 35.7 mm | Agrees with TM53 p.440: core 4.25 lb, 1.406 in diameter, **5.437 in long**, total 16.0 lb. NDRC's data sheets scale tungsten-carbide-cored rounds by "the weight and diameter of the core, or part of the projectile which penetrates the plate" (NDRC p.~402, notes to sheets 2C3–2C8). |
| AP (bullet): flight `Ogive(6)`, hit `Cap(0.5)`, carrier `None` | **Construction mismatch.** A US AP bullet is "a gilding metal jacket, a tungsten chrome steel core, a lead … point filler" (.30 M2), or "a gilding metal jacket; a tungsten-chrome steel core; and a point filler of lead hardened with antimony" (.50 M2) (TM9-1904). That is a hard core inside a soft envelope, the same construction as a kept carrier. NDRC scales "small-caliber jacketed bullets" by core weight and diameter. |
| HE: `Ogive(4)` both | Plausible as a class. US HE runs 4.34 crh (37 mm M54) to 7.51 crh (75 mm M64, "a modification of the M48 High-explosive Shell") (TM9-1901, MEASURED). It is per-round, not per-class. |

## 3. Construction per type

### 3.1 Representative examples (WWII first, then later)

Columns: d is the bore calibre. m/d³ is a size-free relative weight in kg/dm³ (DERIVED). L/D is length over diameter, where a source gives the length. MV is muzzle velocity.

| Type | Example | What meets the plate | Mass | m/d³ | MV | Source |
|---|---|---|---|---|---|---|
| AP (monobloc shot) | British 2-pdr AP, 40 mm | whole shot | 2 lb 6 oz = 1.08 kg | 16.8 | 2,616 ft/s = 797 m/s | TM30-410 par.99 (MEASURED). Monobloc construction RECALLED |
| AP | US 76 mm M79 shot | whole shot: "steel slug with a rounded nose", 1.68 crh, "neither ballistic nor armor-piercing cap"; "heat-treated, hardened steel, solid shot" | ≈14.9 lb = 6.75 kg | 15.3 | 2,600 ft/s = 792 m/s | TM9-1901 par.142, TM9-1904 p.448 (MEASURED). Mass DERIVED: complete round 24.24 lb − (M62 complete 24.80 − projectile 15.44); TM9-1901 says the rounds differ only in the projectile |
| AP | US 75 mm M72 shot | monobloc, "without armor-piercing cap or ballistic cap", solid but for a tracer cavity, 1.64 crh, length 9.21 in (L/D 3.1, DERIVED) | ≈13.8 lb = 6.28 kg | 14.9 | 2,030 ft/s = 619 m/s | TM9-1901 par.92 (MEASURED). Mass DERIVED as for M79, ASSUMED same case and charge as M61 |
| APHE (uncapped, filled) | German 3.7 cm Pzgr. | whole shell, base fuze, filler | 1.68 lb = 0.76 kg | 15.0 | 2,500 ft/s = 762 m/s | NDRC Data Sheet 2C7, p.409 (MEASURED; the sheet's "AP shell"). TM53 p.382: "A. P. projectile without cap", fuze and tracer |
| APC (capped, NDRC usage) | German 5 cm (Pak 38) "APC" | capped body | 4.54 lb = 2.06 kg | 16.5 | 2,700 ft/s = 823 m/s | NDRC 2C7 (MEASURED). The German round is the Pzgr. 39, which TM53 lists as APCBC. NDRC files it as APC: the labels are not construction |
| APBC | Soviet 76 mm BR-350A | blunt head under a ballistic cap, filler | 6.3 kg (sources disagree) | ≈14.4 (DERIVED) | 662 m/s | **[S]** forum citations of Krogfus; no primary read |
| APCBC | German 8.8 cm Pzgr. 39 | piercing cap over the body; ballistic cap for flight; base fuze Bd.Z. 5103; ¼ lb RDX/wax | 10.2 kg (22 lb in TM53) | 15.0 | 773 m/s | Repo `88mm_AP.round.ron` (NV5 **[S]**); TM53 pp.447–448. TM53 Fig. 426 (Flak 41 Pzgr. Patr. 41, p.439) draws "piercing cap" and "ballistic cap" |
| APCBC ("APC" in US usage) | US 75 mm M61 | "a steel body which contains a small charge of explosive D; a steel armor-piercing cap; and a light-weight steel or aluminum ballistic cap"; false ogive 7.17 crh; base fuze M66A1 (delay) | 14.96 lb = 6.79 kg | 16.1 | 2,030 ft/s = 619 m/s (M3 gun) | TM9-1901 par.87; TM9-1907 Table LI (MEASURED) |
| APCBC | US 76 mm M62 | "heat-treated, hardened steel body", "softer steel AP cap sweated on", "cast aluminum windshield"; false ogive 7.05 crh | 15.44 lb = 7.00 kg | 15.8 | 2,600 ft/s = 792 m/s | TM9-1901 par.139; TM9-1904 p.448; TM9-1907 Table LI |
| APCBC | US 37 mm M51 | body "very hard steel"; cap "soft steel … sweated on"; aluminium windshield; components 1.61 lb core, 0.23 lb cap (14 % of core), 0.03 lb windshield; L/D 4.36 with windshield, 3.17 without (DERIVED: 6.36 and 4.63 in over 1.46 in) | 1.87 lb (NDRC); 1.92 lb (TM9-1904) | 16.8 | 2,900–2,920 ft/s ≈ 890 m/s | NDRC Table 4 p.189; TM9-1904 p.~347 (MEASURED) |
| APCBC (later) | US 90 mm M82 | capped, fuzed (BD M68) | 24.11 lb = 10.94 kg | 15.0 | 2,670–2,800 ft/s = 814–853 m/s | TM9-1907 Table LI |
| APCR | German 8.8 cm Pzgr. 40 | TC core, 35.7 mm × 138 mm (L/D 3.87, DERIVED), 1.93 kg; mild-steel body; ballistic cap | 16.0 lb = 7.26 kg (TM53); 7.3 kg (NV5 **[S]**) | 10.7 (whole); core 42.3 | 930 m/s (NV5 **[S]**) | TM53 p.440 Fig. 427 (MEASURED); TTT24 |
| APCR (HVAP) | US 76 mm M93 | "aluminum body with an adjustable nose plug, a tungsten core, a steel base, a windshield, and a tracer"; core reported as 3 lb 15 oz = 1.79 kg, 42 % of the round (**[S]** collector page; DERIVED share) | 9.35 lb = 4.24 kg | 9.6 | 3,400 ft/s = 1,036 m/s | HVAP45 pp.1–2; TM9-1900 par.100c(6); TM9-1907 Table LI |
| APCR | German 5 cm Pzgr. 40 (Pak 38) | TC core in a light body | 1.95 lb = 0.88 kg | 7.1 | 3,930 ft/s = 1,198 m/s | NDRC 2C7 |
| APCR (later) | US 90 mm M304 HVAP | TC core, light body | 16.80 lb = 7.62 kg | 10.5 | 3,350 ft/s = 1,021 m/s | TM9-1907 Table LI |
| APDS | NDRC model of the 6-pdr DS Mk I, at 37 mm | **jacketed** TC sub-projectile: core 0.72 in / 0.43 lb in a jacket 0.95 in / 0.23 lb; complete 0.88 lb in a 1.46 in bore. DERIVED: sabot 0.22 lb (25 % of launch mass); jacket 35 % of the flying mass; core/bore 0.49 | 0.88 lb launched; 0.66 lb flies | launch 7.8; sub-projectile 21.3; core 31.9 | 4,050 ft/s = 1,234 m/s | NDRC Table 4 p.189 (MEASURED, read from the page image) |
| APDS | British 6-pdr 7-cwt (57 mm) | TC core with "heavy steel windshield and Duralumin pad covering the nose" | — | — | 4,150 ft/s = 1,265 m/s | NDRC p.187 |
| APDS | British 17-pdr APDS | TC core in a light steel sheath, four-piece light-alloy sabot | — | — | ≈3,950 ft/s ≈ 1,204 m/s | **[S]** only (ra39-45.co.uk); no primary read |
| APDS (later) | 105 mm M392 (UK L36A1/L28) | "sheathed tungsten carbide core" in a sabot; spin-stabilised | complete round 41 lb | — | 1,478 m/s | TM43 p.2-71 (MEASURED) |
| APFSDS (later) | 105 mm M735 | "steel-nickel body, which houses a tungsten core", aluminium windshield and fins; aluminium sabot in three 120° sections | complete round 39.50 lb | — | not captured | TM43 p.2-101 |
| APFSDS (later) | 120 mm M829 | "DU penetrator … one-piece", windshield, six-bladed aluminium fin; aluminium sabot in four 90° segments | — | — | 5,510 ft/s = 1,679 m/s | TM43 (M829 sheet) |
| APFSDS (lab rod) | BRL half-scale DU rod, L/D 20 | 730 g core, 266.7 mm long (d ≈ 13.3 mm, DERIVED); 815 g in flight; 3.06 kg in bore | 0.73 kg | 308 | 1.7–2.4 km/s tested | BRL-TR-3236 (MEASURED) |
| HE | German 8.8 cm Sprgr. L/4.5 | thin-walled shell; 2 lb amatol 60/40; nose fuze A.Z. 23/28 | 9.30 kg | 13.7 | 820 m/s | TM53 pp.444–445; repo `88mm_HE.round.ron` |
| HE | US 37 mm M54 | thin-walled; 4.34 crh | complete round 2.62 lb | — | 2,600 ft/s | TM9-1901 par.33 |
| HE | US 76 mm M42 | "1-piece steel"; 0.86 lb TNT | — | — | — | TM9-1904 p.448 |
| HEAT | German 8.8 cm Gr. 39 HL | shaped charge, 0.91 kg RDX/wax; nose fuze A.Z. 38 | 7.71 kg (TM53) | 11.3 | 600 m/s | TM53 p.444; prior repo note |
| HEAT | US 75 mm M66 (howitzer) | "thin-walled casing containing a shaped high-explosive filler … closed off … by a ballistic cap in the form of a thin steel cone"; BD fuze, nondelay; nose taper 22°50′ | complete round 16.30 lb | — | ≈1,000 ft/s | TM9-1901 par.129; penetration "at any range" ≈3½ in (MEASURED, OCR partly garbled) |
| HEAT (later) | 105 mm M456 | steel body, copper cone liner, Comp B 2.14 lb, standoff spike with piezoelectric nose, fins, PIBD fuze M509A1 | complete round 48 lb | — | 1,173 m/s | TM43 p.2-83 |
| HESH / HEP (later) | 105 mm M393 HEP | "a thin-walled cylinder with a relatively short ogive and a flat base"; 6.6 lb Comp A3 (plastic HE); BD fuze | complete round 45 lb | — | 731.5 m/s | TM43 p.2-81. No WWII tank-gun HESH found in the sources read |
| AP bullet | US .30 AP M2 | jacketed tungsten-chrome steel core with a lead point filler | 166 gr = 10.8 g | 24.3 | 2,775 ft/s = 846 m/s | TM9-1904; TM9-1907 Table LI |
| AP bullet | US .50 AP M2 | jacketed tungsten-chrome steel core | 708 gr = 45.9 g | 22.4 | 2,935 ft/s = 895 m/s | as above |

**Class patterns (DERIVED from the table).**

- Full-calibre steel AP, APC and APCBC sit at **m/d³ ≈ 15–17 kg/dm³** (m/d² ≈ 1,100–1,350 kg/m² at 75–90 mm).
- APCR sits at **≈ 7–11** (light, full calibre).
- An APDS sub-projectile sits at **≈ 21**; the long rod at **≈ 300**.
- The tungsten-carbide core of an APCR is **≈ 40–50 % of bore diameter** (0.41 for the Pzgr. 40). NDRC p.189: "a tungsten carbide core having a diameter approximately half that of the gun".
- Overall lengths: "about 3.5 calibers overall" for monobloc steel rounds and tungsten-carbide cores. Cores of capped steel rounds are "shorter than 3.5 by ½ to 1 caliber" (NDRC p.185, MEASURED). APFSDS rods are L/D ≥ 15 (patents **[S]**) and 20 in the BRL lab rod.

### 3.2 Construction elements, per the sources

**Body and nose (AP).** "Made of heat-treated high-carbon alloy steel. The head is very hard for penetration of armor and the body is tough to withstand the strains imposed by impact and the twisting action of the projectile at angles of impact oblique to normal" (TM9-1900 par.100c(5); TM9-1901 p.28). NDRC p.184 explains why: "the nose is kept hard but the body softened to allow plastic deformation … a nose failure leads immediately to shatter, which is much more disastrous than a body failure". The US small-arms AP core steel is Rockwell C 61–65, specific gravity 8.00 (BRL41 p.5, MEASURED).

**Armour-piercing cap.** The sources disagree on cap hardness because designs differed:

- **Hard face, soft core:** TM9-1900 par.100c(5)(c): "of forged alloy steel, heat-treated to have a hard face and a relatively soft core. On impact, the hardened face of the cap destroys the hardened surface of the armor-plate, while the softer core of the cap protects the hardened point … by distributing the impact stresses over a large area of the head". TM9-1904 p.16 agrees: "high-carbon chrome steel … very hard [in front of the point] while the skirt is very tough".
- **Soft cap:** the 37 mm M51 cap "is made of soft steel … Its purpose is to reduce the possibility of ricochet and to serve as a cushion or guide for the body" (TM9-1904). The 76 mm M62 has "a softer steel AP cap sweated on" (TM9-1904 p.448).

Cap mass: 14 % of core weight (M51) and 27 % (an NDRC test cap) (NDRC pp.186, 188, MEASURED).

**Ballistic cap (windshield, false ogive).** "To increase the ballistic efficiency … The windshield is made of brittle material and shatters on impact with the target" (TM9-1904 p.16). It is light: 0.03 lb on the 1.87 lb M51, 1.6 % (DERIVED). On HEAT the conical windshield is the **standoff**: "It has a conical windshield which provides stand-off for the charge" (TM9-1900 par.100c(2)).

**Filler and fuze (APHE, APCBC-HE).** "When the cavity is filled with explosive D, the projectiles are fitted with a base-detonating fuze having a delay action … providing an opportunity for penetration before detonation" (TM9-1900; TM9-1901 par.87). The filler is small: ¼ lb in the 22 lb Pzgr. 39, ≈1.1 % (DERIVED from TM53).

**Tungsten-carbide core.** "Cemented or sintered tungsten carbide … characterized by high density, high hardness, and low transverse rupture strength" (NDRC glossary, MEASURED). At 6 % Co or Ni binder: "the compressive strength … is over twice as great as that of the standard steel, and the density greater by almost the same factor" (BRL41 p.5).

**Carrier (APCR body).** TTT24 gives a mild-steel body for the AP 40. HVAP45 and TM9-1900 give an aluminium body and nose plug with a steel base for the M93. The Soviet TsNII-48 study, via a 2020 article **[S]**, gives a mild-steel "spool" of about 140 HB that "acted as a plastic sleeve upon impacting the armor".

**Sabot (APDS, APFSDS).** It leaves the sub-projectile at the muzzle: "Setback, centrifugal, and air pressure forces cause the sabot to discard upon leaving the gun tube" (M392); "aerodynamic forces cause the sabot to separate" (M829A2) (TM43).

**HEP / HESH.** "Upon impact, the plastic explosive filler of a HEP projectile mushrooms on the armor and detonates. This force transmits shock waves through the armor, causing large jagged spall to be torn from the back of the armor plate" (FM17-12 par.20b(2)).

## 4. What each feature does at the plate

**4.1 Shatter is the dominant switch.** "At the shatter velocity … projectile deformation usually leads to an abrupt increase in the energy required for perforation … greatest at normal incidence (increase by as much as 100 per cent) … significant … for angles at least as great as 45 degrees" (NDRC p.7). The shatter velocity depends on nose and angle. Watertown data for steel projectiles against homogeneous plate of BHN 341 (NDRC Table 3, p.184, MEASURED):

| Angle of attack | 3.0 crh | 1.5 crh | 1.27 crh | 1.0 crh |
|---|---|---|---|---|
| 0° | 4,000+ ft/s | 3,400+ | — | — |
| 15° | — | 3,200 | 3,050 | 2,600 |
| 22.5° | — | 2,750 | 2,600 | 2,500 |
| 30° | 2,000 (est.) | 2,400 | 2,350 | 2,450 |

What the table means:

- A sharp nose resists shatter at normal and a blunt one at 30°.
- A rounded tip lowers every shatter velocity. Flat ends shatter most but "require considerably less energy" against very thin plate.
- In m/s: 2,000–2,450 ft/s at 30° is 610–747 m/s (DERIVED). That is below the 773 m/s Pzgr. 39 and the 792 m/s M62 muzzle velocities, so uncapped steel at 30° shatters at ordinary WWII tank-gun speeds.
- NDRC p.186: "The shatter velocity of a monobloc steel projectile is so low at obliquities that even a full-caliber projectile of this type is ineffective if fired from a present day standard gun in the important range between 20 and 45 degrees."
- Skirting plates "decap" capped rounds (NDRC p.190).

**4.2 What a cap does.** "The purpose of the cap is to prevent shatter" (NDRC p.186). The sources quantify it:

- "A particular cap (27 per cent of core weight) increased the shatter velocity … from less than 2,400 fps to approximately 4,100 fps; … equally effective … at all angles of attack from 0 to 60 degrees". Where shatter was not the limit, the same cap cost "for 1-caliber plate at normal incidence … the limit velocity by 10 per cent and the limit energy by 54 per cent" (NDRC pp.186–187, MEASURED).
- Against face-hardened plate "the cap is useful at all striking velocities". Against soft homogeneous plate (BHN 250) "the monobloc has the advantage at low velocities (less than approximately 2,500 fps …)" (NDRC p.186).
- In US service data, AP M79 against APC M62 from the same 76 mm gun at 1,000 yd and 0°: face-hardened plate **3.0 against 4.7 in**; homogeneous plate **4.5 against 4.5 in** (TM9-1901 pars.139, 142, MEASURED).
- Obliquity and bite: "The cap also performs the valuable function of increasing the angle of obliquity at which penetration or perforation will take place, thus tending to avoid ricochet" (TM9-1904 p.16). For the soft M51 cap: "reduce the possibility of ricochet and to serve as a cushion or guide" (TM9-1904). Neither source gives a number.
- Face-hardened armour depends on shatter: "Face-hardened armor is appreciably better than homogeneous only if the hard surface succeeds in shattering the projectile … when the cap accomplishes its purpose the advantage of face-hardened armor largely disappears" (NDRC p.171).

**4.3 What a ballistic cap does: flight only.** It is brittle and "shatters on impact" (TM9-1904). Its whole effect is range. NDRC p.187: "the performance of the monobloc projectiles is worse at long ranges than that of the capped projectiles … The cause … is not its poor terminal-ballistic behavior but the fact that only the capped projectiles have windshields."

**4.4 Blunt against sharp nose: ricochet, scooping, entry bend.**

- **Intact perforation energy.** At normal, the larger-radius ogive is best: a 1.5-crh nose needs 15 % more limit energy than a 5.0-crh nose on 4-calibre plate, and 4 % more on 1-calibre plate. "At high obliquities, projectiles with a large radius ogive have a greater tendency to scoop; since this leads to a higher limit energy, a blunt-nosed projectile becomes best under these conditions" (NDRC pp.184–185, MEASURED).
- **Entry bend.** The sources contradict the folk idea that a nose straightens on entry. "Transverse forces … tend to turn its axis away from the normal to the plate as the nose passes through the front face and toward the normal as it leaves the back … the axis of the hole usually makes a larger angle with the normal … than the angle of incidence" (NDRC p.174; shadowgraphs of a 1.5-crh steel round against thin plate at 60°). That is one observation, on thin plate, not a law.
- **Ricochet.** A 1942 Soviet letter exchange gives two opposed views. Malyshev: "A blunt nosed shell that hits armour at an angle ricochets … a sharp nosed shell … bites". GAU KA: "the sharp tipped shell penetrates the armour 30-40 mm away from the place of impact" so "the chance of ricochet is higher". GAU KA also says sharp tips "work better against homogeneous armour at 0 degrees" but "worse against cemented armour", and blunt shells "need less speed … under most conditions" (CAMD RF 81-12042-85, via Tank Archives **[S]** translation). NDRC also notes that past the ricochet angle "very little of the projectile's striking energy is absorbed by the plate; it is merely redirected" (p.174).

**4.5 APCR/HVAP at range and at obliquity.** See §6. In brief: the low ballistic coefficient makes the round "effective only over a very short range" (NDRC p.189), and the tungsten-carbide core is weak to transverse load at obliquity (BRL41).

**4.6 Where shatter appears for tungsten carbide.** "Body failures occur at extremely low velocities … The nose remains intact, however, until the shatter velocity is reached, whereupon the whole projectile disintegrates." "Although the shatter velocity of a tungsten carbide projectile is not much greater than that of a good steel, it can have almost twice the striking energy without shattering because of its greater weight" (NDRC pp.184–185). In Data Sheet 2C6 (Aug 1944): "Tungsten carbide cores usually break up on impact. If there is complete disintegration of the core, perforation of the indicated thickness may not be attained"; capped and uncapped cores "at 0° scatter randomly through the band; at 30° there is an evident separation" (NDRC p.408). Against face-hardened plate at normal, carbide cores beat steel ones "mainly … [because] the carbide cores tend to remain intact while the standard shatter" (BRL41 abstract).

**4.7 Long rods (APFSDS).** At high speed the rod erodes. Penetration tends to a hydrodynamic limit **P/L = √(ρp/ρt)**, independent of speed. Below 1.8 km/s DU has "an approximate 100 m/s performance edge" over 90 % tungsten heavy alloy, attributed to adiabatic shear, and the two converge by 1.9–2.0 km/s (BRL-TR-3236 pp.12–13). The rigid cavity-expansion law the game ships cannot state this regime; an eroding regime would need **penetrator length**.

**4.8 Chemical energy (HEAT, HEP).** "Chemical energy projectiles have a given armor-defeating potential that is constant regardless of range to the target or velocity of the projectile" (FM17-12 par.20b). For the M66 HEAT: "The effect … is obtained by the force of the detonation … rather than from striking velocity" (TM9-1901 par.129).

## 5. The typical differences that matter for play

- **AP against APC against APCBC (same gun).**
  - Against homogeneous plate at modest striking speed and small angles, AP ≈ APC; plain AP can be slightly better because the cap's mass does no penetrating work.
  - Against face-hardened plate, or at 20–45° above ≈2,000–2,500 ft/s, AP shatters and APC does not, and APC wins big (76 mm, 1,000 yd: 3.0 against 4.7 in face-hardened).
  - APC's blunt cap flies badly. APCBC is APC with a windshield, so it keeps the cap's behaviour at the plate and flies like a 7–9 crh shell.
  - APHE is any of these with a small filler cavity and a delayed base fuze.
  - APBC is a blunt-nosed body under a windshield, with no piercing cap. Its blunt nose resists shatter and scooping at obliquity (NDRC); the Soviet use is attested only secondhand.
- **APCR against APCBC (same calibre).** APCR is lighter (m/d³ ≈ 10 against 15–16), faster at the muzzle (+30 %: 3,400 against 2,600 ft/s at 76 mm) and carries a dense core of about half the bore. Measured on the 76 mm, M93 against M62:
  - The muzzle lead is **+95 % at 20° and +89 % at 30°**.
  - The lead shrinks steadily with range.
  - Parity comes at **≈3,000 yd at 30°** (2.6 against 2.6 in) (HVAP45 p.3, MEASURED).
  - APCR pays more for obliquity than APCBC does (§6.2).
  - Behind the plate it carries less: a 1.9 kg core against a 10.2 kg body (the game already derives a weaker cone).
- **APDS against APCR.** Both carry a tungsten-carbide core of about half the bore and both reach hypervelocity.
  - APDS **discards its carrier at the muzzle**: 25 % of launch mass in the NDRC 6-pdr model. It flies on a dense sub-projectile, m/d³ ≈ 21 against ≈ 10 for whole APCR.
  - So APDS keeps its velocity: "the high ballistic coefficient of the subprojectile makes it effective over a much longer range than the composite rigid" (NDRC p.189).
  - The jacket stays on and protects the nose like a cap: a heavy steel windshield gave "approximately 25 per cent more armor at all but near-normal obliquities" (NDRC p.187).
  - The NDRC 37 mm DS model gives "60 per cent more armor than the full-caliber projectile; … still 22 per cent at 55 degrees" (NDRC p.190).
  - Costs: WWII accuracy ("not so accurate at the present stage of development") and sabot petals downrange.
- **APFSDS** is a long eroding rod, L/D 15–30, fin-stabilised. Penetration grows with rod length and density (§4.7).
- **HEAT** potency does not depend on range. **HEP/HESH** works by a contact shock that tears spall from the back face, with no penetration. **HE** works by blast and fragments, with fuze options.

## 6. The APCR/HVAP carrier body, and APDS by contrast (the owner's question)

**Setting (DERIVED).** Pzgr. 40: a 7.3 kg round with a 1.93 kg core, so the body is 74 % of the mass and of the impact energy at any speed. At 930 m/s that is 3.16 MJ in total, 0.83 MJ in the core and 2.32 MJ in the body. The M93's reported core (**[S]**) makes its body ≈58 %. Today the game flies the whole round and lets only the core meet the plate.

### 6.1 Where the body's energy goes at impact

- **Against armour.** No primary source traces the body's energy. The sources do state what the body does **not** do: the WWII data reduction treats "projectiles with cap or windshield which come off on impact, or tungsten-carbide cored projectiles" by "the weight and diameter of the core, or part of the projectile which penetrates the plate" (NDRC notes to Data Sheets 2C3–2C8, MEASURED). NDRC p.187 adds two qualifications:
  - "The portion of the jacket surrounding the sides of the core apparently has little effect in controlling breakup"
  - but "in cases where the core does not shatter, however, material behind the core often contributes to the penetration by giving the core a boost from behind". That is why some measured limits "are sometimes less than would be computed" from the core alone.

  The TsNII-48 summary **[S]** says the mild-steel spool "acted as a plastic sleeve upon impacting the armor", protecting the core. The body's energy therefore goes into its own flow and the face: a cratered or splashed entry. RECALLED, not sourced: the body's deformation, front-face cratering and heat; it leaves the plate in front, not behind.
- **Against soft targets (masonry, timber, soil).** Not found in any source read. FM17-12 only says "HE concrete, HEAT, shot, hypershot, or sabot is required to penetrate concrete or masonry walls". Proposed reasoning (DERIVED, unverified): the body strips only where the face pushes back harder than the body can carry. Against brick or timber, whose resistance is far below mild steel's or aluminium's flow strength, the whole round stays together and makes a full-calibre hole.
- **Game check (DERIVED).** The game's core-only readout puts the Pzgr. 40 at ≈1.57× the Pzgr. 39 at the muzzle, against the record's ≈1.43 (repo note, NV5 **[S]**). RECALLED: NV5's KwK 36 table is at 30°, the German convention. A 0° readout set against a 30° record over-states APCR by the obliquity penalty in §6.2. So the gap does not call for a "body boost", and the core-only rule needs nothing added for penetration.

### 6.2 Does the body's collapse at obliquity yaw the core?

- **Not found as a mechanism in any primary read.** The primary sources name a different mechanism: **the core breaks under transverse load**. "At obliquities the penetrating ability of the carbide bullets relative to that at normal deteriorated more rapidly than that of the standard … associated with the relative weakness of these hard sintered materials to transverse impact, all cores tending to shatter or pulverize" (BRL41 abstract). Against homogeneous plate at normal, carbide and steel cores had "identical" efficiency; at obliquity carbide "became progressively inferior" (BRL41).
- **How large (DERIVED from MEASURED tables).**
  - US 76 mm, HVAP45: from 20° to 30°, line of sight alone predicts a ratio of 0.922 (cos 30°/cos 20°). M93 measures **0.84–0.87**, that is 6–9 % below line of sight. M62 measures **0.86–0.93**, 0–7 % below.
  - Allied estimates for the German 88 mm Flak 41 (NDRC Data Sheet 2C7, p.409, "in most cases are estimates based either on actual firing data or on extrapolation"), from 0° to 30°: APCBC holds **0.85–0.86**, which is line of sight (cos 30° = 0.866). AP40 holds **0.79 at 500 yd falling to 0.65 at 2,500 yd**, so the AP40's penalty grows as its striking speed falls.
  - At 30° the AP40's lead over APCBC is gone between 1,000 and 1,500 yd (6.5/5.9, then 5.4/5.5 in). At 0° it lasts to 2,500 yd (5.7/5.6).
  - NDRC p.189 cautions against over-reading this: "Contrary to the opinion that tungsten carbide projectiles are inferior at oblique angles of incidence, their superiority exists at 55 degrees obliquity as well as at normal, although not to the same extent" (for composite rigid and sabot rounds in five guns).

### 6.3 Tungsten-carbide core shatter: obliquity, face-hardened plate, velocity window

- The core cracks across its body at "extremely low velocities", "invariably". The nose holds until the shatter velocity, and then "the whole projectile disintegrates". Shatter velocity rises with hardness, and rupture velocity rises with transverse-rupture strength; the composition trades one for the other (NDRC p.184).
- "Monobloc tungsten carbide projectiles might conceivably be used in subcaliber sizes if the striking velocity was limited to values below about **3,000 fps** [914 m/s, DERIVED] and the attack restricted to homogeneous plate" (NDRC p.186). That is a velocity window for an unprotected core. Data Sheet 2C6: "usually break up on impact".
- Against face-hardened plate at normal, carbide cores **outlast** steel ones (BRL41). The weakness is transverse: obliquity, and yaw at impact. The TsNII-48 summary **[S]** reports cores often shattering against the high-hardness T-34 plate.
- Nose protection helps: a heavy steel windshield plus a Duralumin pad gave about +25 % "at all but near-normal obliquities" at 4,150 ft/s (NDRC p.187).

### 6.4 In flight: the sectional-density penalty (MEASURED, same gun)

M93 HVAP firing table, 76 mm and 3-inch guns (HVAP45 pp.9–10), MV 3,400 ft/s:

| Range | Terminal velocity | Share of MV kept (DERIVED) |
|---|---|---|
| 1,000 yd | 2,838 ft/s | 0.835 |
| 2,000 yd | 2,302 ft/s | 0.677 |
| 3,000 yd | 1,803 ft/s | 0.530 |

- M62 APCBC from the same gun, MV 2,600 ft/s, read from the TM9-1907 Fig. 114 chart (p.204): ≈2,280 / ≈2,000 / ≈1,700 ft/s at 1,000 / 2,000 / 3,000 yd. That is MEASURED by chart reading, ±50 ft/s. Its rotated grid makes this the least certain number in the note.
- Penetration falls in step. The M93 keeps 35 % of its muzzle penetration at 3,000 yd (3.0/8.6); the M62 keeps 64 % (2.8/4.4) (HVAP45).
- TTT24 says of the German AP 40: "the velocity falls off rapidly with the increasing range, so that increased penetration is obtained at short ranges only".

**Check against the game's drag law (DERIVED).** With the shipped constants, a 4-crh flight nose has form 0.1424, giving k = 1.95e-4 per m for the M93 (4.24 kg, 76.2 mm). The firing table's first 1,000 yd gives k = ln(3400/2838)/914.4 m = **1.98e-4 per m**, a match within 1.5 %. The table's later segments give 2.29e-4 and 2.67e-4: drag rises as the round slows. That is the Mach dependence the `drag` block already labels a `StandIn`.

**The Pzgr. 40's flight nose (DERIVED).** The Tigerfibel tells the gunner to aim the Pzgr. 40 on the Pzgr. 39 scale **100 m short at 600–1,000 m and 200 m short at 1,100–1,500 m** (D 656/27 p.71, as cited in `kwk36-rounds-2026-10-30.md`; not re-read here). Integrating flat-fire drop under the game's law, with the Pzgr. 39 on 4 crh (k = 1.081e-4) at 773 m/s, gives these aim-short distances:

| Pzgr. 40 flight nose | 600 m | 1,000 m | 1,200 m | 1,500 m |
|---|---|---|---|---|
| Today, 1 crh (k = 2.79e-4) | 72 m | 88 m | 85 m | 68 m (shrinking) |
| Ballistic cap, 4 crh (k = 1.51e-4) | 92 m | 144 m | 166 m | 197 m |

The capped nose reproduces the Fibel's rule; today's nose does not. This agrees with the drawing.

### 6.5 Verdict: which effects get a stage

| Effect | Verdict | Why |
|---|---|---|
| Carrier strip before the core bites, for penetration | **No stage** | The sources' own reduction scales by the core alone. The game's core-only readout already over-reads the 30° record, and the only quantified correction (the "boost from behind") is second-order and points the other way. |
| The carrier's energy at the face | **Keep, as face damage, not penetration** | At 74 % of the round's energy (Pzgr. 40) this is visible: a wide crater or splash at the entry. Cheap: deposit the carrier's kinetic energy at the face's crossing as the face's dose. Not sourced quantitatively. |
| Carrier against soft faces | **One law, not a stage (proposal, unsourced)** | The carrier strips only where the face's resistance exceeds the carrier's flow strength; otherwise the whole round meets the target. That uses substance facts the game already holds (`flow_strength`) and needs no per-type rule. Settled by a sourced firing of composite rigid shot against masonry or timber. |
| Body-induced yaw of the core at obliquity | **No stage** | Not found in the sources. The measured oblique penalty (6–9 % beyond line of sight for M93; up to ≈25 % in the Allied AP40 estimate at low striking speed) is attributed to core breakup. |
| Core shatter (tungsten carbide and steel alike) | **A stage: one generic shatter gate** | This is the largest effect in every source: up to 2× the perforation energy, and the thing that separates AP, APC and APCBC. It reads striking speed, obliquity, the hit nose, cap protection and the core material's hardness and transverse-rupture strength. Without face-hardened plate in the game, its main visible effect is at 20–45°. |
| Sectional-density penalty in flight | **No stage; emerges** | From mass, calibre and flight nose. It needs the APCR flight nose corrected to its ballistic cap. |
| APDS by contrast | **No new stage** | The carrier is `Discarded` at the muzzle (already modelled). The jacket that stays on is part of the flying penetrator and acts as its nose protection, which the shatter gate's "cap protection" input covers. |

## 7. Recommendation: the minimal construction fields

**Decisions.**

1. **The round-type table stops holding numbers.** Every nose and carrier fact moves into the round file, because each is a per-round fact. US windshields alone run 7.05–8.96 crh, and AP monoblocs 1.52–1.68 crh. A type default standing in for a round's truth is the "fifth outcome" the house rules forbid.
2. **The type key stays as a display and validation label.** It states which construction blocks a round must and must not have (for example, APCBC needs a cap and a flight nose distinct from its hit nose; APCR needs a penetrator with a kept carrier). It holds no numbers and no law reads it.
3. **The `drag` block stays where it is.** It holds the drag law's parameters, not round facts.

Labels follow the project's jobs: Measured, Derived, Unknown (what would settle it), StandIn, Unit.

| Field (round file) | Unit / type | Job label (expected) | Read by | Replaces / keeps |
|---|---|---|---|---|
| `mass` | kg | Measured | force law, drag, recoil, energy | **keeps** |
| `caliber` | m (body diameter) | Measured | drag area, full-calibre contact | **keeps** |
| `length` | m (flying body) | Measured, or Unknown until a drawing | drag (later: L/D, yaw), eroding-rod regime, an upper bound on burst-cavity geometry (the APHE round's 70 mm cavity length is ASSUMED today, `88mm_APHE_ring.round.ron`) | **new** |
| `flight_nose` | `NoseShape` (the outer contour: windshield, or the body's own nose) | Measured from a drawing (e.g. 7.05 crh false ogive), else Unknown | drag form | **replaces** the type's `flight_nose` |
| `hit_nose` | `NoseShape` (the nose that meets the face: the cap's if capped, the penetrator's if sub-calibre, else the body's) | Measured, else Unknown | force law `N`, contact (bite) law `n`, shatter gate | **replaces** the type's `hit_nose` and the round's optional `nose` override (one field, required) |
| `body` substance | substance key | Measured or Unknown | shatter gate (hardness, transverse rupture), the regimes that deform | **keeps** today's `core`, renamed (for a sub-calibre round it names the carrier; see below) |
| `cap` | `Option<(mass kg, substance)>` | Measured (e.g. M51 0.23 lb) or Unknown | contact law (the cap meets the face first; its mass is spent there, not carried through: NDRC +54 % limit energy where shatter is not at stake); shatter gate (raises the shatter threshold) | **new** |
| `penetrator` | `Option<(mass kg, caliber m, length m, substance)>` | Measured (Pzgr. 40: 1.93 kg, 35.7 mm, 138 mm, tungsten carbide) | force law, contact law `A`, shatter gate (L/D, transverse strength), eroding regime | **extends** today's block with `length` and `substance` (today the round's `core` names the penetrator's substance and the carrier is unnamed) |
| `carrier` | `Option<Kept \| Discarded>`; its mass is DERIVED (`mass` − `penetrator.mass`) and its substance is the round's `body` | Derived / structural | drag (which body flies), face-damage deposit, soft-face strip law | **replaces** the type's `carrier`; required exactly when `penetrator` is present (as today) |
| `burst`, `fuze` | as today | as today | burst and fuze laws | **keep** |
| `shaped_charge` | `Option<(liner substance, charge diameter m, standoff m, filler substance, filler mass kg)>` | Unknown until a HEAT round is authored | a jet law whose potency does not depend on speed (FM17-12) | **new**, when HEAT is in scope; standoff is the windshield's length (TM9-1900) |
| `wound`, `effects`, `name` | as today | Unit / Taste / Measured | crew line rate; view | **keep** |

**Material facts the laws will need (substances table, not round fields).** `MaterialFacts` holds density, flow strength, E and ν. A shatter gate also needs **hardness** and **transverse-rupture strength**: the two properties the NDRC trades against each other for carbide, and the one BRL41 blames for the oblique breakup. Add them as Unknown with "settled by a cited property table". BRL41 Table I gives grade-by-grade carbide values; NDRC p.184 gives the trade.

**Fields deliberately not added.**

- **Windshield mass:** 1.6 % on the M51 (DERIVED), and it "shatters on impact". It is fully stated by `flight_nose` ≠ `hit_nose`.
- **Spin or fin stabilisation:** matters only for dispersion and impact yaw; defer until a law reads it.
- **A HESH block:** the M393 is the existing `burst` with an impact fuze on a thin, flat-based case. Its spall-tearing effect is a later law, not a new field set.
- **Separate jacket fields for APDS:** NDRC treats the sub-projectile (jacket plus core) as the penetrating body. Author it as the `penetrator` with the core's substance, and let the shatter gate read the jacket as nose protection through the `cap` block if a source gives the jacket's nose mass.

**Concrete consequences for today's rows.**

- **APCR (88mm_APCR):** set `flight_nose` to the ballistic cap's pointed ogive, Unknown in crh until the drawing is measured. The DERIVED Fibel check prefers about 4 crh over 1 crh. `body` = mild steel (TTT24), `penetrator.substance` = tungsten carbide, `penetrator.length` = 0.138 m (TM53).
- **AP bullet (7.9mm_AP):** author it as a **jacketed core**: `penetrator` = the steel core, `carrier` = Kept, `body` = gilding metal. The core's mass and diameter are Unknown, settled by an S.m.K. drawing. NDRC scales jacketed bullets by the core. Retire the label "AP" for it, or split "AP shot" (monobloc) from "AP bullet".
- **APCBC (88mm_AP, 88mm_APHE_ring):** add a `cap` (mass and substance Unknown); `hit_nose` = the cap's nose (Unknown); `flight_nose` = the windshield (Unknown; US examples 7.05–8.96 crh, MEASURED).
- **HE (88mm_HE):** `flight_nose` = `hit_nose` = the shell's ogive (Unknown; US HE 4.34–7.51 crh); `length` from L/4.5, the designation's length in calibres. RECALLED that the suffix means this; settled by TM53's drawing.

## 8. Open items (each with what would settle it)

- **Pzgr. 39 cap:** mass, substance, nose shape, and the body ogive under it. Settled by H.Dv. 481/60 or a German projectile drawing; TM53 Fig. 426 draws the Flak 41 round, not the KwK 36's.
- **Pzgr. 40 ballistic-cap crh, and its remaining velocity at range.** Settled by a KwK 36 firing table (H.Dv. 481/60) or Brit43's range data.
- **Carrier behaviour against masonry and timber.** No source found. Settled by a composite-rigid firing record against non-metal targets.
- **The M62 velocity at range.** The TM9-1907 chart reading is approximate. Settled by firing table FT 76-C-1.
- **True APC (cap without windshield) examples with figures:** British 6-pdr and 17-pdr APC are RECALLED only. Settled by a British ammunition handbook.
- **APBC (BR-350A) construction figures:** secondary only. Settled by a Soviet ammunition manual.
- **APFSDS penetrator masses and lengths for service rounds (M829, M735):** TM43 gives construction and MV, not dimensions.

## 9. Sources

Primary:

- **TM9-1900:** TM 9-1900 *Ammunition, General*, War Dept, 18 Jun 1945, par.100 (projectile components and types). <https://archive.org/details/TM9-1900>
- **TM9-1901:** TM 9-1901 *Artillery Ammunition*, War Dept, 29 Jun 1944, pars.33, 34, 42, 73, 87–92, 129, 139–142 and p.28. <https://archive.org/details/Tm9-1901>
- **TM9-1904:** TM 9-1904 *Ammunition Inspection Guide*, War Dept, 2 Mar 1944, p.16 (AP, cap and windshield functions), the 37 mm M51 entry, p.448 (M79, M62), and the cal .30/.50 AP bullets. <https://archive.org/details/TM9-1904>
- **TM9-1907:** TM 9-1907 *Ballistic Data, Performance of Ammunition*, Dept of the Army, July 1948, Table LI p.188; Figs. 114 and 116, pp.204 and 206. <https://archive.org/details/TM91907BalisticDataPreformaceOfAmmunition>
- **TM53:** TM 9-1985-3 *German Explosive Ordnance (Projectiles and Projectile Fuzes)*, March 1953, pp.382, 439 (Fig. 426) and 440 (Fig. 427, Pzgr. 40), read from page images. <https://archive.org/details/TM9-1985-3>
- **NDRC:** *Summary Technical Report of the NDRC, Division 2, Vol. 1: Effects of Impact and Explosion*, OSRD, 1946. Used: summary pp.6–8; ch. 6 "Terminal Ballistics of Armor" (C. W. Curtis) pp.160–190, including Table 3 p.184 and Table 4 p.189; glossary; notes to Weapon Data Sheets 2C3–2C8; Data Sheet 2C6 (Aug 1944) p.408; Data Sheet 2C7 (PTM No. 99, Feb 1945) p.409, read from the page image. <https://archive.org/details/effectsofimpacte01unit> (also DTIC AD0221586)
- **BRL41:** J. Leeder, *Armor Piercing Bullets with Sintered Carbide Cores*, Ballistic Research Laboratory, Aberdeen Proving Ground, Nov 1941 (DTIC AD0702256), abstract and pp.5–6. <https://archive.org/details/DTIC_AD0702256>
- **HVAP45:** Office of the Chief of Ordnance, *Data for HVAP Ammunition for 76 mm and 3" Guns*, May 1945 (with covering letter of 21 May 1945), pp.1–3 and 9–10. <https://www.bulletpicker.com/pdf/HVAP-Ammunition.pdf>
- **TTT24:** "The German 'AP 40' Antitank Shot", *Tactical and Technical Trends* No. 24, 6 May 1943. <https://lonesentry.com/articles/ttt/ap40-pak-ammunition.html>
- **TM30-410:** TM 30-410 *Handbook on the British Army*, 30 Sep 1942, par.99. <https://archive.org/details/TM30-410HandbookOnTheBritishArmy1942>
- **FM17-12:** FM 17-12 *Tank Gunnery*, Dept of the Army, 1961, pars.19–23 and the armoured-targets section. <https://archive.org/details/FM_17_12_T_G_1961>
- **TM43:** TM 43-0001-28 *Army Ammunition Data Sheets: Artillery Ammunition*, 1994: M392 p.2-71, M393 p.2-81, M456 p.2-83, M735 p.2-101, M829 and M829A2. <https://archive.org/details/1994USArmyLargeAmmoDataSheetsTM43000128914p>
- **BRL-TR-3236:** *High Velocity Performance of a Uranium Alloy Long Rod Penetrator*, Ballistic Research Laboratory, 1991 (DTIC ADA236191), pp.5, 12–13. <https://archive.org/details/DTIC_ADA236191>
- Tigerfibel D 656/27 p.71, as cited in the repo note `.agents/scratch/rounds-research/kwk36-rounds-2026-10-30.md` (not re-read here).

Secondary **[S]**:

- T. Jentz and H. Doyle, *Tiger I Heavy Tank 1942–45* (NV5), via the repo round files.
- Tank Archives, "Shell shape" (29 Nov 2013): a translation of CAMD RF 81-12042-85 (1942). <https://www.tankarchives.com/2013/11/shell-shape.html>
- Armedconflicts.com summary of the TsNII-48 study, via Ye. Fyodorov, topwar.ru (2020). <https://www.armedconflicts.com/18098->
- Ra39-45, the 17-pounder page (APDS). <https://ra39-45.co.uk/guns-equipment/17-pounder-anti-tank-gun>
- bocn.co.uk collector thread on the 76 mm HVAP (M93A1 core weight). <https://www.bocn.co.uk/threads/evolution-of-american-76mm-hvap.90633/latest>
- Battlefront forum posts citing Krogfus (BR-350A). <https://community.battlefront.com/topic/39888-762mm-vs-tiger-side-armor>
