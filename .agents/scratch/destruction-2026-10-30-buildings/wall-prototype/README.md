# Wall prototype: the push rule on a baked brick wall, handbook values only

Subject: the game's environment destruction, as game state only. A wall chunk is **turned to dust**
(removed) or **cracked loose** (its joints broke); rounds and blasts **dose** what they cross.

Labels: **MEASURED** (read off this prototype's runs), **DERIVED** (computed here from other
numbers), **CLAIMED** (stated by a source; "read" when the source was read this session),
**ASSUMED** (a choice with no source).

## Run it

```
cargo armor          # = cargo run --bin armor_sandbox --features dev_ui
```

The wall stands to the right of the range (x 13-19 m, z 4 m). Press **G** to stand in front of it,
**H** for the key and colour help.

| Key | Does |
|---|---|
| B / F | pick the round (88 mm AP, 88 mm APCR, 88 mm APHE, 7.92 mm) / fire it (an MG fires a burst of 50) |
| O, `,` `.`, `;` `'`, V | blast at the aim point; filler 0.1-5 kg; standoff 0-5 m; cased or bare |
| J | spall: today or the candidate (still open) |
| G / N | camera: wall front, back, witness panel / new wall |
| H | the help panel |
| LMB, E/Q, C, R, Space, Up/Down | the range's own keys |

The readout (top left, shown while the camera is within 12 m of the wall) is one line: the last
shot and its energy. Every value of a resolved shot (material, blast budget and coupling, first
structural failure, spall today against the candidate) is in the log.

Code: `src/sandbox/wall.rs` (the sandbox side), `src/sandbox/wall/baked.rs` (selects and places the
bake's wall), `src/sandbox/wall/model.rs` (removal, the push rule, support, structural check; reads
only the chunk table and the joint graph), the `Brick` row in `assets/materials/materials.ron`.

## Captures

`cases/B-<case>-<mode>.jpg`: front and back, before and after, one 2x2 per run, on the earlier
hand-cut wall (4 x 2 m, a running bond of 0.2 m chunks: 205 chunks, 575 joints, 20 ground anchors,
MEASURED), before the sandbox read the bake's cut. `mode` **L** is the local response alone, **S**
local plus structural. Their numbers are in `outcomes.md`; `readout/` holds the one-line readout and
the help panel on the baked wall.

| case | what |
|---|---|
| `ap`, `apcr`, `mg50` | 8.8 cm Pzgr. 39 (solid), 8.8 cm Pzgr. 40, 50 x 7.92 mm S.m.K., fired at the chunk nearest the wall's centre |
| `x<kg>-<m>` | cased blast of 0.1, 1, 5 kg filler at 0, 1, 3 m standoff |
| `B-ap-spall-today`, `-candidate` | the AP, last frame from behind the witness panel |
| `*-bare-S` | the same blast with no casing |

Colours: chunk heat brick to yellow as the pool empties; local cracks cyan, structural magenta; a
group a local crack freed grey-blue, one a structural crack freed purple; blast rays pale yellow;
exit cones orange (today) or pale blue (candidate); witness tiles yellow to red a decade of dose
per step from 1 J.

Reproduce one: `OVERMATCH_MUTE=1 BEVY_ASSET_ROOT=<repo> OVERMATCH_CAPTURE=out.png
OVERMATCH_CAPTURE_AT=12.5 OVERMATCH_CAPTURE_SIZE=1920x1080 OVERMATCH_CAPTURE_STEP_S=0.0333333
OVERMATCH_WALL_CASE=88mm_AP target/debug/armor_sandbox` (`x1@0` for a blast;
`OVERMATCH_WALL_CASING=bare`, `OVERMATCH_WALL_SPALL=candidate`, `OVERMATCH_WALL_VIEW=witness`,
`OVERMATCH_WALL_HELP=1` as needed), after
`scripts/serial-cargo build --bin armor_sandbox --features dev_ui,bitprobe`. The window stays
hidden.

## The wall

The east wall of the baked house (`assets/buildings/house_1/house_1.building.ron`): its brick
elements (piers, sills, lintels around three windows), 6 m wide, 2.9 m tall, 0.25 m thick, 254
chunks and 619 joints (MEASURED, from the sandbox log). The bake's convex chunks are the colliders
and the drawn pieces; its joints and ground anchors are the graph. The wall's joints to the
house's other walls and its wooden plate and roof are dropped. The law reads only the chunk table
(volume, centroid, mass, material) and the joint graph (pair, contact area, centroid, normal;
anchors); the tests renumber the chunks and check that no outcome moves.

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

