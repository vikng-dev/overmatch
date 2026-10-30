# Wall prototype: candidate crack laws on a brick wall, handbook values only

Subject: the game's environment destruction, as game state only. A wall chunk is **turned to dust**
(removed) or **cracked loose** (its joints broke); rounds and blasts **dose** what they cross.

Labels: **MEASURED** (read off this prototype's runs), **DERIVED** (computed here from other
numbers), **CLAIMED** (stated by a source; "read" when the source was read this session),
**ASSUMED** (a choice with no source).

## Run it

```
cargo armor          # = cargo run --bin armor_sandbox --features dev_ui
```

The wall stands to the right of the range (x 14–18 m, z 4 m). Press **G** to stand in front of it.

| Key | Does |
|---|---|
| 1 2 3 4 5 | crack law A, B, C, D, E |
| U | structural check on/off (on by default) |
| B / F | pick the round (88 mm AP, 88 mm APCR, 88 mm APHE, 7.92 mm) / fire it (an MG fires a burst of 50) |
| P, `[` `]` | point deposit at the aim point; its energy, 50 kJ to 5 MJ |
| O, `,` `.`, `;` `'`, V | blast at the aim point; filler 0.1–5 kg; standoff 0–5 m; cased or bare |
| J | spall: today or the candidate |
| K / N | other cut (running bond, jittered) / new wall |
| G | camera: wall front, wall back, witness panel |
| LMB, E/Q, C, R, H, Space, Up/Down | the range's own keys (H lists them all) |

The readout (top left, shown while the camera is within 12 m of the wall) states the active law's
formula, the material values, the last deposit's outcome, each closed-form law's crack reach for
that energy, the blast's energy budget and coupling, the first structural failure, and the spall
today against the candidate. Its `blast [O]` line is the tool's current setting; the `last:` lines
are the deposit that happened.

Code: `src/sandbox/wall.rs` (the sandbox side), `src/sandbox/wall/model.rs` (removal, laws,
support, structural check; reads only the chunk table and the joint graph), the `Brick` row in
`assets/materials/materials.ron`.

## Captures

`cases/<law>-<case>-<mode>.jpg`: front and back, before and after, one 2×2 per run. `mode` **L** is
the local response alone (structural check off), **S** local plus structural. `sheet-<law>.jpg`:
every after frame of one law, local beside local+structural.

| case | what |
|---|---|
| `ap`, `apcr`, `mg50` | 8.8 cm Pzgr. 39 (solid), 8.8 cm Pzgr. 40, 50 × 7.92 mm S.m.K., fired at the chunk nearest the wall's centre from 3.8 m |
| `p100k`, `p1m`, `p5m` | point deposit of 100 kJ, 1 MJ, 5 MJ on the face (no push, so no S run) |
| `x<kg>-<m>` | cased blast of 0.1, 1, 5 kg filler at 0, 1, 3 m standoff |
| `B-ap-spall-today`, `-candidate` | the AP under law B, last frame from behind the witness panel |
| `*-bare-S` | the same blast with no casing |
| `*-jit-*` | the jittered cut: the laws unchanged on a cut that is no grid |

Colours: chunk heat brick to yellow as the pool empties; local cracks cyan, structural magenta; a
group a local crack freed grey-blue, one a structural crack freed purple; blast rays pale yellow;
exit cones orange (today) or pale blue (candidate); witness tiles yellow to red a decade of dose
per step from 1 J.

Reproduce one: `OVERMATCH_MUTE=1 BEVY_ASSET_ROOT=<repo> OVERMATCH_CAPTURE=out.png
OVERMATCH_CAPTURE_AT=3,4,11.5,12.5 OVERMATCH_CAPTURE_SIZE=1920x1080
OVERMATCH_CAPTURE_STEP_S=0.0333333 OVERMATCH_WALL_CASE=B,x1@0 target/debug/armor_sandbox`
(`OVERMATCH_WALL_STRUCTURAL=off`, `OVERMATCH_WALL_CASING=bare`, `OVERMATCH_WALL_CUT=jittered`,
`OVERMATCH_WALL_SPALL=candidate`, `OVERMATCH_WALL_VIEW=witness` as needed), after
`scripts/serial-cargo build --bin armor_sandbox --features dev_ui,bitprobe`. The window stays
hidden. Every run's numbers, read off its log, are in `outcomes.md` (MEASURED).

## The wall

