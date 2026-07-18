# Steering-transmission literature findings (web agent, 2026-07-18)

Confidence-tagged research backing `transmission-design.md`. Sourcing caveat: WebSearch
budget was exhausted this run — sources are direct-fetch tier (Wikipedia EN/DE,
panzerbasics, two open-access engineering PDFs: Milićević & Blagojević FME 51(3) 2023
[primary math], Rabiee & Biswas ICRA 2019 [primary friction cap], Effati PhD 2020
[primary power model]). Print-tier canon NOT reached: Wong ch.6, Merritt's papers,
Jentz & Doyle Panzer Tracts No. 6, Renk/Allison TMs.

## Architecture taxonomy (verdicts)

| Architecture | Inner→outer power? | Radius control | Neutral steer |
|---|---|---|---|
| Clutch-and-brake (T-34) | NO — brake heat (~0%) | coarse variable | no |
| Double-diff superimposed (Tiger L 600 C) | YES minus steer-brake slip | 2 fixed radii/gear = 16 | MARGINAL, brake-gated (see conflict) |
| Triple-diff regenerative (Merritt-Brown) | YES near-total | CONTINUOUS | family-capable [vehicle-specific uncertain] |
| Hydrostatic superimposed (HSWL 354 / X-1100) | YES, ~65%-band steer path only | continuous + pivot | YES (Wendestufe / pivot) |

- Clutch-brake heat-dump corroborated by a 1944 German field report on the T-34
  ("steering clutch heats up and covers with oil quickly") [secondary quoting primary].
- Merritt-Brown: "fully regenerative… none of the energy is lost to brakes or clutches";
  loss ∝ steering effort only, not total power [secondary].
- L600 lineage "Merritt-Brown derivative" is repeated but tag [secondary]. "Merritt-Maybach"
  is a CONFLATION of two lineages (British Merritt-Brown vs German Maybach double-diff) —
  refuted as one name. Panther pivot capability: sources conflict, do not assert.
- Tiger neutral steer CONFLICT: EN/DE Wikipedia say tracks counter-rotate in neutral;
  restoration tier (panzerbasics) says "Technically yes, advisable no", via the PARKING
  BRAKE button. Best synthesis: marginal brake-assisted neutral turn, not clean powered
  counter-rotation [secondary, conflict recorded].

## Braking

- "Brake ≈ traction limit" is a SOUND sizing rule [primary]: retarding force hard-caps at
  μ·N regardless of brake strength (Rabiee friction model) — a bigger brake just locks the
  track. Defensible bound: per-track brake capacity ≈ μ·W/2 at the sprocket.
- No published steering-brake torque rating reachable for any specific tank [uncertain].
  Tiger: Argus disc brakes (~38–55 cm, sources vary) as the emergency clutch-brake path.

## Steering theory (the equations, primary via Milićević)

- Steady turn: F_o = R_tot/2 + M_r/B, F_i = R_tot/2 − M_r/B; below the free-turning radius
  R0 the inner thrust goes NEGATIVE (must brake). μ(R0) = 2·f·B/L — larger L/B steers harder.
- Turning-resistance moment M_r = μ·W·L/4 (Merritt/Steeds form).
- μ is RADIUS-DEPENDENT: μ(R) = μ_max/(0.925 + 0.15·R/B) (Nikitin & Sergeev; Wong's form).
  NOTE FOR OUR MODEL: this empirical falloff is what the per-element isotropic resultant-j
  law GENERATES mechanically (Wong & Chiang) — we do not author it; classical models need
  it as input, ours emits it. Useful as a VALIDATION curve for sandbox measurements.
- Power penalty of turning: max at pivot (>2× straight-line), decays hyperbolically with R
  (Effati). Ground-drag term is irreducible; clutch-brake ADDS driveline loss on top;
  regenerative pays only ground drag + steer-path loss.
- μ ranges [uncertain, paywalled tables]: turning-resistance μ_max ~0.5–0.6 concrete /
  0.4–0.5 firm soil / 0.25–0.35 loose; traction μ ~0.9–1.2 hard / 0.7–0.9 loam /
  0.2–0.5 mud / 0.15–0.3 snow-ice. Rolling resistance is separate and additive.

## Tiger I data (for authoring)

WEB-VERIFIABLE: Maybach-Olvar OG 40 12 16 preselector, 8F/4R; top-gear gearbox ratio 0.98,
final drive 10.55, total 10.339; 45.265 km/h @ 3000 rpm / 37.72 @ 2500 (governed down from
Nov 1943); 20 links/rev × 130 mm pitch = 2.6 m per sprocket rev; two steering radii per
gear (16 total); minimum radius 3.44 m in 1st gear; L600C oil-pressure superimposed unit
with Argus-brake emergency fallback.

PRINT-ONLY (Jentz & Doyle Panzer Tracts No. 6 / D 656 manuals / Tigerfibel): full per-gear
ratio table, speed-per-gear table, the other 15 radius values, any brake torque rating.
Defensible inference if print unavailable: radius ∝ gear ratio anchored at 3.44 m (1st) —
mechanically sound for a superimposed unit, explicitly INFERRED not documented.

## Folklore refuted/flagged this run

"Controlled differential does many radii efficiently" (false — one efficient radius per
setting); "Merritt-Maybach" (conflation); Tiger clean powered pivot (brake-gated per
restoration sources); Panther pivot (conflicting sources). Solid everywhere: clutch-brake
burns inner-track power, regenerative recirculates it.