`w` cross-check: 135.5 MJ/m³ is 73 kJ/kg, the order of the Bond work index for grinding rock to
~100 µm (10–20 kWh/t, 36–72 kJ/kg; CLAIMED, not read). Brick unit, not masonry assemblage
(12.3 MPa): a round's line crosses brick far more than mortar.

## Removal

A chunk turns to dust when the energy in it reaches `w × V`; partial deposits wear the pool.

- **Line deposit** (a round): the energy the walk says the round lost in each chunk
  (`VolumeCrossing::energy`) wears that chunk.
- **Blast**: each chunk takes the energy of the rays that stop on it.

**Verdict: in range, but the chunk was coarser than the crushed zone on the hand-cut wall.** No
chunk turned to dust in any brief case there (MEASURED). The 88 mm AP leaves 278.0 kJ in one chunk, 21 % of its pool (MEASURED);
its crushed radius is √(ε/πw) = 5.1 cm against the round's 4.4 cm (DERIVED), so the hole is real but
smaller than a 20 cm chunk. The first dust needed a bare 5 kg contact charge (MEASURED).
No material value is off; what is missing is a representation rule (a finer cut near a hit, or
removing a chunk once a stated share of it is crushed). That is an owner decision, not a tuning.

## The push rule

Every deposit becomes point sources: a round's chord in each chunk is split every 5 cm; a blast
gives one per chunk it reaches, at its rays' energy-weighted hit point. Loads superpose; a joint
does not remember a load below its threshold. The load is the mean normal stress over a 7-point
disc of the joint's area from each source of energy E at distance d, and the joint cracks when it
reaches the joint's tensile strength f_t:

3E/(8 pi d^3) (1 - 3 cos^2 theta) >= f_t

The deposit crushes a ball whose wall holds the crush pressure P = w (work P dV = energy), radius
r_c with P r_c^3 = 3E/4pi. Outside it the elastic (Lame) field of a pressurised spherical cavity:
radial -P (r_c/d)^3, hoop +1/2 P (r_c/d)^3. theta is the angle between the joint's normal and the
radius, so a joint facing the source is squeezed and one lying along the radius is pulled (radial
cracks). P r_c^3 = 3E/4pi: the far field depends on the energy alone, not on the brick's crush
strength (DERIVED). A chain of sources along a chord gives 3/4 of the cylindrical Lame field
E/(pi d^2) (DERIVED). A lone point's reach is (3E/(8 pi f_t))^(1/3): 0.26 m / 0.56 m / 0.95 m at
100 kJ / 1 MJ / 5 MJ (DERIVED).

**Verdict: adopted.** On the hand-cut wall: 88 AP 6 joints, APCR 2, 7.92 mm none, blasts 0-18
(MEASURED). The cracks are radial (a cross about the hit). Radial cracks alone free nothing: 0
groups fell in every run without the structural check (MEASURED), so the structural check is part
of the design.

**Provenance.** Four other forms were run against it on that wall and set aside: energy flux
against G_f (cracked all 575 joints for every deposit, four orders off range), a 1 % share of that
flux (the same, and no constant share fits), B plus the push reflected off the back face (it adds
cracks near the back face and frees almost nothing through-thickness), and Griffith's strength
sqrt(E_m G_f / (pi a)) with the joint's own size as the flaw, which agrees with this rule (0.49-0.59 MPa
on that cut against f_t 0.69 MPa; a lone 278 kJ point reaches 0.41 m against this rule's 0.36 m, DERIVED).
The handbook toughness and strength are one property once the flaw size is the joint size.

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
  88 AP (DERIVED from the walk's 278 kJ).

Local response to blasts: crushing never emptied a chunk below a bare 5 kg contact charge on the
hand-cut wall (MEASURED). The rule cracks 0-18 joints, and only within a chunk of contact.

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

**Known gap:** the check ignores the wall's own mass (a later fix). **Open factor:** how much of a round's lost momentum the wall keeps. The walk credits all of it
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
