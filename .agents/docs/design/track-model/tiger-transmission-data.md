# Tiger I transmission — authored data tables (2026-07-18)

Supersedes the geometric inference in codex's derivation run: the REAL per-gear speed
ladder surfaced (alanhamby.com/technical.shtml, corroborated by the Jentz & Doyle scan's
2.84 km/h first-gear figure and the panzerbasics top-gear anchors). Confidence per value:
ANCHOR (source-stated) / DERIVED (arithmetic from anchors) / INFERRED (assumption, stated).

## Anchors

- Maybach OLVAR OG 40 12 16, 8F/4R preselector [ANCHOR]
- OLVAR direct addressing: British Preliminary Report No. 19 and period operating instructions
  describe preselection of an arbitrary gear while any gear is engaged [ANCHOR]. Therefore the
  Tiger authors `gearbox.shift_addressing: Direct`: the scheduler may commit F6→F4 in one paid
  shift window. This is a selection capability, not a faster generic gearbox rule.
- Per-gear max speeds @ 3000 rpm (km/h): F1 2.8, F2 4.3, F3 6.2, F4 9.2, F5 14.1, F6 20.9,
  F7 30.5, F8 45.4; R1–R4 identical to F1–F4 [ANCHOR — hamby; Jentz scan gives 2.84 for F1]
- Top gear: gearbox 0.98 × final drive 10.55 = total 10.339 [ANCHOR]
- Governed 2500 rpm from Nov 1943 → F8 = 37.7 km/h ≈ 10.48 m/s (matches authored
  max_speed 10.5) [ANCHOR]
- Steering: "wheel controlled hydraulic operated regenerative" [ANCHOR — architecture
  confirmation]; minimum turning radius 3.44 m, MAXIMUM turning radius 165 m [ANCHOR];
  two fixed radii per gear, 16 total [ANCHOR]; "steering ratio 1.28" [ANCHOR, meaning
  unresolved — not used]
- Track contact surface 3.605 m; 96 links/track; 26.76 kg/link; combat track width 720 mm
  [ANCHOR — authoring cross-checks: our spec has 97 links (loop-rounding, fine),
  link_mass 30.0 and width 0.79 differ slightly — feel-neutral, revisit at Tiger authoring]
- HL230 P45: ~700 PS @ 3000, ~600 PS @ 2500, peak torque ~1850 N·m @ 2100 (alternate
  conversion 1810) [ANCHOR, medium confidence on peak]

## Open discrepancy (resolve at Tiger authoring)

Sprocket: RON authors 19 teeth (mesh-measured) → r_s = 19×0.130/τ = 0.3931 m; the
2.6 m/rev anchor implies 20 links/rev → r_s = 0.4138 m. IMPLEMENTATION RULE: author
per-gear SPEEDS (the anchors) and derive reductions internally against the spec's own
r_s — speed ratios are r_s-independent, so the ladder survives whichever tooth count wins.

## The ladder (DERIVED from speed anchors, scaled off the anchored G8 = 10.339)

| gear | G_total | v @3000 (km/h) | v @2500 (m/s) | R_tight (m) | r_tight | R_wide (m) | r_wide |
|---|---:|---:|---:|---:|---:|---:|---:|
| F1 | 167.64 | 2.8 | 0.648 | 3.44 [A] | 0.3954 | 10.2 | 0.7445 |
| F2 | 109.16 | 4.3 | 0.995 | 5.28 | 0.5599 | 15.6 | 0.8259 |
| F3 | 75.71 | 6.2 | 1.435 | 7.62 | 0.6727 | 22.5 | 0.8759 |
| F4 | 51.02 | 9.2 | 2.130 | 11.30 | 0.7670 | 33.4 | 0.9147 |
| F5 | 33.29 | 14.1 | 3.264 | 17.32 | 0.8416 | 51.2 | 0.9435 |
| F6 | 22.46 | 20.9 | 4.838 | 25.68 | 0.8903 | 76.0 | 0.9615 |
| F7 | 15.39 | 30.5 | 7.060 | 37.47 | 0.9235 | 110.8 | 0.9735 |
| F8 | 10.34 | 45.4 | 10.509 | 55.78 | 0.9479 | 165.0 [A] | 0.9821 |
| R1–R4 | = F1–F4, sign reversed | | | | | | |

- Radius law: R_g = R_F1_tight × G_F1/G_g within each step (radius ∝ output speed per
  engine rev — codex's corrected direction) [INFERRED mechanism, anchored at BOTH corners]
- Tight:wide ratio = (165/3.44)·(G8/G1) = **2.958 per gear** [DERIVED from the two radius
  anchors — replaces the earlier 2× guess]
- r = (R − b)/(R + b) with b = 1.4904 m (spec plane_x): the belt-speed ratio the fixed-radius
  constraint enforces.

## HL230 torque curve (piecewise-linear authoring points)

| rpm | N·m | provenance |
|---:|---:|---|
| 800 | 1300 | INFERRED (~0.70 × peak) |
| 2100 | 1850 | ANCHOR (medium — 1810 by alternate conversion) |
| 2500 | 1686 | DERIVED from 600 PS anchor |
| 3000 | 1639 | DERIVED from 700 PS anchor |

Governed fleet condition = 2500 rpm [ANCHOR]; keep 3000-rpm point for reference configs.

## Sanity

- F8 @ 2500 → 10.48 m/s ✓ matches authored max_speed 10.5.
- F1 combined sprocket force at peak torque ≈ 1850×167.6/r_s ≈ 750–790 kN (r_s-dependent)
  → far past the 2×252 kN grip cap: first gear saturates traction, as it should.
- Stage-C 20-degree reconstruction: total grade demand is **191.2 kN DERIVED** from
  `57 000 × 9.81 × sin(20°)`. At the investigation's **~980 rpm DERIVED** F4 landing, the authored
  curve and reduction give **~169 kN DERIVED** (the original investigation's **165 kN DERIVED**
  rounding), a
  reserve deficit; F3 clears the `0.10 D + 10 kN` scheduler margin.