4 × 2 m, 0.25 m thick, cut in a running bond of 0.2 m chunks through the full thickness: 205 chunks,
575 joints, 20 ground anchors (MEASURED). A chunk is 0.01 m³ and 18.5 kg; the wall 3,690 kg
(DERIVED). The laws read only the chunk table (volume, centroid, mass, material) and the joint graph
(pair, contact area, centroid, normal; anchors); the generator alone knows the pattern. A second
generator (K, `*-jit-*`) jitters courses and seams, and its joints come from shared faces; the tests
renumber its chunks and check that no outcome moves.

## Material values

| value | used | source | range | label |
|---|---|---|---|---|
| brick density ρ | 1845 kg/m³ | Engineering ToolBox, "Bricks - Densities" (London stock) | 1765–2165 (red facings to engineering) | CLAIMED, read |
| brick compressive strength f_c | 36.5 MPa | BIA Technical Note 3A (2024) Table 1, molded solid brick mean (s.d. 12.6) | molded 24–49 (±1 s.d.); extruded 77.9; fire clay 105.8 | CLAIMED, read |
| `Brick` resistance factor | 36.5 ref-mm/m | f_c / 1 GPa × 1000, Wood's derivation | 24–106 with f_c | DERIVED |
| crush energy w | 135.5 MJ/m³ = 3.711 f_c | the walk's own static resistance at that factor: factor/1000 × a, a = 3.711 GPa the reference cavity strength | 88–182 with f_c (±1 s.d.) | DERIVED |
| chunk pool | 1.355 MJ | w × 0.01 m³ | | DERIVED |
| joint tensile strength f_t | 0.69 MPa | BIA TN 3A Table 7, modulus of rupture normal to bed joints, solid units, portland cement-lime Type N | 0.35 (masonry cement N) – 0.92 (M or S) | CLAIMED, read |
| joint fracture energy G_f (mode I, brick–mortar interface) | 10 J/m² | van der Pluijm's couplet tests, as Lourenço (1996) restates them | 5–20 | CLAIMED, not read (geometric mean taken) |
| masonry modulus E_m | 8.6 GPa | TMS 402 E_m = 700 f'm (BIA TN 3A eq. 1); f'm = 12.3 MPa by TN 3A Table 5 for 36.5 MPa units, Type N | 700–1200 f'm: 8.6–14.7 GPa | DERIVED |
| wave speed c = √(E_m/ρ) | 2159 m/s | | | DERIVED |
| filler chemical energy | 4.184 MJ/kg | the TNT-equivalent convention (NIST SP 811: 1 ton TNT = 4.184 GJ) | | CLAIMED; ASSUMED to stand for the seed round's filler |
| casing per kg filler M/C | 24.0 | the seed round's burst (v_s 541.5 m/s, 211.1 kJ driven) and its core's K = 2680 m/s: M/C = (K/v_s)² − ½ | | DERIVED |
| burst spread of the 7.92 mm burst | 3 mrad | no gun file states dispersion | | ASSUMED |
| crack share η (law C) | 0.01 | "about 1 % of comminution energy ends as new surface" | | ASSUMED (CLAIMED in comminution texts, not read) |

`w` cross-check: 135.5 MJ/m³ is 73 kJ/kg, the order of the Bond work index for grinding rock to
~100 µm (10–20 kWh/t, 36–72 kJ/kg; CLAIMED, not read). Brick unit, not masonry assemblage
(12.3 MPa): a round's line crosses brick far more than mortar.

## Removal

A chunk turns to dust when the energy in it reaches `w × V`; partial deposits wear the pool.

- **Line deposit** (a round): the energy the walk says the round lost in each chunk
  (`VolumeCrossing::energy`) wears that chunk.
- **Point deposit**: the ball holding `E / w` m³ of material about the point is crushed; each
  chunk pays its overlap with the ball (each chunk taken as the ball of its own volume).
- **Blast**: each chunk takes the energy of the rays that stop on it.

**Verdict: in range, but the chunk is coarser than the crushed zone.** No chunk turned to dust in
any brief case (MEASURED). The 88 mm AP leaves 278.0 kJ in one chunk, 21 % of its pool (MEASURED);
its crushed radius is √(ε/πw) = 5.1 cm against the round's 4.4 cm (DERIVED), so the hole is real but
smaller than a 20 cm chunk. A 5 MJ point crushes 3.7 chunk volumes (DERIVED) spread over 11 chunks
(MEASURED), none fully. The first dust needs a bare 5 kg contact charge (MEASURED, `D-x5-0-bare-S`).
No material value is off; what is missing is a representation rule (a finer cut near a hit, or
removing a chunk once a stated share of it is crushed). That is an owner decision, not a tuning.

## Crack laws

Every deposit becomes point sources: a point deposit is one; a round's chord in each chunk is
split every 5 cm; a blast gives one per chunk it reaches, at its rays' energy-weighted hit point.
Each source of energy E at distance d from a joint. Loads superpose; a joint does not remember a
load below its threshold. Stress laws take the mean normal traction over a 7-point disc of the
joint's area.

| law | formula | reach of a lone point (DERIVED) at 100 kJ / 1 MJ / 5 MJ |
|---|---|---|
| A energy flux | E / (4π d²) ≥ G_f | 28 m / 89 m / 199 m |
| B stress | 3E/(8π d³) (1 − 3 cos²θ) ≥ f_t | 0.26 m / 0.56 m / 0.95 m |
| C energy share | η E / (4π d²) ≥ G_f | 2.8 m / 8.9 m / 20 m |
| D thin wall | B + 3E/(4π d′³) cos²θ′ ≥ f_t | depends on the back face |
| E Griffith | B's stress ≥ √(E_m G_f / (π a)), a = √area / 2 | 0.29 m / 0.62 m / 1.06 m (head joint) |

**B's spreading law.** The deposit crushes a ball whose wall holds the crush pressure P = w (work
P·dV = energy), radius r_c with P r_c³ = 3E/4π. Outside it the elastic (Lamé) field of a pressurised
spherical cavity: radial −P(r_c/d)³, hoop +½P(r_c/d)³. θ is the angle between the joint's normal
and the radius, so a joint facing the source is squeezed and one lying along the radius is pulled
(radial cracks). Note that P r_c³ = 3E/4π: the far field depends on the energy alone, not on the
brick's crush strength (DERIVED). A chain of sources along a chord gives ¾ of the cylindrical Lamé
field ε/(π d²) (DERIVED).

**D's reflection.** The source's mirror image across the free back face (found along its push by
a ray cast; b its distance) sends the radial push back as tension, magnitude 3E/(4π d′³) along the
image's radius. Thickness enters through b.

### Verdicts

- **A: off-range by four orders.** Every deposit, down to one 7.92 mm bullet (2.3 kJ), cracks all
  575 joints and the whole wall comes apart (MEASURED, every A run). To hold a 278 kJ deposit's
  cracks to 0.5 m, G_f would have to be 88 kJ/m², ~9,000× the handbook's 10 J/m² (DERIVED).
  **Missing factor:** energy flowing past a joint is not a load on it. A crack needs the stress at
  the joint, or the elastic energy stored there, not the flux through it.
- **C: off-range, and the wrong form.** η = 0.01 still cracks the whole wall for every round and
  every deposit of 100 kJ or more (MEASURED). The share that would match B's reach is
  4π G_f r_B²/E: 8.5e-5 at 100 kJ, 3.9e-5 at 1 MJ, 2.3e-5 at 5 MJ (DERIVED). It falls as E^(−1/3),
  so no constant share fits. **Missing factor:** the same as A; "most energy becomes heat" is true
  but is not the factor that limits crack reach.
- **B: in range.** 88 AP 6 joints, APCR 2, 7.92 mm none, points 2 / 22 / 56 joints, blasts 0–18
  (MEASURED). The cracks are radial (a cross about the hit). **But B never frees a piece:** 0 groups
  fell in every B run without the structural check (MEASURED). Radial cracks leave every chunk
  bonded to its neighbours along the circumference, and the struck chunk's own joints are squeezed.
  **Missing factor:** a mechanism that cuts around the hit (circumferential cracks, or crushing at
  chunk scale). D was meant to supply it.
- **D: in range, thickness enters, but it frees almost nothing with through-thickness chunks.** It
  adds cracks where the back face is near (5 MJ point: 72 joints against B's 56; MEASURED), yet the
  reflected pull is mostly normal to the wall's face, and no joint here lies in that plane: every
  chunk spans the full thickness. **Missing factor:** a cut with seams parallel to the face (two
  wythes, or chunks that stop short of the back), so a back-face layer can break out. That is a cut
  decision, not a material value.
- **E: in range, and it agrees with B.** The Griffith strength from handbook E_m and G_f, with the
  joint's own size as the flaw, is 0.49–0.59 MPa on this cut (DERIVED), inside the handbook f_t band
  0.35–0.92 MPa. Results track B (1 MJ: 28 joints against 22; MEASURED). So the toughness and the
  strength the handbooks give are one property once the flaw size is the joint size, and E makes
  joint size matter: larger joints are weaker.

## Blast (no direction)

- **Energy:** chemical − casing = C × 4.184 MJ/kg − ½ (M/C · C) v_s². With M/C = 24.0 and v_s =
  541.5 m/s (the seed round's burst law), the casing takes 3.518 MJ per kg of filler, leaving 0.666
  MJ/kg cased, against 4.184 MJ/kg bare (DERIVED).
- **Rays:** 4096 directions, the field machinery's ball at ratio 0 (uniform sphere,
  `spall_cone::ConeShape::spread`). Each ray carries 1/4096 of the energy and momentum and stops on
  the first chunk it meets. It never crosses brick.
- **Coupling from geometry alone:** about half at contact, 47–49 %, with the charge's own radius
  and the wall's edges taking the rest. 20–21 % at 1 m, 5.4–5.5 % at 3 m (MEASURED).
- **Push:** one textbook relation, K = p²/2m (kinetic energy of a mass; any first-year mechanics
  text), so p = √(2 C E) with m the filler's mass, the detonation products. ASSUMED: the products
  carry the momentum, and air swept up conserves it. Each ray stops inelastically: Δp = p_ray. A gas
  reflecting off the wall would deliver up to twice that, so this is a lower bound. Cased: 115 /
  1154 / 5771 N·s for 0.1 / 1 / 5 kg; bare 1 kg 2893 N·s (DERIVED).
- **Rounds push too:** the penetrator's mass × the speed it lost across the wall: 368 N·s for the
  88 AP (DERIVED from the walk's 278 kJ). Point deposits carry no push.

Local response to blasts: crushing never empties a chunk below a bare 5 kg contact charge
(MEASURED). B, D and E crack 0–24 joints, and only within a chunk of contact.

## Structural check

At each section (a joint alone, or every live joint in one plane), take the group that section
alone holds to the ground. The pushes on that group give an angular impulse about the section's
centroid; its in-plane part H bends the section. The section fails when

H ≥ √(2 I G_f A)

- I is the group's moment of inertia about the bending axis.
- A is the section's area.

In words: the group's rotational energy H²/2I reaches the energy it takes to open the section,
G_f A. It generalises the trees' ram law (impulse × lever against a threshold). Here the threshold
comes from handbook toughness and the group's own mass, with no constant. The worst failing section
cracks first, and the check repeats. The strength view is ~36× smaller than G_f A at the base
(f_t²/2E_m over a 10 mm joint, DERIVED), so toughness governs.

**Verdict: in range; the open factor is the momentum share.** The only section that fails on an
intact cantilever wall is its base: 20 anchors, A = 1 m², capacity 314 N·s·m (MEASURED). These fail
it:

| case | H (N·s·m) |
|---|---|
| 88 AP | 406 (the wall rocks and stands) |
| 1 kg cased at contact | 317 |
| 5 kg cased at contact | 1577 (the wall topples) |
| 5 kg cased at 1 m | 948 (the wall topples) |
| bare 1 kg at contact | 794 |
| bare 5 kg at 3 m | 732 |

APCR, the 7.92 mm burst, every 0.1 kg charge, 1 kg at 1 m or more, and 5 kg cased at 3 m hold
(MEASURED).

A second textbook form agrees, so this is not an artefact of the energy view. The wall's first
cantilever mode is ω₁ = 137 rad/s, and the base's moment capacity f_t Z = 28.7 kN·m. Together they
put the failing angular impulse at f_t Z / ω₁ = 210 N·s·m (DERIVED), the same order as 314.

**Missing factor:** how much of a round's lost momentum the wall keeps. The walk credits all of it
to the struck chunk, but the brick it crushes leaves the back as ejecta and carries a share away.
That share decides whether one 88 mm slug cracks a free-standing wall's base.

Local against structural: in the `-L` and `-S` pairs, the local laws leave the wall standing with
crack patterns. The structural check is the only path here that brings a wall down, as one purple
body.

## Spall toggle

Today the exit cone is 0.88 × the energy lost. The substance sets only its angle. The candidate
scales that by the exited substance's strength relative to armour steel, capped at 1: 0.0365 for
brick.

For the 88 AP: today S = 244.6 kJ (MEASURED), candidate 8.9 kJ (DERIVED). The witness panel 4.4 m
behind the wall took 73.3 kJ today against 2.7 kJ (MEASURED, `B-ap-spall-*`). The game's own dose
is untouched: both are computed off the shell's record.

## Not done

- **The free-standing steel plate** (the optional third addition): not cheap in this pass. It
  needs a second structure, with steel's crush energy and a fine cut, in the ray and walk paths.
  The local model would carry it unchanged.
- **Fatigue:** a joint forgets a load below its threshold.
- **Shear failure of joints, and gravity's precompression** (36 kPa at the base, DERIVED, 5 % of
  f_t): not modelled.
