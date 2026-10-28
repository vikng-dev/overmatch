# Design sketch: armor penetration & module damage (the ballistics simulator)

**Status: SPEC for the in-progress build (2026-06-27).** Decided in a design interview; being
implemented now, starting with an isolated sandbox (§11). Graduates to ADR(s) if it survives
contact — the `ballistics`-as-library-plugin + `shooting` split (§10) is the most ADR-worthy part.
Vocabulary from `.agents/GLOSSARY.md` and `.agents/skills/codebase-design` (seam, depth, leverage).

This is the next vertical-slice mechanic after the gunner's sight (`design/gunner-sight.md`). It is
deliberately deep and physically-grounded; §9 names what is *out* of the first slice so the depth
doesn't masquerade as scope.

## 1. The knockout model — crew is the only health

There is **no tank HP**. A tank is knocked out when it has **no active crew** — the count threshold
("fewer than 2 = knocked out") is **deferred pending playtest** (see §1a below). Everything else is
emergent and *repairable* — only ammunition is terminal. Three paths empty crew:

- **Direct hits** — a penetrator or spall fragment reaching a crewman.
- **Engine fire** — damages nearby crew slowly, by proximity, over time.
- **Ammo cookoff** — detonation of an ammunition volume; **instantly knocks out all crew**.

Module damage (engine, breech, optics, transmission, …) *never* knocks the tank out — it degrades
capability and can be repaired. Ammunition is the one exception.

### 1a. Knockout threshold — A/B under test (2026-06-29)

(Tracked as a playtest fork: `.agents/scratch/playtest-forks/README.md` (F2).)

**Background.** Originally (this §1) a tank was knocked out at fewer than 2 active crew — the "a one-man
tank is operationally finished" rationale. Implementing per-position capability gating (§7) exposed
a tension: the backfill-slice gameplay we want (a lone survivor choosing which single position to
staff — drive OR shoot, not both) is *only possible* if a lone-crewman tank is still in action. So the
<2 threshold gates off the backfill tradeoff before it can exist.

**v1 cut (chosen for now, pending playtest): pure per-position gating, no crew-count KO.**
Capability loss is derived entirely from per-position staffing (§7): `capability_available =
staffed AND module HP > 0`. A tank with one surviving Driver can still drive; a tank with one
surviving Gunner can still traverse/fire/reload (but not move or see); etc. `TankKnockedOut`
becomes a *readout/label* only, never a gameplay gate. A tank is fully knocked out only at **0 active
crew** (every position unstaffed) — a separate condition from a count threshold.

**A (this cut) vs B (the old <2 rule):**
- **A — pure per-position (chosen for v1):** enables the backfill-slice tradeoff; smallest
  primitive; KO gate can be re-added as one AND-clause if A feels wrong in playtest.
- **B — <2 crew = knocked out:** tanks are knocked out earlier; lone survivors can't act; backfill
  tradeoff never arises because the tank is already knocked out at the moment a second crewman is
  knocked out.

**What A means for the existing wiring:**
- `function_disabled` (module HP gate) stays as-is.
- A crew-staffing gate is *added* alongside it (§7): capability is on iff position staffed by an
  active crewman AND module HP > 0. Both must hold; they compose.
- `TankKnockedOut` is **retired as a gameplay gate** (no system reads it to disable drive/fire).
  It may survive as a derived label for HUD/scoring, or be removed entirely pending slice-2
  needs. Cookoff (§8) still knocks out all crew and launches the turret — that hook moves from
  `TankKnockedOut`-triggered to `CookedOff`-triggered directly.
- **If A plays wrong in testing**, B is re-added as: `capability_available AND tank not KO`, where
  KO latches at active crew < 2. One AND-clause, mechanical revert.

**What knocks a tank out under A:** only 0 active crew. (A "combat-ineffective" derived label — no
combat capability remains, or no drive AND no fire — may be added later for scoring; it is *not* a
gameplay gate in v1.)

## 2. The unified primitive — the ballistic volume

There is no real distinction between an "armor plate" and a "module": both are a **watertight solid
mesh + a material** (density/hardness → a *material factor*) that taxes a penetrator over the
line-of-sight distance through it. The Tiger's upper front plate is a thin, dense, high-cost slab;
the engine is a big low-density box that only stops a round if the path chews through enough of it.
Same primitive.

- A **module** = a ballistic volume that *also* carries function + state (engine, ammo, breech).
- **Crew** = a ballistic volume that can be incapacitated.
- Every module/crewman is a ballistic volume; not every ballistic volume is a module (bare plates
  have no function to lose). The march reads the *volume* layer (cost only); the consequence step
  reads the *module/crew* layer — "same geometry, consumed by type" (ADR-0008).

**Geometric thickness, not a slope coefficient.** Thickness is *measured* from the solid: the path
enters the front face and exits the back face, and the distance through the solid **is** the
line-of-sight thickness. Slope is free (a sloped plate is geometrically longer); no `cos` term.
Friend is modelling the Tiger's armor as solid volumes now.

**Convexity is not required; watertight/manifold is.** ADR-0008's convex constraint is a *physics
solver* requirement on the dynamic collision proxy. The armor layer is read by our **penetration
raycast** — a static-frame spatial query — and ray-vs-triangle works on any geometry. What replaces
convexity is **manifold/watertight**, so entry/exit faces pair cleanly and "inside the solid" is
well-defined.

## 3. The penetrator march — velocity is the source of truth

> **Resolution model superseded by §13 (2026-08-03).** The laws below (capability, cost,
> perforate/embed, residual velocity) stand unchanged — but "per volume crossed" as a *serial,
> one-entity-at-a-time* resolution is retired. §13 replaces it with the union field walk:
> overlapping volumes behave as their boolean union, and cost/damage/spall become field + boundary
> laws over the material-factor field. Implement crossings from §13, magnitudes from here.

For a fixed round (mass, calibre, nose factor), penetration is a monotonic, invertible function of
velocity. So **velocity is the stored state; penetration *capability* is its derivative.** The
march, per volume crossed:

1. `capability = f(round, velocity)` — reference-mm this shell can defeat right now, by the
   force-law (Poncelet) form:

   ```
   A        = π c² / 4                              presented area (m²), c = calibre
   k        = B · N · RHO_REF                       inertia coefficient (kg/m³)
   depth(v) = m / (2 A k) · ln(1 + k v² / a)        metres of reference steel still to go
   C        = 1000 · depth(v)                       capability, reference-mm
   v(C)     = sqrt((a / k) · (exp(C/1000 · 2 A k / m) − 1))
   ```

   No fitted constant. The reference steel is armour-class rolled homogeneous steel, given by its
   material facts: flow stress `Y = 1.0e9 Pa` (mean of proof stress and tensile strength), Young's
   modulus `E = 205e9 Pa`, Poisson's ratio `ν = 0.29`, density `RHO_REF = 7850 kg/m³`
   (`.agents/scratch/damage-cam-2026-10-08/research/hardness-anchor-free.md` Q2). The strength
   term `a` (Pa) is the compressible elastic–perfectly-plastic cavity-expansion term
   `a = (2/3) · (1 + ln(E / (3 (1 − ν) Y))) · Y ≈ 3.711e9 Pa` (DERIVED): a prior from material
   facts, not validated against thick-steel depth data, and the compressible form was read in
   restatement, not in its primary. `B = 1` is the target inertia factor, a named approximation
   weakly constrained by the available plate data. The fixture rounds at muzzle carry about
   115 reference-mm (main gun, 773 m/s) and 14.5 reference-mm (machine gun, 755 m/s), DERIVED.
   The nose factor `N` is the frictionless nose factor of the cavity-expansion form, in `(0, 1]`.
   It is **round data authored as geometry**: each round file (`assets/rounds/<key>.round.ron`) carries
   a required `nose` (parameters in calibres), and `N` is derived once at load
   (`spec::NoseShape::factor`) — `Ogive(crh: ψ)`, `ψ ≥ 0.5`: `N = (8ψ − 1) / (24ψ²)`;
   `Cone(length_calibres: φ)`, `φ > 0`: `N = 1 / (1 + 4φ²)`; `Cap(radius_calibres: φ)`,
   `φ ≥ 0.5`: `N = 1 − 1 / (8φ²)`; `Flat`: `N = 1`. A parameter outside its range fails to load.
   The derived scalar travels with mass and calibre through `FireShell` and `Projectile`; the fire
   wire (`FireEvent`) carries the round's index in its weapon's `rounds` list, and every role binds
   the round from its own data. The shape never leaves load. Fixture noses are labelled:
   `Ogive(crh: 1.0)` for the main-gun rounds, `Cap(radius_calibres: 0.5)` (a hemisphere,
   `N = 0.5`) for the machine-gun round. Calibre enters raw penetration through `A`: at equal mass
   a wider round presents more area and goes less deep. The law's invariants: strictly increasing
   in speed; replica scaling (mass ×s³, calibre ×s gives capability ×s); the local speed exponent
   is 2 at low speed (the inertia term is negligible) and falls below 2 as it grows; and spending
   composes, because a crossing spends reference-mm and the inverse is exact. A crossing whose
   limit speed is `v_l` leaves `v_r² = (v² − v_l²) / (1 + k v_l² / a)`.

   **A sub-calibre round flies whole and meets the plate as its penetrator** (owner, 2026-10-23;
   `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/decision-2026-10-23.md` #4).
   A round may author a `penetrator` (mass and calibre; its nose is the round's), no heavier and
   no wider than the round; without one the round is its own penetrator, bit for bit. Flight reads
   the whole round: drag (`drag_k`) and the shell's view. Everything at the plate reads the
   penetrator: the disc the shell samples the world with, overmatch, this law and the cone's hole
   (§5). The momentum a hit or a bounce delivers is the whole round's: the body stops at the plate
   with its penetrator. Penetration stays rigid: the penetrator's
   material is named (`core`) and read by no law. The 88's Pzgr. 40 reads about 180
   reference-mm at the muzzle (930 m/s) as its 1.93 kg, 35.7 mm core, against about 112 as its
   whole 7.3 kg body and the Pzgr. 39's about 115 (DERIVED, shipped RHA facts); the record gives
   it more than the Pzgr. 39 at the muzzle by about 1.4× (Jentz & Doyle), the law 1.57×.
2. `cost = LOS_distance × material_factor` (× angle effects), also reference-mm.
3. If `capability > cost` → **perforate**: spend `cost`, invert `f` back to a reduced **residual
   velocity** (the Lambert–Jonas shape — barely-penetrate exits slow, big overmatch barely slows),
   and throw spall (§5).
4. Else → **embed** and stop.

Properties:

- **Modules tax the penetrator too** — they are volumes. A shell can run out of steam *inside* the
  tank after crossing the outer plate plus an engine block.
- **The path is a multi-segment world-space ray** that bends (normalization) and deflects
  (ricochet). A segment can leave one tank and strike another (skip off a glacis into a neighbour).
- **Deliberate omission:** *shatter* (penetration going non-monotonic at extreme velocity against
  hard armor) is out of the slice.

## 4. Boundary interaction — the decision tree at each face

> **Amended by §13.3 (2026-08-03):** overmatch and ricochet consult **factor-weighted**
> (steel-equivalent) thickness, not geometric thickness, and surface behaviour gates on a
> significant factor *step* at the boundary — otherwise a thick soft volume (a forearm, a fuel
> tank) deflects main-gun rounds. The decision tree's structure is unchanged.

When the path reaches a volume's face:

1. **Impact angle** = angle between path and surface normal at the hit point.
2. **Overmatch** — if caliber ≥ ~k× the volume's thickness *along its normal* at that point: suppress
   ricochet, normalize almost fully, punch through (cost still applies). The game's namesake, but
   **one modifier among many — not the centerpiece.** Don't over-build it.
3. **Ricochet** — if not overmatched and impact angle exceeds the **per-shell-type ricochet
   threshold** (a fixed constant for now; we have one shell): **deflect** — spawn a new path segment
   off the face, bleed velocity, do *not* enter. **No spall on ricochet** for now (spall is an
   exit/perforation event only — §5).
4. **Otherwise** — **normalize** (bend the path toward the normal), enter, and march the geometric
   LOS cost; perforate→exit (spall) or embed.

The *structure* is the design; the magnitudes (k, ricochet angle, normalization degrees, material
factors) are **live knobs the sandbox exists to tune** — built as adjustable
constants, not baked.

## 5. Spall — the spall cone

Spall is a **field**: strength over directions from an origin, stopped by the hard bodies it meets,
dosing the boxes it reaches (glossary: *Field*, *Spall cone*). Adopted by the owner 2026-10-12;
decision record `.agents/scratch/damage-cam-2026-10-08/product/in-battle/owner-direction-2026-10-10.md`
("The dose model"), evidence in `research/cloud-falloff.md`, `research/cloud-angle-deep.md` and
`research/codex-cloud-angle/verdict.md` beside it.

**Which steps throw.** A cone is thrown at every **body exit** the round reaches, sized by that
exit's own spend and speeds, whatever the round does afterwards. A body is a maximal stretch of
hard material along the path (a witness, §6, is no material of the walk) (welded voids inside it included) over which no layer resists less than
the one before it. It exits where the next medium resists less — a lower factor, or air — and the
next body starts there. A step to an equal or higher factor continues the body without a throw,
whatever the substances: plates of equal factor in contact are one plate and throw once at the far
face with their summed spend (owner, 2026-10-12), and a step up into harder material adds what is
crossed behind it to the same body, since no softer step intervened. Exits reached before the round
stops still throw; the body the round stops inside throws nothing. A bounce throws nothing.

**The HP quantum gate.** A cone whose whole emission `S/2` converts to no HP point (below half a
point, §6) is not thrown: no direction is walked and nothing is recorded (owner, 2026-10-17). `S/2`
is the most any one box can take from a cone — every direction doses a box at most once, and the
directions' weights emit `S/2` (**Sampling**) — so a gated cone could not have cost any box a
point. The gate adds no constant: it is the HP quantum.

**Strength.** `S = spend × κ × (v_exit / v_entry)² × (d / d_ref)²` (reference-mm; the cone emits
`S/2` in all, **Sampling**).
`spend` is what the round's mechanism spent crossing that body (its readout drop, η-weighted by
the disc); `v_entry`, `v_exit` are the round's speeds entering and leaving it. `κ = f_rear / f_ref`
is the debris competence of the exit material (owner, 2026-10-17): `f_rear` the resistance factor
of the body's last layer before the exit — its highest, since a body never steps down — and
`f_ref` the reference factor (RHA, 1000). κ is exactly 1 for the reference, so a steel exit's `S`
keeps its bits; every other material scales by its own data, and soft bodies fall to noise on
their own (`.agents/scratch/damage-cam-2026-10-08/research/debris-strength-law.md`). A
barely-through round has a small energy ratio. `(d / d_ref)²` is the **hole term** (owner,
2026-10-23): the cone's budget goes with the area of the hole, `d` the calibre of the round's
penetrator (§3) and `d_ref` the calibre the field's rate was set against (`reference_calibre_m`
in `assets/ballistics/spall_cone.ron`, 88 mm, TUNED). The exponent 2 is the law (a plug's mass
goes with its area), not data. The term is exactly 1 at the reference calibre (`x / x` is 1 in
IEEE arithmetic), so a reference-calibre cone keeps its bits; two rounds of different width
through one plate throw different cones. DERIVED: the Pzgr. 40's 35.7 mm core gives 0.165, a
7.9 mm machine-gun round 0.0081.

**Profile — the ball law** (owner, 2026-10-23, night; decision record
`.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/decision-2026-10-23.md`, the law
in `shape-law.md` §1.2 beside it). The debris leaves the exit with sideways speeds spread evenly
through a ball about its forward speed, the greatest `1/λ` of it. The share of the emission inside
polar angle `θ` of the axis is

> `F(μ) = 1 − μ·D³`, `μ = cos θ`, `D = sqrt(1 − λ²(1 − μ²))`,

0 on the axis and 1 at the rim `α = asin(1/λ)`; per steradian the field is
`p_Ω = D(D² + 3λ²μ²) / (2π)`, brightest on the axis and nothing at the rim. The exponent 3 is the
law (a ball of speeds), not data: another profile is another law. A cone's `λ` is its exit layer's
substance's (**Data**), so its half-angle is the exit material's. A cone carries its law and `λ` as
its **field descriptor** (a law id and one `f32`) in the shot record and on the wire, and the
client rebuilds every direction and weight from it (`src/damage_cam/wire.rs`); a ratio below 1, a
field over the whole sphere, is the same law's closed form, which only a burst's ball reaches
(§5a). The on-axis weight is half the centre weight `(1 + 3λ²)(1 − cos α)`: through RHA (26.2°) the axis
direction carries about `0.84 S` and the outermost of 64 about `0.05 S`, so an on-axis box takes
about 1.7× what an even profile gave it, one near the rim a graze, one at an average spot the same
(DERIVED). The even/linear dial (playtest fork F6) retired with it.

**Axis.** At a genuine exit (material to air through a rear face), `normalize(n_rear + d_exit)`,
`n_rear` the face's outward normal and `d_exit` the round's direction; at an internal step (no face
of its own), `d_exit`. ENGINEERING ASSUMPTION with MEASURED support (Verolme 1997; Recht & Howell
1967: 25° measured against 22.5° predicted at 45° obliquity).

**Sampling.** `N` directions in equal-solid-angle strata, `μ_k = cos θ_k = 1 − (1 − cos α)(k + ½)/N`,
golden-angle azimuth, each standing for `1/N` of the cone. Direction `k` carries `s_k = S·f_k`, its
weight `f_k = (N/2)[F(e_{k+1}) − F(e_k)]` its stratum's exact share of the emission between the
stratum's edges `e_j = 1 − (1 − cos α) j / N`, so the emitted total `(1/N) Σ s_k` telescopes to
`S/2` at every `N`: `N` is resolution, never balance. Shipped `N` = 64 (owner, 2026-10-17), the
throw's cost against its resolution (one throw MEASURED at each count in
`assets/ballistics/spall_cone.ron`'s provenance). The azimuths are generated once with `libm`; a
cone's `μ_k`, `sin θ_k` and `f_k` are built from its `λ` with IEEE arithmetic and `sqrt` alone, so
their bits match on every shipped target and a decoder rebuilds them from the descriptor.

**Shadowing and reach.** A direction's strength is constant in air — no drag, no range — and it
doses only the struck tank's boxes (hitting another tank is a preserved alternative). **Full
shadowing** (owner, 2026-10-17): a direction stops at the first hard body it enters. Armour takes
nothing and stops it, a hard box takes its whole strength and stops it, so whatever hard body lies
first shields everything behind it, whatever its material; a direction that enters none ends where
it leaves the struck tank. A witness (§6) never stops a direction: each one it enters short of
where it stops is dosed at its strength, so crew never shield crew. The `1/r²` fall-off is not a
parameter: a box farther away covers fewer directions.

**Walk.** A direction is walked like the line's corridors: the collector and the pairing walk along
the whole ray through the struck tank, from outside every candidate's bounds through the throw
point, with nothing declared. So what it starts inside is established, never assumed, and every
volume along it is present in order, nested and overlapping ones included. No nudge displaces the
ray: the volumes the exit leaves are recognised by identity, so a direction starting inside one of
them has not entered it and is neither stopped nor dosed by it there. A direction that starts inside
any other volume has entered it at the throw point.

**Dose.** A box's dose from one cone is `D_b = (1/N) Σ s_i` over the directions that dose it — a
hard box those stopped in it, a witness every one entering it short of its stop — each direction
dosing a box once, in the count unit (reference-mm), never scaled by the box's own factor; armour
(no count) takes nothing. Doses go through the addressed `Dose` effect, like the line's. A box's
dose from the whole shot (the line, a bounce's shock and every cone) becomes whole HP points once,
on its total (§6). Every field deposit, to a hard box and a witness alike, enters that total at the
field's own calibration (`hp_per_reference_mm` in `spall_cone.ron`, TUNED 70 HP points per
reference-mm against the line's 10) and nothing on the line or a bounce's shock does; `S` stays in
reference-mm, so the calibration sets every cone's potency at once and the HP quantum gate tests
`S/2` at it. A box of face `A` fully inside the cone, `θ` off its axis, has the knockout radius
`R = sqrt(2 f(θ) E A / (pool Ω_c))`, `E = S/2`, `Ω_c = 2π(1 − cos α)` and `f(θ)` the ball's weight
there (½ averaged over the cone). A bigger box is a bigger target: it takes more of the cone's
directions, and nothing scales its pool to match (§6). On a weak ordinary perforation through RHA,
for the 30 HP crew pool at a seated crew member's 0.2 m² mean face, the face the rate is set
against, it is about 1.9 m at an average spot and 2.5 m on the axis (DERIVED in `spall_cone.ron`);
the seed tank's crew box, 0.4 × 0.3 × 0.7 m and MEASURED at a 0.2033 m² mean face, moves each
radius out by under 1 % (DERIVED: `sqrt(0.2033 / 0.2)` = 1.008). The rate stays 70 under the ball
(owner, 2026-10-23, night): lowering it to hold the on-axis radius would weaken every cone. Dose
summed over boxes is not conserved (a direction doses every witness it enters short of its stop as
well as the hard box that stops it); the emitted budget is.

**Potency colour.** The pen cam colours a field by its potency, never by a display gain (decision
record `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/decision-2026-10-23.md`,
#8). Each of the `N` directions stands for `Ω / N` of the field's support (`Ω` its solid angle: a
cap's, a ring's band's or the sphere's) and deposits `s / N`, so a direction of strength `s` emits
`ρ s / Ω` HP points per steradian (`ρ` = `hp_per_reference_mm`), and at distance `r` along it, on
the support and short of its reach, the **local intensity** is `I = ρ s / (Ω r²)` HP points per m²,
zero past the reach; a thin unshadowed box of face `A` there takes `I · A` (resolver test
`the_pen_cam_intensity_times_a_face_is_its_dose`). Doses add, so where fields overlap the bands
classify the sum of their intensities. Three **potency bands**, parted at thresholds of the
struck tank's own crew boxes (`BoxClasses::potency_thresholds`): **knockout** at `I ≥ T_mid`,
`T_mid = crew_hp / A_crew`, `crew_hp` the shared crew pool (`assets/ballistics/box_classes.ron`)
and `A_crew` the mean face of the struck tank's crew boxes at rest in its sim geometry, the mean of
the three axis-aligned face areas of each box's bounds (`TankShapes::crew_face`; the seed tank's
0.2033 m², MEASURED, so 30 / 0.2033 = 147.5, DERIVED); **reduced** from `T_floor` up to it;
**nothing** below `T_floor`, the least share of an HP point the sim's once-per-shot rounding counts
(half a point: to nearest, ties away, `ballistics::least_counted_hp`) over the same face
(0.5 / 0.2033 = 2.46, DERIVED). The face is read from the geometry and authored nowhere else, so
the bands follow the boxes they describe; every crew box of a tank presents one mean face
(`src/tests/crew_faces.rs`), so the thresholds hold for each station alike, and a plan with no
crew box has no bands and draws no field. The promise: a crew station whose face lies wholly in
knockout is knocked out; one wholly in reduced has its pool dosed and usually not emptied, for the
rounding tips it at the band's top, from `T_mid − T_floor` (145.1, DERIVED), where its face's total
(29.5 HP points) rounds to the whole pool; one wholly in nothing takes no HP point from the
field. A box only partly inside a field takes that intensity over that part
alone. Knockout takes the field teal and reduced the deep teal (`timeline::FIELD`, `FIELD_DIM`;
relative luminance 0.50 and 0.12, DERIVED, so they part in every colour vision type), nothing no
colour; neither is the state scale's red. Along a direction the intensity reaches mid at
`sqrt(ρ s / (Ω T_mid))` and the floor `sqrt(T_mid / T_floor)` ≈ 7.7 times farther (DERIVED); s1's
strongest direction (924 HP points per steradian) reaches mid at 2.50 m and the floor at 19 m,
past every reach of the shipped records (5.2 m at most); the weakest field, s4's and s16's (2.5),
reaches its floor at 1.0 m, just past its 0.79 m reach (DERIVED from the records). So the reduced
band ends at a reach or a shadow of the struck tank's hard bodies, rarely at the floor, and its
edges are theirs.

A direction's strength is the field's law, read where the point lies
(`src/damage_cam/pen_cam/look.rs`, `law`; the shader's `law`): `S` times the law's weight
averaged over a window one stratum wide about the direction's depth `t = 1 − cos θ`, held to the
support, `(t_hi − t_lo)/2 · share(a, b)/(b − a)`, `share` the law's closed-form share between the
window's ends. At a stratum's centre the window is the stratum and the strength is the record's own
`S f_k` (MEASURED over every shipped record: within 2.7·10⁻⁵ `S` in f32); between centres it runs
continuously. The law's density read at a stratum's centre (`FieldDescriptor::weight_at`) is not
the record's `f_k`: it lies up to 3.1·10⁻³ `S` off it (MEASURED), so the look takes the mean.

The pen cam reads each field's reach from a **depth picture**
(`src/damage_cam/pen_cam/picture.rs`): the struck tank's hard bodies, posed for the record, drawn
on the GPU from the field's origin once per record and screen size, each texel holding the radial
distance (record m) to the nearest face its direction meets head-on, so a line from inside a body
leaves it; a direction that meets no body ends on the world-axis box about them. Crew stations are witnesses
and never cast (ADR-0040). A field's colour is the law's intensity wherever its picture says it
reaches: the law × the depth picture. The picture is the cube about the field's axis, each face
cropped to the support, and a record's faces share one atlas, which every view of the record
reads (the corner's frames, the kill cam and the x-ray). A field's **pitch** (rad a texel) is
sized from the live screen, as LOD is: each view asks the detail of what its camera draws, read
as it stands that frame (the expanded corner's image for the corner; for the kill cam's lesson
and the x-ray, the main pass of the camera that draws each, at the render scale where that pass
runs at it), and the pitch is the least angle one pixel of that detail
spans seen from the field's origin, over 16 points along each recorded direction as the corner's
along-shot eye sees them at 72 spins, and as the slice's eye sees them in both lenses where the
record's verdict opens the slice; where several views show a record, the finest any of them asks.
So a texel spans at most a pixel of the live target at those sampled points (DERIVED); a point
between them may take a finer pixel, and a record whose faces outgrow the 8192-texel atlas has
every pitch grown (none of the shipped records at a 4K target, MEASURED). A view's detail is its
target rounded up to 64 px a side (`DETAIL_STEP_PX`; TUNED), so a window dragged within a step
lays nothing out anew, for at most ((h + 64)/h)² the texels (DERIVED: 12 % at 1080 rows, 6 % at
2160). s14's atlas, the largest shipped, is 1618×3872 texels at a 1920×1080 target and 3335×7976
at 3840×2160 (MEASURED, the expanded corner at a scale factor of 2). A change of the details asked
lays the record's pictures out anew into a new atlas, once, at the end of the frame, whatever
order its views asked in; its views draw the old atlas until the new
one's rendering has been submitted, and a record's first pictures not before theirs, so no frame
reads an unwritten atlas and none drops the fields across a resize. The viewer's drag, the x-ray and the gunner's sight see the field at that pitch. The
shipped records' finest at a 4K target is 5.5·10⁻⁴ rad (MEASURED). Over every recorded direction
of every shipped record, the picture's CPU mirror (`Casters::reach`: the GPU's near plane, culling
and horizon, computed on the CPU) ends where the record's direction does to within 10⁻⁴ m
(MEASURED: 1,728 directions in 21 fields): a direction stops at the first hard body it enters. No readback checks the GPU's
picture against it. **The record wins**: a field whose
recorded directions all ended at its origin (s16's burst at rest inside armour) is drawn nowhere,
whatever its picture would hold. A point reads its field from the four texels about its
direction, each testing the point against its own reach, the results weighted bilinearly: a
reach is never interpolated across a shadow's edge, so behind a body the field reads the same
share at every depth out to what lies beyond it, and none once every texel about the direction
is the body's. A texel's reach stands for its whole cell of directions, and a surface the field
stops on lies a little before or past it across the cell, so each texel also counts a point
within a **reach tolerance** past its own reach, never past the farthest of the four: twice the
diagonal of a texel at the pitch, times `r` (`REACH_TOLERANCE_DIAGONALS`; the angle DERIVED from
the pitch, the factor TUNED). A surface the field stops on reads inside to at least 63° off square
(DERIVED, tan⁻¹ 2), and a point deeper behind a body than the tolerance keeps its shadow. Where the
four texels' results differ, across a shadow's edge, a band's edge follows the bilinear weights
texel by texel: a stair-step at most a pixel of the live target wide at the sampled points
(DERIVED from the pitch). A direction reads its own face's texels alone (each face's
crop runs a texel past its edge), and the faces are not blended: a shadow's edge that crosses
from one face to the next is drawn within half a texel of where it lies on either side, out to the
edge (DERIVED, the bilinear read; held over 6,000 seeded seams by
`picture::tests::a_silhouette_across_a_faces_edge_jogs_by_at_most_a_texel`), so it jogs there by
at most a texel, under a pixel of the live target (DERIVED); MEASURED on the CPU read over 6,000
random seams, the jog is at most 0.64 pitch.

The fields draw as **one volume** (`assets/shaders/pen_cam_look.wgsl`, `visible`, `peak` and
`strongest`) on the back faces of a box about every field's reach, coloured by **density**: a
pixel takes the colour of the strongest band the search finds on its eye ray, at that band's one
opacity (knockout 0.5, reduced 0.2; TUNED), knockout over reduced over nothing. A knockout core
reads inside its reduced shell, and a long run of the ray through a weak band never reads as dense
as a short one through a strong band: no term depends on the ray's length inside a band. The
ray's visible part starts at the camera's near plane and at nothing else, so a field's origin
between the eye and that plane colours nothing, and an orthographic view with a negative near draws
what lies between that plane and its camera's; it ends at its pixel's nearest scene surface in the
part mask and at the cut. Its **peak**, the greatest summed intensity the search finds along it,
sets the colour. The search takes each field's point nearest its origin on its stretch; then each
field's stretch in 64 equal steps, ends included, however short the part mask or the cut leaves it;
then 8 golden-section steps about the best sample, one of its stretch's steps either side; it stops
once the sum is past the mid's window, where the colour no longer changes. It skips no sample. A
skip bounded by each field's greatest law weight over `r²` was not conservative: the law's f32
evaluation can exceed its weight on the axis (MEASURED on the scalar f32 arithmetic: a cap of ratio
10 at `N = 65,536` strata, depth 1.503769·10⁻⁸, weighs 0.75390625 on its axis, 0.7568512 with the
skip's rounding margin and 0.7598202 off the axis). Searching every sample costs no time the
instrument resolves: paired medians of +0.48, −0.71, −2.22 and +0.76 ms a frame over four runs of 25
frames, −0.12 ms their median (MEASURED, s14's glass frame at 4.5 s, 1920×1080, against the search
that skipped). Each band's edge fades in density, never in thickness: its presence is a smoothstep
in its held level, `log2` of the peak held to half an octave either side of its threshold, over one
pixel's change of that held level, as the painting's edges fade.

How wide that fade runs on screen is a local estimate, not a promise. For an isolated field of one
strength in every direction, under parallel eye rays and with a peak no clip moves, the peak falls
as `1/d²` with the ray's distance `d` from the field's origin, so its `log2` changes `2/(n ln 2)`
a pixel `n` px out, at most `√2` times that across a pixel's two axes (the sum `fwidth` takes),
and half an octave either side holds a whole pixel's change from `n ≥ 2√2/ln 2`, 4.1 px (DERIVED);
nearer, the level leaves the window within a pixel and the edge is harder. Holding the level
narrows the fade as well: radial samples 4.1 and 3.1 px out differ by 0.807 octave, but by 0.5
once held where the outer lies on the threshold (DERIVED). A field whose strength turns with
direction, a support's edge, a front, the part mask, the cut or a shadow sets an edge the `1/d²`
law does not describe, and the fade there follows that edge's own change across the pixel.

The search never overstates: each sample is the sum where it lies, so a shadow it steps over
never colours a pixel, and the refinement keeps the best sample and adds only sums it reads inside
the visible part. It can understate. Any lit or supported interval narrower than a step can fall
between the samples, whatever its band: a lit sliver between shadows, or a narrow support crossed
near its edge. A ring's band of half-width 0.1° about 45°, reach 2 m, under an eye ray parallel
to its axis 0.5 m off it, holds a 3.49 mm stretch of the ray against a 60.52 mm step; the nearest
sample lies 45.924° off the axis and the nearest-point seed at 90°, off the support, so the search
reads nothing where the ray crosses the field (DERIVED, a review fixture). The golden-section
stage is a heuristic: neither the sum, its support nor the depth picture makes the sum unimodal in
the bracket, so it can settle on a lesser peak than the bracket holds, and the bracket's shrink
bounds no error in the peak's place. A shadow's edge in a depth picture is such a comb, its texels
lighting slivers about a texel wide, and the search reads the edge to within the comb's spread:
0.12 octave at worst over 625 rays of a CPU fixture, two fields under a coned shadow (MEASURED; a
fixture's figure, no bound on what a shadow can hide). The count of steps is set on the image:
s14's glass frame at 4.5 s, 1920×1080, its pixels whose largest channel differs by more than 8
from a search in 1,536 equal steps a stretch (MEASURED; every row but the built one while the
search still skipped samples by the bound above, which left 241 at 64 steps):

| Search | Pixels off |
|---|---|
| steps of a fixed length, a ball's 48th, at most 48 a stretch | 1,566 |
| 48 steps a stretch, equal in the angle about the field's origin | 403 |
| 96 steps a stretch, equal in that angle | 201 |
| 48 equal steps a stretch | 325 |
| **64 equal steps a stretch (built)** | **245** |
| 96 equal steps a stretch | 150 |
| 384 equal steps along the whole ray | 169 |

Steps of a fixed length stair-step a band's edge wherever it crosses a short stretch (a clip of
the part mask, a chord near a ball's rim): they cross it in one or two, read its ends alone and
step over the lit sliver between shadows there. Colouring by thickness drew the same stairs
(MEASURED, s14's glass frame at 7680×4320).

The fields are also **painted** on the components' surfaces (a playtest fork,
`.agents/scratch/playtest-forks/field-footprints.md`): a point takes the band of the summed
intensity there, at opacity 0.6 (TUNED), its band edges a pixel wide, by the rule its
component's role gives. A hard component (every role but crew) is painted where a field meets
it: on a face turned to the field's origin, on the support, inside the front, within each
texel's reach, its tolerance and 1 cm (TUNED), and within the field's longest recorded reach and
that 1 cm, where the volume ends. A crew station, a witness (ADR-0040), is painted
wherever its surface lies inside a field, the volume's own test, whichever way the face turns, so
its far faces read the field it stands in. The hull glass and every surface no component claims
are never painted. The glass, victim and outside frames and the victim's x-ray paint; the slice
does not. Mechanism and entry points:
`src/damage_cam/pen_cam/look.rs`, module doc. The calibration stays provisional until the
scenarios re-recorded under the ball are seen (owner, 2026-10-23, night).

**Draw order.** Every x-ray view that draws the tank's body (the corner's glass frames, the kill
cam's lesson, the victim's x-ray, the slice) draws the body, the main armour, external armour and
detail, behind everything else. The scene, the components with their state fills and outlines, the
round's own model and the fields' volume, is ordered by depth among itself: a component or the
round in front of a field hides it, a field in front of either blends over it. The volume blends
one colour a pixel, its strongest band's, over what lies behind it. The fields'
painting lies on a component's own surface (its mesh's, its state fill's), so it is ordered as the
component is. The round keeps its own materials and is drawn whole, in the slice too. The fields'
depth pictures are drawn off screen, once per record and screen size, read only once their drawing
has been submitted, and take no part in the order. Each layer is one surface deep. Every depth the
order compares is the eye depth, one convention in both projections: the distance from the eye in
perspective, the distance along the camera's forward in orthographic. The outside view (a damaged
tank seen whole) keeps the body in front and the volume unbounded. The trail and the rings are
outside the order. Mechanism and entry points: `src/damage_cam/pen_cam/look.rs`, module doc.

**Data.** The axis rule and `N` live in `assets/ballistics/spall_cone.ron`, parsed
and validated while the app is built (ADR-0014: built from data, synchronously). A cone's ratio is
its exit layer's substance's: every hard substance authors `debris_half_angle_deg`
(`assets/materials/materials.ron`, `λ = 1/sin α`), refused at load outside `(0°, 90°]`. The exit
layer is the one `κ` reads, the body's last before the exit; where volumes overlap in it, the cone
takes the one of highest factor, the larger ratio between equals. RHA's 26.2° is the constant-angle
fit of the MEASURED Qi et al. 2026 normal-impact debris extents (shots 5 and 6, `shape-law.md`
§2.1–2.2); those are 97 % contours, so the ball's rim may sit up to about 2° wider. Every other hard
substance authors RHA's value as Unknown until its own extent is measured. A spread whose rim's
`cos α` rounds to 1, or which does not emit `S/2` within the rounding of its weights, is refused,
and a throw whose ratio the law refuses throws nothing. The axis is playtest fork F5 (its
half-angle settled by this; `.agents/scratch/playtest-forks/README.md`).

**Class invariants** (tests hold them for any tank and any cone; fixture numbers are fixture data).
Exact emission `S/2` at every N from 1 to 1024 and every ratio from 1 to 10⁴; under the ball at a
fixture ratio: resolution stability; homogeneity in `S`; inverse square for an unshadowed thin box;
factor independence of a box's own dose; shadowing monotone (adding a volume never raises a dose).
κ is 1 to the bit for the reference and proportional otherwise; a cone below the HP quantum is not
thrown; a shot's dose to a box is rounded once on its total. A direction stops at the first volume
it enters, a box there taking its whole strength, armour nothing and shielding what lies behind;
a plate thinner than any nudge stops it; it is neither stopped nor dosed by a volume its throw
leaves, and has entered any other it starts inside at the throw point. The ball's weights are its
strata's shares of the emission, never grow from the axis to the rim, and leave the rim stratum a
graze; its continuous weight is zero at and past the rim; a thrown cone's field is its exit layer's
substance's, and its rays' strengths are `S·f_k` of that field to the bit. Witnesses: adding one
never changes the shell's state, any direction's reach or any hard box's dose; a witness behind a
witness is dosed as it would be alone; a witness takes what a hard box in its place would, and
nothing past the hard body that stops a direction. The shipped cone's walks, and six whole
main-gun shots at the bound tank (exit detection, spends and speeds, strength, axis, walks, HP
points), are digested on every target by `src/ballistics/field_casts.rs`; the force law's
transcendentals are `libm`'s, so a shot's spall sources are the same bits everywhere.

**Not built.** The blast sphere; plate failure mode (coherent plug vs distributed debris)
selected by material and impact conditions. **HE** stays a separate, penetration-independent
mechanic: a fuse triggers on minimum steel-equivalent thickness, then bursts after a delay. A burst
round's fuze and field are §5a.

### 5a. The burst — a burst round's fuze and its field

A burst round carries a burst core in its body. A body thick enough arms its fuze, the fuze runs a
delay, and where the delay runs out the round bursts and throws a field from that point about its
flight direction. The owner compares two shapes of that field, the **ring** and the **ball**, both
built as rounds on the main gun's selector (playtest fork F7); the loser leaves the list after
play. Brief: `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/brief-4b-aphe-sim.md`,
its round data from `tiger-ammo-history.md`, its fuze's delay and arming threshold from
`burst-delay-history.md` and its speed law from `shape-law.md` §4 beside it. A round without a
burst block (in the round file, `assets/rounds/<key>.round.ron`) is inert; a sub-calibre round carries none.

**The trigger.** The shell's fuze is built at spawn from its round's burst block (ADR-0014) and
advanced by the authority's march alone; a replica's march decides no crossing, so its fuze never
arms. It arms when the round leaves a body (§5, **Which steps throw**) whose spend reaches the
arming threshold: the reference-mm spent in that one body, never a sum over bodies. The delay is
counted as path from that exit: the burst lies `v × delay` metres past it, `v` the exit speed, so
the speed the round loses in the air over the delay is not counted (DERIVED). Every body exit
after that restarts a delay still running from that exit at its speed, whatever that body's spend,
and so does a bounce off a face; a bounce never arms. The count runs over every metre the round flies: open
air, a corridor found clear, the gaps between a crossing's bodies, the approach to a body it stops
in. A round no body arms flies as a solid shot, bit for bit. The shipped burst rounds arm at 35
reference-mm (MEASURED as 35 mm of plate at normal impact: L.Dv. 487, March 1943, page 487_21,
which also states the restart) and burst 1.5 ms after the exit (DERIVED: the midpoint of the 1–2 ms
that page gives the Bd.Z. 5127; it is the Pzgr. 39/1's and 39/43's fuze, and no time is found for
the Pzgr. 39's own Bd.Z. 5103), so the burst lies 0.38 m past an exit at 250 m/s, 0.6 m at 400 m/s
and 0.9 m at 600 m/s (DERIVED).

**The burst point.** The round bursts where the count runs out, in open air or a clear corridor,
inside the tank or past it; the crew boxes the leg to there crosses take their line (§6). A delay
that runs out inside a crossing, between its bodies, bursts where the crossing hands the flight
back. An armed round that stops before its delay ends (in a body, on the ground, or where a walk
failed closed) bursts where it lies, at carried speed zero, when the delay's remainder (the metres
left at the speed the delay started at) runs out: the record times the burst then, and the march
settles its doses in the tick the round stopped.

**The parts and the two shapes.** A round's parts are DERIVED at load from its body (mass `m`,
calibre `d`, its body steel's density from `materials.ron`) and its burst block (core mass `C`, its
core type's density `ρ_c` and speed constant `K`, the base parts' mass `B`, the cavity's diameter
`d_c`): the **cavity** is a cylinder of the core's volume `C / ρ_c` and diameter `d_c` just forward
of the base parts; the **wall** is the steel annulus around it over its length, outer diameter `d`;
the **nose** is the rest, `m − wall − B − C`. A burst gives a driven mass `M` the greatest sideways
speed `v_s = K (M / C + ½)^−½` (`shape-law.md` §4), DERIVED per round and never authored, and the
field's ratio is `λ = v_c / v_s`, `v_c` the carried speed.

- **Ring.** The wall is driven. The field is uniform per steradian over the band `θ_c ∓ h` of polar
  angle from the flight axis, `θ_c = atan(1/λ)` (90° at rest: a flat band), its near edge held at
  the axis where `h > θ_c`; `h` is the band's half-width about its centre (`ring_half_width_deg`).
  The nose piece flies on from the burst point at the carried speed as the shell's round, inert:
  its mass the derived nose's, its calibre and nose factor the round's, its drag re-derived, its
  crew line rate the round's. It meets the world as any round's penetrator does (§3: the disc,
  overmatch, the force law, its own line and its own spall cones). A ring that bursts at rest
  throws its band and nothing flies on.
- **Ball.** The round less its core is driven: one lobe under the ball law (§5, **Profile**), a
  cone of half-angle `asin(1/λ)` for `λ ≥ 1`, the whole sphere leaning forward below it. There the
  share inside `θ` is `F = (1 + λ − μg)(2 + λ − λ² + λμg)/4`, `g = λμ + D`, continuous at `λ = 1`
  and the even sphere at `λ = 0`; one density covers every `λ`,
  `p_Ω = (max(t₊, 0)³ − max(t₋, 0)³)/(4π)`, `t± = λμ ± D`, zero where `D² < 0`. The round ends at
  the burst.

Under both, every plate exit reached before the burst threw its spall cone as a solid shot's does,
and nothing of the round goes on past the burst but the ring's nose piece. The shipped 8.8 cm
Pzgr. 39 body (10.2 kg, 88 mm, GunSteel at 7800 kg/m³) with its burst block (`C` 0.060 kg MEASURED,
`ρ_c` 1650 kg/m³ DERIVED, `K` 2680 m/s DERIVED, `B` 0.33 kg MEASURED, `d_c` 35.7 mm TUNED,
provisional) gives, DERIVED:

| part | value |
|---|---|
| cavity | 36.36 cm³, 36.33 mm long |
| wall | 1.440 kg |
| nose (no source gives it: provisional) | 8.370 kg |
| ring: `v_s`, `½ M v_s²` | 541.5 m/s, 211.1 kJ |
| ball: `v_s`, `½ M v_s²` | 205.8 m/s, 214.8 kJ |

**The field.** Thrown from the burst point about the round's flight direction at strength
`S = r_E · ½ M v_s²` (reference-mm, `r_E` = `reference_mm_per_kj`), emitting `S/2` in all; a burst
is no plate exit, so neither `κ` nor the hole term applies. Its `N` directions sample the law's
support in equal-solid-angle strata (the band, the cap or the sphere) weighted by their strata's
exact shares, so it emits `S/2` at every `N` (§5, **Sampling**). They march, shadow and dose as a
cone's do (§5, **Shadowing and reach**, **Walk**, **Dose**) through the volumes of the tank whose
body armed or last restarted the fuze, leaving none, behind the HP quantum gate, and convert at
the field's rate (`hp_per_reference_mm`, 70). The record and the wire carry the law (the ring's
with its half-width) and the ratio as the field descriptor, and a decoder rebuilds the directions
from it. Data: the `burst` block of `assets/ballistics/spall_cone.ron` holds `r_E`, `h` and each
law's `N`, with their provenance and the throw's MEASURED cost at 64, 256 and 684 directions.

**Potency.** Ring and ball carry nearly the same energy (211.1 against 214.8 kJ): they differ in
where the potency goes, not how much there is. `r_E` = 1.0 per kJ (TUNED, provisional) is set so a
crew box of a 0.2 m² mean face (a seated crew member's), wholly inside the ball at rest 2.0 m from
the burst, has its 30 HP pool emptied. A thin unshadowed box of face `A` wholly inside a field,
where the field is `b` times as bright as an even sphere's (`b = 4π p_Ω`), has the knockout radius
`R = sqrt(ρ (S/2) A b / (4π pool))`, `ρ` = 70; a module's follows from its own authored pool and
face (§6). DERIVED, at rest and at s1's carried speed (244.6 m/s), for the crew at 0.2 m² (the seed
tank's crew box, MEASURED at 0.2033 m², reaches each 0.8 % farther, DERIVED):

| field | where | `b` | crew (0.2 m²) |
|---|---|---|---|
| ball at rest | anywhere | 1 | 2.00 m |
| ring at rest | in the band, 80–100° | 5.76 | 4.75 m |
| ball at 244.6 m/s (`λ` 1.188, rim 57.3°) | on the axis | 10.47 | 6.46 m |
| ring at 244.6 m/s (`λ` 0.452, band 55.7–75.7°) | in the band | 6.32 | 4.98 m |

**The record.** The shot record carries the burst (`damage_cam::Burst`): when (the path reaching the
burst point, its last leg timed at the speed the burst trigger counts it at, so a burst in flight
lies the delay after the exit that last started it; for a round at rest, the delay's remainder after
it stopped there), the burst point, the carried speed, the ring's nose piece's mass, and the field
as a cone record (absent when the HP quantum gate held it). The ball's record ends `Burst`; the
ring's path runs on with the nose piece's line to its own end, its open air timed at the carried
speed. A struck tank's record stays open while a fuze that tank armed runs, so a burst past its last
body lands in it. The wire round-trips the burst; its 256 directions add 0.3–2.1 KB to a message
(MEASURED, `src/damage_cam/wire.rs`). The pen cam draws the burst field as it draws a plate cone
(`src/damage_cam/pen_cam/picture.rs`): thrown when the shell's nose reaches the burst point on the
corner's clock, its front running out each direction's reach, over the law's support alone. The ring
reads as a band about the line with nothing drawn inside its near edge; the ball as a cap ahead, or
the whole sphere, closed with no seam. The ball's line ends at the burst point, where the shell
rests at the end; the ring's runs on with the nose piece. A box the burst doses is reached when the
first field that dosed it, a plate cone's or the burst's, reaches it.

**Scenarios** (`assets/dev/pen_cam/`, fixture data; each record's `shows` says what it shows).
s13 and s14 fly s1's line with the ring and the ball: the upper front plate arms the fuze, and the
round bursts 0.37 m past its exit at 244.6 m/s, short of the driver's station. The ring's band
empties the HP pools of the bow gunner, the transmission and two ammunition racks, and its nose
piece flies on through the driver's station to the rear plate. The ball's lobe empties those of
the commander, the gunner, the loader, the engine and three ammunition racks, and the round ends
short of the driver. In both the plate's cone knocks out the driver, as in s1, which empties no
other HP pool and leaves the gunner and the commander reduced. s15 flies s6's line with the ball:
no body holds the arming spend, so its record is s6's. s16 flies s4's line with the ball: the road
wheel arms the fuze, its rubber and far disc restart the delay, and the round stops 20.7 mm into
the lower hull side and bursts at rest there; every direction starts inside that plate, which
stops it, so the field doses nothing (MEASURED, from the records).

**Class invariants** (tests hold them for any round and tank; fixture numbers are fixture data).
The fuze arms only at the threshold; the delay runs from the exit and restarts at a later body or
a bounce; the burst lies the carried speed times the delay past the exit; a round at rest bursts
where it rests after the remainder; a round no body arms, or without a burst block, flies as a
solid shot. The ball's and the ring's weights emit `S/2` at every `N` the data allows, `λ < 1`
included; the ring's directions lie in its band; the ball's field leans forward with the carried
speed. The ball ends the round; the ring's nose piece flies on at the derived mass and nothing else
does. The parts sum to the round, and a burst that leaves no nose is refused. The record and the
wire round-trip a burst.


## 6. Component damage — HP per component, never per tank

Every component has its own **HP pool** in whole **HP points**. A crew station's is the one shared
crew pool (30 HP, TUNED, provisional, `assets/ballistics/box_classes.ron`), which a tank file
cannot author: a crewman is one entity across tanks, and the pool has no relation to the size of
the crew box. Every other pool is a plain integer authored by hand in the tank data (`hp` in
`<tank>.tank.ron`), tuned in play; no class rate or size rule derives it (owner, 2026-10-28, #104;
a pool derived from class × volume is deferred to tank #2, if hand-authoring proves inconsistent).
The field needs no such rule: a bigger box is a bigger target, crossed by more of a field's
directions, so it takes more dose (§5). A crew box is person-sized, one shared size within a tank
(the seed tank's 0.4 × 0.3 × 0.7 m, a 0.2033 m² mean face, MEASURED from its sim geometry), close to
the 0.2 m² face the field's calibration is set against (§5), and the pen cam's potency bands are
read from it (§5, "Potency colour"). One
point is 0.1 reference-mm of line dose (`ballistics::HP_PER_REFERENCE_MM`, a unit with Resolution
provenance, not a balance value); a field deposit converts at the field's calibration (§5). A spall
cone doses it by the strength reaching it (§5); the **main penetrator transiting** a hard module
deposits what it spent crossing it; a bounce jars the face it struck.

**Witnesses** (ADR-0040) are dosed by a law of their own. A witness substance has no resistance and
is no layer of the walk (§13), so the shell and the field cross it as air. Once the hard world has
resolved the shot, each witness box takes:

- **the line:** `chord × wound`, `chord` the disc-mean chord the shell's legs cut through it (the
  walk's own k-sample disc, §13.5, each sample that misses counting zero; a leg ends at the rest
  point of a shell stopped inside it), `wound` the **round's** crew line rate (reference-mm per metre,
  the round file, TUNED);
- **the field:** each spall direction entering it, at its strength there × its share (§5).

Both go through the same `ShotDose` tally, units and once-per-box rounding as a hard box's, and a
mark's deposits all land before its one settle. Crew carry a witness substance and only crew do,
refused at bake otherwise. A box's dose from **one shot**
— the line, a bounce's shock and every cone, summed — is converted once, rounded to the nearest
point (ties away from zero, IEEE `roundToIntegralTiesToAway`, no transcendental), never per
direction and never per source; the pool stops at zero. HP falls as the shot is resolved, each
step taking what the rounded running total has gained, so it lands on the rounded total when the
shot ends. HP replicates as whole points.
This is *local function state*, not a global health bar. A tank is knocked out at zero active
crew, a cook-off taking precedence (`knockout_from_counts`, `src/damage.rs`); crew < 2 is the
kept alternative, playtest fork F2.

- **Crew are witnesses** (low HP) — the knockout currency; a graze of the cone's edge leaves a station
  reduced, a station inside the cone's knockout radius is knocked out, and a clean main-gun
  transit's line alone empties the crew pool (10 cm of chord at 30 reference-mm/m is 30 HP,
  DERIVED). (HP, not binary, so fire-over-time and near-misses have somewhere to accumulate.)
- **Modules are tougher and repairable** — one stray fragment scratches an engine; a faceful or a
  direct transit wrecks it.
- **Degraded performance (later):** checkpoints preferred (e.g. ≤50% HP → −x% power) over continuous
  `hp% = perf%`, for legibility. Tuning, not slice.

## 7. Crew — positions with capabilities, served by crewmen

Crew are not a counter; each crewman **is a station/function**: gunner (aim), loader (reload), driver
(move), commander (view/command). Capability is never owned by a module alone — it is **served by
whichever active crewman holds that station, at their effectiveness.** Stations **backfill**, the
commander being the universal (degraded) backup:

- Loader down → commander loads, slower.
- Gunner down → commander overrides the optic (modern), or the player falls back to the commander's
  third-person view (old). Lose the commander too and that view is gone.

**The view/control modes are themselves crew functions** — third-person = the commander's eyes out of
the cupola; the gunner's optic (`sight.rs`, in flight) = the gunner's station. So the crew system
will eventually **gate** sight/aim/driving rather than sit beside them. *Flag for the gunner-sight
work:* the optic and third-person toggle are crew-served capabilities, not unconditional.

### 7a. v1 capability model — positions + composition (2026-06-29)

> **Superseded by §7b (2026-06-29).** This records what slice 1 shipped: capability *tags on
> positions*, AND-only requirements, the `Action` name. §7b is the current target — `Capability`
> (renamed), requirement *groups* (Part/Pool/Backup), graded effectiveness, and the Station/Crewman
> split. Kept for history; do not implement from this section.

**Slice 1 (this slice): composed position capabilities + crew knockouts affecting those.** Slice 2 =
backfill and position swapping between crew members upon a knockout (separate slice; see §7b).

**The primitive:** a ballistic volume that is also a crew position carries a `Position` component
with a set of `Capability` tags it grants when staffed by an active crewman. v1 keeps `CrewStation`
as the station-identity label (what seat this volume is — "Commander", "Gunner", …, used for
diagnostic readouts and slice-2 backfill ("who is where")) and adds a parallel `capabilities`
facet (what the position grants). So `CrewStation` is **not** retired in slice 1; slice 1 keeps
it as identity, slice 2 may grow it (see §7b). The fused-model v1 cuts `CrewStation`'s *role as
the gating key* (gating now keys off the `capabilities` set, not the enum variant) but keeps the
enum itself as the station identity. v1 is the **fused model** — the crewman's body and the
position are the same entity (the existing `Ballistic_<Crew>` volume). The separation refactor
(extract occupant identity into a child entity + `Occupies` relationship) lands at slice 2
(backfill), not now.

**v1 capability set:** `Drive`, `Traverse`, `Fire`, `Reload`, `GunnerSight`, `CommanderView`.
- "Traverse" = slewing the turret/gun servo (gates `ServoCommand` writers in `aim.rs` and
  `sight.rs`). Distinct from `GunnerSight` (the optic camera), which the gunner also carries —
  both go out of action together when the gunner is knocked out, but they gate independent systems.
- `Reload` is modeled explicitly as a `Reload { remaining: f32 }` component on the `Gun` entity
  (in seconds, ticking down; `remaining == 0` = fireable), not a singleton resource. Loader knocked
  out → no decrement (currently-loading round stays partway through).

**Tiger v1 positions and capabilities (the RON's `volumes` facets):**

| Position | Capabilities |
|---|---|
| Driver | Drive |
| Gunner | Traverse, Fire, GunnerSight |
| Loader | Reload |
| Commander | CommanderView |
| BowGunner | ∅ (an HP pool to empty; hull MG capability comes later) |

**v1 staffing query (binary, no efficiency):** "is there any active crewman-position entity with
capability X, owned by this tank?" = `Position { capabilities contains X } AND !Incapacitated AND
VolumeOf(tank)`. The staffing query keys off the `capabilities` set (data), **not** the
`CrewStation` variant — so a future tank could have two Gunner-variant positions with different
capability sets without code change. `CrewStation` is display/identity only. No `Occupies`
relationship in slice 1 (natural assignment only — each crewman is born into its position;
backfill/swap is slice 2).

**Composition with module gates:** a capability is available iff the position is staffed by an
active crewman AND the relevant module HP > 0 (e.g. Drive requires an active Driver AND Engine HP > 0
AND Transmission HP > 0). The existing `function_disabled` gate stays; a crew-staffing gate is
added alongside it. They compose (AND), they don't replace.

**View on crewman knockout — auto-fallback:** losing the crewman whose view is active auto-switches
the player to the other view if its crewman is still in action (CommanderView out of action → gunner
optic; GunnerSight out of action → third-person). Both out of action → dark (pairs naturally with the
imminent 0-active-crew knockout under the §1a model).

**Playtest fork — single-crewman juggling (slice 2, *not* v1):** when only one combat-crewman
remains in action, what happens to the gun? **Default chosen 2026-06-29: hardcore** — the survivor
physically swaps between positions and serves one role at a time (constant no-aim-while-loading /
no-load-while-aiming); the cost is *time*, keeping `Occupies` strictly 1:1 (no efficiency
coefficient needed yet). Arcade variants (dual-occupancy, or remote load) are preserved as
additive layers if the time-cost plays too punishing. Full fork — alternatives, why it's a
playtest call, revert cost — in the register: `.agents/scratch/playtest-forks/README.md` (F1).
Requires the slice-2 position-separation refactor (extract crewman identity from position entity,
add `Occupies`).

### 7b. Backfill + capability model v2 — the swap mechanic (slice 2, designed 2026-06-29)

Designing backfill evolved the capability model past §7a's AND-only requirements and past slice-1's
`Action` naming. **This subsection is the current target; §7a is the slice-1 historical record.**

**Separation refactor — Station vs Crewman.**
- **Station** — the *place*: a fixed, spatial ballistic volume carrying a **role** (the Gunner's
  station grants the gunnery capabilities). Persists whether occupied by an active crewman, a knocked-out one,
  or (transiently) no one. **Role lives on the station, not the occupant** — which is what makes
  "the commander serves the loader's station" expressible (Commander *by specialty*, occupying the
  Loader station *by assignment*).
- **Crewman** — the *human*: HP, `Dead`, a `home` (native station / specialty), and later skill.
- **Bijection invariant:** crew ↔ station is always a perfect matching (N crewmen, N stations). Knocked-out
  crew stay in the matching — their station still absorbs penetrators (unchanged). A swap is a
  **transposition** of two crewmen's station assignments.

**Topology B — occupant-data on seats (chosen 2026-06-29, implemented).** Rather than split station
and crewman into *separate entities* with an `Occupies` relationship (call that topology **C**), the
slice keeps the **fused seat-volume** as the unit and treats the crewman as occupant *data* on it:
the volume carries geometry + `ComponentHealth` (the body's HP) + `CrewStation` (the **seat**'s role)
+ `Crewman { home }` (who currently sits there). A swap **exchanges occupant state** (`home`, HP,
`Dead`) between two seats — so the *active* crewman's hitbox moves with the person (shooting
the new seat knocks out the backfiller), honouring the Q4 honesty decision, while the ballistics march is
**untouched** (it still deposits onto each volume's `ComponentHealth`). `competence(home, seat)`
(`damage.rs`) gates the foreign-seat penalty (native 1.0 / flat 0.6). **Topology C** — first-class
`Station`/`Crewman` entities + an `Occupies` relationship + rerouting the four ballistics deposit
sites to the occupant — is the cleaner long-term form, deferred until a second reason to pay for it
(e.g. crew that visually relocate, or stations with independent geometry); see §9.

**Capability (renamed from `Action`).** The tank-model verb — Drive, Traverse, Fire, Load,
GunnerSight, CommanderView. The player-facing intent verb lives in the future Controls layer
(ROADMAP Phase 2), so "Capability" is free for the model concept *and* carries a **degree**
(effectiveness) that "available action" could not.

**Requirement model — groups.** A capability's requirement is a **list of groups, AND'd** (`min`
across groups). Each group combines its members one of two ways:
- **`Part(x)`** — a single mandatory thing (sugar for a one-member group at 1.0); missing → 0.
- **`Pool([...])`** — *cooperative* redundancy: present contributions **sum, capped at 1.0** (two
  engines at 0.5; two loaders on a heavy gun).
- **`Backup([...])`** — *substitutive* redundancy: the **best** available path wins (`max`) (electric
  vs hand traverse; autoloader vs hand-load). A backup routes around the primary's dependencies, so
  those deps fold into the *primary member's* quality.

A member is `(coeff, [Part])`: `coeff` is its share (Pool) or ceiling (Backup); `[Part]` are the
things it needs, whose qualities **multiply** in. **`Part`** is a single flat enum naming every
referenceable thing (crew stations + module functions, one vocabulary — no `Crew(...)`/`Module(...)`
wrapper). Crew-vs-module is intrinsic to the resolved entity, not the reference.

**Quality** ∈ [0,1], resolved live per `Part`:
- **Module** → condition: 1.0 if HP > 0 (graded damage curve later, §9).
- **Crew** → `competence(crewman, station)` = 1.0 native / flat 0.6 foreign for now; later × skill.
  Competence is **relational** (crewman × station), *not* an attribute of either — the seam the
  skill/training system plugs into.

**Combine (per frame, against the live world):**
```
member  = coeff × Π(quality of each Part it needs)
group   = Part/Pool: min(1, Σ members)   |   Backup: max(members)
effectiveness = min over groups          // 0 = unavailable, 1 = full
```
Effectiveness is a **rate** (`time = base / effectiveness`; reload at 0.3 → ~3.3× as long — or a
speed/power scale). The RON declares only the static *recipe* (structure + coefficients); HP, who's
active, and backfill assignment are live inputs — knockout/damage/swap change the inputs, never the
recipe.

**RON shape (flat):**
```ron
capabilities: {
    Fire:     [Gunner, Breech, GunBarrel],
    Drive:    [Driver, Pool([(0.5, [Engine_L]), (0.5, [Engine_R])]), Transmission],
    Load:     [Backup([(1.0, [Autoloader]), (0.15, [Loader])]), Breech],
    Traverse: [Gunner, Backup([(1.0, [TraverseMotor]), (0.1, [])])],
}
```
Tiger needs only bare `Part` (mandatory) groups; Pool/Backup are exercised only by exotic tanks
(the autoloader tank just writes `Load: [Backup([(1.0,[Autoloader])]), Breech]` and drops its Loader
station — no code change). Serde: mixing a bare `Part` and a `Pool(...)` in one `Vec<Group>` likely
needs `#[serde(untagged)]` on `Group` — verify it round-trips against the pinned `ron` first. `Part`
as a **flat** enum (not wrapping CrewStation/FunctionRole) is the robust choice and avoids untagged
at the reference level.

**The swap mechanic (grilled 2026-06-29):**
- **Trigger** — bare **tap-source** (an active crewman) → **tap-target** (a station). Player picks
  both ends; nothing auto-proposed. Distinct from *view* switching (camera; already exists via
  Lshift).
- **Duration** — a **timed transition**; the crewman **keeps manning the source station** until the
  timer completes, then the assignment flips atomically. Trade-with-latency, **no dead window**.
  **Cancellable, not pausable** (cancel discards the timer; he never left). Anti-spam is the timer
  itself — nothing is gained mid-transit. ~3–5s, tunable.
- **Restrictions** — **anyone → any station.** `competence` is the only gate; an impossible pairing
  is just `competence = 0` (dark though "staffed"). No allow-list system. Tiger MVP: 1.0 home / 0.6
  foreign, no zeros yet.
- **Occupants** — the swap is a **1:1 transposition** (the bijection above): the knocked-out occupant
  takes the survivor's vacated station; the active crewman's hitbox moves honestly to the new station;
  the knocked-out occupant's location is free cosmetics (it absorbs identically anywhere).

**Playtest knobs (register F1):** the transition **cost model** (source-live latency vs both-ends-dark
sacrifice) and the **timer duration** are tuned at the controller, not chosen at the desk.

**Slice-2 build scope:** the occupant-data split (topology B: `Crewman { home }` + `competence`); the
`Capability`/`Group`/`Part` evaluator (general `min`/sum/max — Tiger exercises only `Part`; Pool/Backup
get pure-function unit tests); effectiveness as `f32`; the swap mechanic. Deferred (§9): topology C
(entity separation), graded module damage, the skill/training system, `min`-vs-`product` at the AND
level, min-count floors, per-gun keying (multi-turret), `OneOrMany` serde sugar.

**Build status:** steps 1–3 landed and green — capability model + evaluator (behaviour-preserving),
topology-B occupant split + `competence` (behaviour-preserving), and the swap mechanic
(`PendingSwap` + `tick_swaps`, `SWAP_SECONDS = 4`) with a sandbox crew bar (`1`–`5` tap-source →
tap-target; layer toggles moved to `F1`–`F3`). The sandbox readout shows **scalar effectiveness**
per capability (`Load 60%` when backfilled), not a boolean. GUI-verified. Slice-2 core is complete;
remaining work is deferred refinements (§9) and the eventual game-side (non-sandbox) crew UI.

## 8. Catastrophic & environmental

- **Ammunition** — each shell is modelled **individually** as a ballistic volume + HP. Firing
  **depletes** the stowage, so an emptier rack is a smaller target and less catastrophic (the real
  "empty your ready rack" play). A shell's HP → 0 = **cookoff** = all crew knocked out (knocks out every crew
  position → 0 active crew → tank knocked out under the §1a-A model). Turret launch is triggered off
  `CookedOff` directly, not via `TankKnockedOut` (see §1a — KO is retired as a gameplay gate).
- **Fire** — an **engine hit by a direct penetrator** (not fragments) has an ignition chance. Fire
  does **not spread**; it does range-per-tick damage to nearby crew/components and can be **put out**
  (a crew repair action). A dedicated **fuel** volume comes later.

## 9. Open tabs (deferred, named so they aren't lost)

- Spall strength is settled (§5: `spend × (v_exit / v_entry)²`); remaining tuning = the box counts
  (the owner's feel pass), the half-angle and axis fork, and whether a dedicated brittleness
  (plug vs debris) selects the failure mode.
- Ricochet threshold dependencies: pure angle now; later velocity/caliber-scaled.
- HE: fuse minimum-thickness trigger + delay + dense wide cone (penetration-independent).
- Numeric magnitudes (k overmatch ratio, ricochet angle, normalization degrees, material factors)
  — tuned live in the sandbox.
- Data homes: ballistic-volume geometry on the model (watertight); material factor + shell specs in
  RON (ADR-0010 spirit — ADR-0010 already flags per-plate armor as a genuinely node-attached case);
  crew/module/station definitions TBD.
- Repair detail (who, how long, occupies which station).
- Player feedback / legibility — how the player reads *what happened* to their tank.
- **Capability model (§7b) deferred edges:**
  - Cross-group combine: `min` (bottleneck, chosen) vs `product` (compounding) — they agree unless a
    capability has *two* simultaneously-degraded groups; settle when that first arises.
  - **Min-count floors** — groups that don't degrade gracefully ("need ≥2 or it's 0"), vs the linear
    `Pool` share model.
  - **Skill / training system** — competence beyond the static native/foreign split: per-crewman
    skill, "last stand", swap-timer modifiers. All are multipliers on the two computed scalars
    (effectiveness, transition time); the model is purely additive when it lands.
  - **Per-gun keying (multi-turret)** — the one axis groups *don't* cover: `Capability` keyed by the
    operated gun entity, not a global enum (M3 Lee, Char B1, T-35).
  - **`OneOrMany` serde sugar** — let single-`Part` `Pool` members write `(0.5, Engine_L)` instead of
    `(0.5, [Engine_L])`.
  - **Topology C (entity separation)** — promote `Station`/`Crewman` to first-class entities with an
    `Occupies` relationship and reroute the four ballistics deposit sites to the occupant. Cleaner
    than topology B (the fused seat-volume), but pay for it only when crew visually relocate or
    stations gain independent geometry (§7b).

## 10. Architecture & the seam

- **`ballistics` — a library feature plugin** (ADR-0002): projectile spawn + integration + the
  world-space march + ballistic-volume cost + spall + HP deposit + inspection hooks. A **deep
  module**: a small interface (fire a shot; a ballistic volume registers itself) over a large hidden
  implementation. Consumed by *both* front-ends.
- **Split today's `shooting.rs`:** the ballistics/march moves into the shared `ballistics` module;
  `shooting` keeps only the game-specific **gun control** (fire-on-click, reload, recoil), feeding
  `ballistics`. Same mechanic, two triggers (player's gun; sandbox's camera).
- **The sandbox is a second binary in the same crate**, not a separate crate (perpetual sync
  burden) and not an in-game `AppState` (pollutes the shipping app with sandbox-only systems). It
  composes a *subset* of the library's feature plugins + one sandbox plugin. This matches `main.rs`'s
  own note that features live in `GamePlugin` so they can be mounted on an alternate App.

## 11. Armor sandbox v1 — the build

An isolated tool to develop and *tune* the march deterministically, decoupled from driving/aiming.

**Binary:** `src/bin/armor_sandbox.rs`, run with `cargo run --bin armor_sandbox`. App composition:

```
DefaultPlugins + PhysicsPlugins          // runtime + sim
+ spec::plugin + tank::plugin            // the target tank (reused as-is)
+ ballistics::plugin                     // the shared mechanic (new lib module)
+ <sandbox plugin>                       // camera-as-gun, free-fly, time, inspection
```
Deliberately **no** `driving`, `aim`, `camera`, `sight`, `shooting`.

**Sandbox plugin:**

- **Free-fly camera that *is* the gun** — WASD + Ctrl/Shift to float; the shell spawns at camera
  centre and fires straight down the view axis. Inspection camera and firing solution are one object.
- Keys to set **muzzle velocity** and cycle **shell type** (non-positional inputs).
- **Time controls** — pause / slow-motion / single-step.
- **Inspection draw** — the path segments, entry/exit points, per-volume cost, residual velocity at
  each stage, the spall cones, and HP deposited; the **last shot's path frozen** on screen to A/B an
  angle.

**v1 simplifications:** modules/crew are just *named ballistic volumes with HP* — no function, no
backfill, no fire, no cookoff, no HE. Those are §§7–8 and arrive after the march itself feels right.

**API discipline (AGENTS.md):** verify every Bevy 0.19 / avian3d 0.7 API against versioned docs
(`docs.rs/bevy/0.19.0`, `docs.rs/avian3d/0.7.0`, or the `v0.19.0` / `v0.7.0` tags) *before* writing
it. Do not write engine code from memory.

## 12. Model↔code binding contract (LOCKED, 2026-06-27)

> **SUPERSEDED IN PART — classifier precedent, 2026-08-07 (Yan: "churn desired. no legacy, we're
> setting precedent").** The name-marker convention below (`Ballistic_*` / `*_Ballistic`) is
> **retired and stripped from the source .blend**; the drift lint dies with it. The three concerns
> now separate cleanly:
>
> - **MEMBERSHIP = material, per primitive.** A primitive marches iff its material datablock name
>   is a key in the global substance registry (`assets/materials/materials.ron`, mirrored by
>   `assets/materials/materials.blend`, LINKED never appended). Wearing a registry material IS the
>   declaration — so **decor must never wear a substance material**, and a substance primitive is
>   subject to the per-primitive manifold gate. Factors live in the registry keyed by substance
>   name; per-node `material_factor` in the tank RON dies at the slice-3 landing ("no numbers in
>   the model" survives — the numbers just moved from per-node RON to the one registry).
> - **BEHAVIOR = RON facets, by node name, components only.** `volumes` shrinks to nodes carrying
>   facets (hp / crew / function); plates and wheels vanish from the RON entirely. Both directions
>   fail loud: a facet key without a model node breaks the bind; **material-implied lints** replace
>   the old drift lint (Flesh without a crew facet warns, Ammunition without an ammo facet warns).
> - **IDENTITY = free-form names, never parsed.** Names address (servos, weapons, views, facets);
>   they never classify. The `_Collider` suffix match and the `Wheel_{L,R}_{n}` pattern are
>   sanctioned residue to be replaced by **explicit RON declarations** in the same slice — after
>   which no `ends_with`/prefix scan survives anywhere in the bind path.
>
> Legibility in Blender comes from collections + the substances' distinct viewport colors (a free
> x-ray), not from names. Paint is a **runtime livery** composed onto `paintable` substances —
> never a material assignment in the source.

- **Ballistic volumes are named nodes parented to their rig part** (Hull / Turret / Gun), inheriting
  its motion exactly like `*_Collider` (ADR-0008). Turret armor parents under `Turret`.
- **The RON `volumes` map (keyed by node name) is the source of truth** (updated 2026-06-28 —
  *composition, not prefixes*). A node is a ballistic volume **iff it is a key** in
  `<tank>.tank.ron`'s `volumes`. Each entry: `material_factor` (always — shell-resistance per metre)
  plus **optional facets** that layer roles on top; today the only facet is `hp` (present → a
  damageable `ComponentHealth`; absent → pure armour). Composition over a `kind` enum (§2): future
  consequences (cookoff, crew station, fire) add *more* optional facets → each its own ECS component,
  never a central enum. Role and resistance are **independent data**, so a steel barrel is a
  `Module_` with `material_factor: 1000` *and* an `hp`.
- **One naming convention, NOT parsed for behaviour** (updated 2026-06-28): every ballistic volume
  is `Ballistic_<part>` (e.g. `Ballistic_Hull_UFP`, `Ballistic_Engine`, `Ballistic_Gunner`,
  `Ballistic_Ammo_L_0`, `Ballistic_Gun_Barrel`). The old `Armor_/Module_/Crew_/Ammo_` role-prefixes
  are **gone** — role lives in the RON facets, so encoding it in the name too was a second source of
  truth. The single prefix only marks "this mesh is a hitbox" (vs visual skin / `*_Collider` / rig
  nodes — the model's mesh kinds split by *purpose*, not role) and powers the **drift lint** (a
  `Ballistic_*` node absent from `volumes` warns). The code reads the RON, never the prefix; every
  `volumes` key must have a matching node (asserted at bind).
- **Mesh:** watertight / **manifold** solids *per volume*; convex *not* required (penetration is a
  raycast query, not the physics solver); non-rendering (the sandbox visualises them itself).
  **Amended 2026-08-03/04 (§13.7):** manifold-per-volume is the only geometric gate.
  **Face-to-face contact** is the preferred joint style (x-ray legibility, physical honesty);
  overlap is tolerated (the union walk charges shared space once) and mutual exact-fit is NOT
  required — sub-caliber slop in either direction is harmless (§13.5). Super-caliber corridors are
  fuzzer-hunted.
- **No numbers in the model.** `material_factor`, `hp`, and future facets live in RON keyed by node
  name (ADR-0010). Model = named manifold solids; code (RON) = all semantics.
- **Mesh kinds split by purpose, not role** (restructure 2026-06-28): the rig is a **skeleton of
  Empties** (`Hull`, `Turret`, `Gun`, `Gun_Barrel`, `Muzzle`, `Wheel_*`, `Track_*`, `Center_Of_Mass`
  — plain names, bound by the rig contract; their origin *is* the pivot). Every mesh is a **leaf**
  under a rig empty, prefixed by purpose: `Visual_*` (rendered skin, no gameplay), `Ballistic_*`
  (the march's hitboxes, concave OK), `Collider_*` (convex physics proxy, Vehicle layer). No mesh is
  ever the parent of another mesh or a rig node, so the art carries no mechanism and the three shapes
  stay independent. `Collider_*` is matched by prefix in `on_tank_ready`.

## 13. The union field model — overlaps, gaps, and the classless laws

**Status: SPEC, finalized in design conversations 2026-08-03/04; not yet built.** Supersedes §3's
*serial* crossing resolution and amends §4's overmatch/ricochet inputs; §5's spall budget and §12's
binding contract survive (one §12 mesh bullet amended, §13.7). Three mechanisms on three orthogonal
axes: **union** (shared space, §13.2), **ε-weld** (longitudinal micro-gaps, §13.4), **disc
sampling** (lateral geometry — the shell is caliber-wide, §13.5). Motivated by the visual↔ballistic
unification work on `tiger_1`: authoring exact plate abutment (no overlap, no gap) proved brutal in
Blender — and inspection of the live resolver showed exact fit **has no consumer anyway** (§13.1).
The precedent is instructive: War Thunder ran painted 2D armor for a decade, migrated to volumetric
3D in 2019 because surface-model artifacts were unfixable, and their notorious volumetric bugs were
exactly plate-junction overlaps and gaps. The fix here is not retreating to painted armor — on flat
plates the volumetric chord *equals* `t/cosθ` exactly, and painting is worst precisely where we need
it most (the cast mantlet). The fix is changing the resolution law so seams stop being special: overlap becomes *legal*
(union), micro-gaps become invisible (ε-weld), and the shell meets geometry as a caliber-width
body (disc sampling) rather than an infinitely thin line.

### 13.1 What the serial resolver actually does at seams (findings, 2026-08-03)

The march resolves one plate at a time: first armor hit, then thickness/exit probes **restricted to
the struck entity** (`|e| e == hit.entity` — `ballistics.rs:2243`, `:2330`), charge that plate's
chord, teleport to its exit, nudge `EPS = 1mm`, resume. Two parry facts govern the seams
(verified in `parry3d-0.27.0/src/query/ray/ray_triangle.rs`): trimeshes have **no interior** (the
`solid` flag is ignored for triangles — a ray born inside a plate just flies until it crosses a
face), and triangles are **double-sided with the returned normal flipped to oppose the ray** (a
backface hit from inside reads as a head-on entry). Consequences:

| Seam geometry | Current behaviour |
|---|---|
| Disjoint plates, ≥1mm air | Correct — each crossing charged in full. |
| **Overlap** | Shell pays plate A's full chord, exits *inside* B, next contact is B's **backface from inside** → thickness/span probes find nothing ahead → `unwrap_or(0.0)` → **B crossed at zero cost**, plus a spurious `penetrated: true` `Impact` (flame lick) and junk `PenetrationEvent` at the seam. **Undercharge** — the opposite of WT's double-count. |
| **Exact abutment** | Same bug: the 1mm nudge hops the shared face into B's interior. Stacked plates (mantlet over turret face, appliqué) currently grant the **first plate's protection only**, however perfectly they abut. |
| **Gap** | Shell enters untouched, hits the far wall's *inner* backface → reads as a head-on 0mm penetration → zero damage but a **fabricated `ShellTerminal` "penetrated" confirm** on the wire, at the wrong face. |
| Hole in a plate (non-manifold) | Exit probe finds no face → `span = 0` → free perforation. (The known "non-watertight = zero armour silently".) |

Conclusion: the no-overlap/no-gap authoring standard is unrewarded — perfect abutment produces the
same free crossings as sloppy overlap. The geometry discipline and the resolver must be redesigned
*together*.

### 13.2 The model — a factor field and two kinds of laws

Composition, not classes (§12's doctrine, extended to resolution). Along the march segment the base
truths form a **material-factor field**: at each point, the set of volumes covering it, each
contributing its `material_factor`; `hp` stays an independent facet. Two kinds of laws consume it:

- **Field laws** (pointwise): cost and damage integrate over the field.
- **Boundary laws** (at transitions): ricochet, normalization, spall, entry/exit reads fire where
  the field *steps*.

**Cost law:** `cost = ∫ max(factor over covering volumes) dt`. The combiner `max` is **forced** by
the invariants, not chosen by taxonomy:

- **Idempotent** — the same steel claimed twice charges once (the plate-junction fix).
- **Monotone** — adding any volume never *reduces* protection anywhere (a gunner's arm clipped into
  the turret wall cannot dilute the steel).
- **Commutative** — authoring order can't matter.

This is CSG union evaluated lazily on the 1D ray (Roth '82): overlapping *individually-manifold*
plates behave exactly as their boolean union, without ever building the merged mesh. Boolean union
is the special case of `max` over {absent, factor}; "steel beats rubber" is `1000 ≫ 50`, a
consequence rather than a rule. A witness (§6) contributes no factor: it is no part of the field.

**Damage law:** every HP-bearing hard volume deposits transit damage **over its own presence
chords, at its own factor** — no ownership, no priority, no argmax. "A shell hurts you in
proportion to the material *of yours* it chewed." The error a component clipped into armor
introduces is bounded by `overlap_depth × its_factor`, and is only reachable if the shell actually
penetrated that deep — at which point it was hitting the component anyway. Two modules overlapping
each other: both deposit, cost charged once, nothing needs to win. A witness is dosed over its own
chord by its own law (§6), so two crew overlapping each other, or a crewman overlapping a wall,
each take their whole chord and charge the shell nothing.

**Spall law:** at every **body exit** — a step to a lower factor, or to air — a spall cone is thrown
with that body's own spend and speeds (§5), and its directions march under these same field laws.
Everything on the wishlist emerges with no class rules:

- Crew never spall: a witness is no body (§6), so it throws nothing.
- Crew pressed against an inner wall: the steel backface is a body exit into air as far as the
  field is concerned, and the cone it throws doses the crewman leaning on it (§6).
- Buried steel faces (overlapped plates) are no exits — no seam spall, no seam VFX.
- A small drop into other steel (RHA→cast) throws a cone the material behind absorbs in
  millimetres — self-correcting, no suppression rule needed.

### 13.3 Surface-law amendment — factor-weighted thickness (required for classlessness)

§4's overmatch consults **geometric** thickness (`caliber ≥ 3 × thickness`, `ballistics.rs:2253`).
Classless union breaks here: an exposed forearm is ~80mm "thick" → not overmatched → past 70° **an
arm ricochets an 88**. Amendment: overmatch, ricochet eligibility, and normalization consult
**steel-equivalent thickness** (`thickness × factor`) and gate on a **meaningful factor step** at
the surface. Rubber at 80mm ≈ 4mm-equivalent (DERIVED, factor 50) → overmatched by everything,
never deflects, never bends the path. (A crewman is a witness, §6, and has no surface law at all.) Bonus: fixes the latent thick-soft-volume bug (fuel, stowage, rubber) that exists
today independent of unions.

### 13.4 The walk (per sample ray) — welded runs

1. **All-hits collection** along the segment (not first-hit) — parry's double-sided triangles
   guarantee every crossing is seen.
2. **Pairing per CERTIFIED SURFACE, into `[enter, exit]` intervals.** Every crossing names the
   closed shell it came from (§13.6's manifold gate publishes its own edge-connected components)
   and the welded FEATURE the ray met — a triangle interior, a welded edge, or a welded vertex.
   Both are exact and neither is a tolerance:
   - **Identity is the dedup rule.** Claims are the same crossing when they name one contact of one
     shell, and never because their `t` are close. The collector deliberately lets every triangle
     incident on a feature claim a ray through it (a duplicate is recoverable, a dropped crossing is
     not), and the sheared edge test cancels to *exactly* zero on that feature in all of them, so
     the fan identifies itself. A ray merely NEAR a feature cancels nothing and is claimed once.
   - **A fan that disagrees about direction is a tangent** — the ray met the surface and left on the
     side it came from. No interval, no cost, no event. A fan that agrees is transverse and is one
     oriented crossing. Two contacts are two crossings however close they land.
   - **No threshold decides what counts as a hit.** Parallel is an exactly zero determinant and
     nothing else — a crossing whose determinant cancels to a fraction of an ULP is still a
     crossing, taken in exact integer arithmetic when the float filter's own error bound reaches
     zero. A shell thinner than any window is two different
     contacts and stays an ordinary crossing, charged, with its own entrance and exit. The contact
     parameter is carried at `f64`, from the collector's exact projection through to the cost
     integral, so two contacts keep their exact ORDER and a chargeable metric CHORD even where their
     public `f32` values coincide — a sub-ULP chord is charged like any other traversal. Only two
     contacts bit-equal in the *exact* parameter are refused by name: tangent grade, measure zero,
     no order between them and no thickness to charge.
   - **Occupancy is a boolean and alternation is required.** A certified shell is entered from
     outside and left from inside; a second entry is a violated certificate, not a deeper winding,
     and it is named. Embedding — no self-intersection within one shell — is proven at the bake
     (§13.6), so this is a backstop against a live collider drifting from the buffer that was
     certified, not the only guard.
   - Identity survives the RESTART: a corridor resuming inside material declares which *shells* it
     is inside, never which primitives. It is a runtime identity only — entity-keyed and dense in
     the bake's triangle order — and never crosses a save, replay or wire seam.
3. **ε-weld:** runs separated by less than the weld tolerance merge into one run. Semantics
   sharpened 2026-08-04:
   - **It merges event topology, not material.** One entry face (ricochet/normalization/incidence
     once, at the outer face), one exit at the true inner face, overmatch against the run's summed
     factor-weighted thickness. No second ricochet check mid-sandwich.
   - **Spall follows the body, not the run:** welding deletes only the void's exit/entry face
     pair; the material step it exposes is judged under §5 — a lower-factor substance behind a
     welded joint makes the joint a body exit with its own spend, an equal or higher factor behind
     it continues the body. A 0.4mm void never "develops" spall of its own.
   - **It never charges the gap.** Cost integrates material chords only — welding deletes phantom
     *faces*; it cannot create phantom *steel*.
   - **Measure the gap perpendicular, not along the ray** — else grazing incidence un-welds exactly
     when it matters most: `gap_perp = gap_along × |dot(dir, n_exit)|`. ε ≈ 2mm perpendicular
     (knob): far above export jitter, two orders below real spaced armor (Schürzen at 100s of mm).
   - Welds across differing factors are fine (RHA sandwiched with cast = one run for events; each
     chord charges its own factor).
   - **Chained welds are budgeted** (RULED 2026-08-07): chaining A~B~C is transitive, but the
     cumulative omitted void per welded run is capped at ε itself. Cap = tolerance = the same 2mm
     knob, deliberately — it bounds both extremes at once: a single two-plate sandwich welds
     across up to 2mm of air, and a picket fence of near-touching plates can never collapse into
     one run by chaining. Welding also requires **face compatibility** — the exit and next-entry
     normals must describe opposing sides of a common gap (cosine bound, bounded along-ray
     lookahead), else a grazing side face would weld to unrelated geometry far downrange.
4. **Walk the boundary list:** each sub-chord carries `max`-factor + the presence set. Integrate
   cost; deposit per-entity damage; fire boundary laws at steps. Interior gaps ≥ ε between runs are
   free flight — **spaced armor is emergent**.

Entry-face semantics (incidence, normalization, ricochet test, the terminal/`Impact` reads) evaluate
at the outermost *significant* factor step; spall at body exits (§5). **A boundary buried under
equal-or-higher factor is deliberately eventless** — it is not a field step, so there is no ricochet,
no spall, no impact and no normal to attribute, and the exposed outer boundary is read from the face
that actually moves the field. That is the qualifier on "a thin shell has real events": it has
ordinary entrance and exit crossings, and they are silent wherever the union hides them. The output per sample ray is a
sequence of welded runs `[t_in, t_out]` with chords, factors, and entry/exit faces — the inputs
§13.5 aggregates.

### 13.5 The shell is a sampled disc — the finalized primitive (2026-08-04)

> **The shell samples the world as a disc, not a point. The three-phase crossing — entrance,
> transit, exit — survives unchanged; every scalar it consumed from one axis point sample becomes
> an area aggregate over the frontal disc.**

**k sample rays** (the axis + a ring at `r = caliber/2`; k ≈ 8–16, knob), each marched through
§13.4's walk. A **crossing event** is the cluster of welded runs that overlap longitudinally across
samples. Per event:

- **Entrance:** `n̄` = area-mean entry normal over covered samples — yes, the normals *integrate*.
  Incidence, the ricochet test, normalization, and overmatch (§13.3, factor-weighted) consume `n̄`
  unchanged. This also *repairs* the point model: a point normal at an edge or on curved cast is
  degenerate (today's `unwrap_or(-dir)`); the patch average is smooth everywhere — corners average
  to their bisector. **Ricochet bleed, impulse exchange, AND the deflection angle scale by η**
  (RULED 2026-08-07 — angle scaling added): the outgoing direction blends from the incoming ray
  toward the full-ricochet direction by η, and normalization on penetrating entries scales the
  same way. η = 1 is the classic bounce; η → 0 fades to undisturbed flight. Continuity is the
  point — 1mm of aim must never flip the outcome between "flies past" and "full bounce". Whether
  a bounce happens at all stays with the factor-weighted overmatch law (mass/velocity live there).
- **Transit:** `cost = (1/k) Σᵢ costᵢ`, each sample's cost its union-max-factor chord integral,
  uncovered samples contributing 0 — automatically `η × mean covered chord cost`, where
  **η = the covered fraction, the engagement fraction**. Spend against capability unchanged.
  Damage stays per-presence, now per-sample: each HP entity deposits for the samples crossing *it*
  — a shell clipping the corner of a module damages it fractionally, and a witness reads the same
  disc (§6), so a graze of a crew volume doses it fractionally. Emergent.
- **Exit:** area-mean exit point/normal. Each body exit is grouped over the samples that reach
  it; its spend is the mechanism's readout drop across that body, shared among the samples by the
  factor each was resisted by, so it is η-weighted by construction (§5).

**What the uniform law dissolves** (each was a provisional rule during 2026-08-03/04 design;
none survive — recorded so they aren't re-invented):

- *Admission ("can the shell fit")* — there is no fit/no-fit, only material to defeat. Punching
  through an undersized opening = paying the rim annulus's η-weighted cost; embed when capability
  can't cover it.
- *Side-counting / arc tests / encirclement / "n bodies"* — box-world proxies. The turret-ring
  slit, the plate-edge graze, and the mantlet MG port are just different (η, n̄) pairs; the
  geometry around the axis is the fact, indifferent to how steel is partitioned into meshes.
- *Scrape thresholds* — an edge skim's covered samples see ~90° incidence on `n̄`, so the
  **existing ricochet law** fires with η-scaled bleed *and deflection*: a graze IS a partial
  ricochet. Continuous, no cliff, no lottery.
- *Nearest-contact re-aim / capsule shapecast* — the aggregates are the resolution; the "capsule"
  is implemented as the sub-ray ring itself (same raycast machinery, same determinism posture, no
  new parry query type).

**Emergent results:** the MG port in the cast mantlet is a *graded* weakspot (η ≈ 0.9 of the cast
chord — most, not all, of the resistance; zero authored weakspot data); an 88 cannot thread the
turret-ring slit while an MG round centered in it flies free (η = 0) — **caliber-gated weakspots
from geometry**; sub-caliber accidental cracks self-heal (η ≈ 1, near-full charge); a shell with
r→0, k=1 degenerates to the pure ray.

**Deliberate costs, accepted 2026-08-04:**

- **Armor-zone boundaries grade over ~one caliber** (partial-area engagement is the physical
  truth). Chosen over sharp-line legibility — the sharp line is what bred every seam pathology.
- **Lateral asymmetry unmodeled:** a corner clip physically torques the shell away from the plate;
  the aggregate sees the mean, not the asymmetry. If ever wanted, the mean lateral offset of
  covered samples is the ready-made kick direction. Knob, not now (re-affirmed 2026-08-07:
  torque/tumble stays deferred; only the η-scaled deflection magnitude ships).
- **Sampling noise at very small η** — k is the resolution dial; the sandbox chooses it.

**Impulse ownership across rigid bodies (safe default, 2026-08-07):** when an event's presence set
spans several *rigid bodies* (only possible when distinct bodies overlap in space — hulls in
contact, a shot threading a wreck seam; within one tank every volume shares the body, so the
question cannot arise), the full momentum exchange goes to the body owning the **entrance
surface** — the first body physically touched. Anything behind it is shoved through contact by
the physics engine. Damage attribution stays per-presence as always; only the shove is routed.
Revisit only if seam-threading shots ever matter.

### 13.6 Invariants — the machine-checkable contract

- **Idempotence:** duplicating any volume changes no outcome.
- **Monotonicity:** adding a volume never lowers protection along any ray; removing one never
  raises it.
- **Order independence:** outcomes are authoring-order-free.
- **Seam invisibility:** a shot near the seam of two flush same-material plates resolves
  *identically* to a shot mid-plate — the union field gives every sample the same answer; the
  shell cannot detect the seam. (This is also why a neighboring plate on the disc near a seam
  contributes nothing extra: including it would make seams *anti*-weakspots.)
- **Locality:** field laws are local to the overlap; boundary laws local to the overlap's *edges*
  (an overlap deletes transitions — buried faces — and creates them — steel→rubber contact).
- **No fabricated events:** a ray crossing no material produces no armor events (what makes gaps
  *detectable* rather than event-noise).

Enforced mechanically, not by eyeball:

- **Ray fuzzer** (bake/CI gate): 10⁵–10⁶ random rays at the bound tank, asserting the invariants,
  and reporting every corridor that reaches crew/ammo with its **admitting caliber and per-caliber
  η** ("this seam admits ≥8mm; at 88mm η = 0.93"). **Any such corridor fails the gate by name.**
  There is no exemption channel: a route into the crew compartment that no gun has to pay for is a
  defect in the model, and it is fixed in the model.
- **Per-primitive manifold gate:** weld-by-position, then closed-manifold + positive-orientation
  test per connected shell, on every substance primitive (classifier precedent 2026-08-07 — the gate
  keys off the material, not a name). Welding, degeneracy, closure, winding and signed volume are all
  judged on the authored buffer, and its edge-connected components ARE the surface identity §13.4
  pairs on, published rather than discarded.
- **Embedding, per shell, unconditionally.** Closure says a shell has an inside; embedding says the
  ray that enters it leaves it. For every same-shell triangle pair whose closed AABBs overlap, the
  exact intersection `I` must lie inside the feature `F` the two DECLARE they share — their common
  welded ids: none, one vertex, one edge, and three is one face exported twice. No adjacent pair is
  exempt; the exemption IS the declared feature. Every sign is exact — a float filter decides only
  where its own proven roundoff excludes zero, and coplanarity, collinearity and tangency are
  exact-zero. There is no flag and no allowlist: a shell that passes through itself charges a
  fraction of the plate it crossed, and §13.4's alternation cannot be relied on to notice, because
  a fan whose claims disagree about direction is a tangent that carries no evidence.
- **Unit scale is the authoring contract for ballistic geometry, at every seam it can be broken
  at.** A componentwise scale is injective over the reals and not over `f32` — two authored
  coordinates can land on one, turning a positive-thickness shell into a zero-thickness one that the
  gate has already passed — so the model carries the scale in its vertices, never on its nodes.
  Three refusals, all bit-exact against 1 and none of them a compensation: the bake refuses a
  substance primitive whose composed node scale is not 1, naming the node (fix it in the .blend and
  re-export); the spawn refuses a tank root spawned at any other scale, because a scaled root moves
  and shrinks every collider's world AABB and the swept query can then miss the armour entirely,
  leaving nothing downstream to refuse; and the collector refuses a live collider that reaches the
  world at any other scale, which is what catches a runtime mutation of a hierarchy that spawned
  clean.

### 13.7 Authoring contract (amends the §12 mesh bullet; revised 2026-08-04)

- **Watertight per volume is the only hard mesh gate. Mutual exact-fit is retired as a
  *requirement*.**
- **Face-to-face contact is the preferred joint style** (Yan ruling 2026-08-04, revising the
  earlier "overlap recommended"): physically honest — real plates are separate bodies joined at a
  seam, not interpenetrating — and the garage **x-ray inspection** renders clean, where visible
  overlaps would confuse the player. **Overlap is tolerated, not recommended** — the union law
  makes it harmless (charged once), it's just not the style. Avoid exactly-coincident *coplanar*
  faces (z-fighting in the x-ray render).
- **Sub-caliber slop is harmless in both directions** — small overlaps union away; micro-gaps
  ε-weld (longitudinal) or self-heal at η ≈ 1 (lateral cracks). Don't sweat sub-caliber precision.
- Components may clip into walls; the damage law makes the slop harmless.
- **Real openings are modeled honestly, not plugged** — the turret ring, MG ports, vision slits
  become caliber-gated, η-graded weakspots by the disc law. On the Tiger they are modelled as solid
  cast steel, so the disc law has no geometry to grade and the fuzzer finds no corridor at all.
- The remaining geometry sin is the **super-caliber accidental corridor** — hunted by the fuzzer,
  not the eyeball.
- **Plates that touch face to face are separate PRIMITIVES.** Two closed shells of one primitive
  sharing a welded edge put four triangles on it, and the manifold gate refuses that before shell
  identity is assigned — deliberately, because guessing geometrically where one shell ends and the
  next begins is exactly the ambiguity §13.4's pairing exists to remove. Inside one primitive the
  legal contacts between islands are a shared vertex and interior intersection; face-to-face contact
  between *primitives* is unaffected and remains the preferred joint style.
- **Substance boundaries are geometry, never paint** (2026-08-07, wheels precedent): a
  mixed-substance part is authored as one object holding **one closed shell per substance
  region**, each shell wearing its substance material — the Tiger road wheel is five islands
  (two steel bodies, two rubber rims, one axle) in one object with `[MildSteel, Rubber]` slots.
  Manifold is judged per shell, not per object; multiple closed shells per object are legal and
  expected. Assigning a substance to *faces* of a shared shell is forbidden — the walk pairs
  entry/exit per closed surface, and a painted-on boundary has no exit face to pair.

**Open tabs (§13):** k (sample count) and the ring layout; the weld-ε value; whether ricochet-shock
on exposed components adopts the same factor-step gate; the
lateral-asymmetry kick (§13.5, deliberately omitted). Material near-ties need nothing: `max`
needs no tie-break for cost, and damage has no ownership at all.
**Track belt ballistics (RULED IN, 2026-08-07, design pending):** Yan — links, sprockets and
idlers "should be part of the ballistics, and track links will even have health later."
Sprocket/idler are rig-mounted and land as ordinary Cast volumes. The BELT is the open design:
194 links on a dynamic wrap — ballistic volumes extract from static glb nodes, so shootable
tracks need a belt-geometry→volumes mapping (the collider cousin of the track wrap view),
plus the future per-link health model. Until it lands, shots pass through the belt run
unresisted — a known, deliberate gap, not an oversight.

## Build status (2026-06-27)

- **Done & verified:** `shooting` → `ballistics` split (shared mechanic, trigger-agnostic `FireShell`
  event); `bin/armor_sandbox` + `sandbox.rs` — free-fly camera-gun (heading-relative WASD,
  Shift/Ctrl altitude), pause + 5-step slow-mo + `[`/`]` fine time control, shell tracer gizmo.
  **Geometric march cut:** `Layer::Armor` + `BallisticVolume`; the step march crosses volumes
  recording entry/exit (geometric line-of-sight thickness via a `solid=false` exit probe restricted
  to the same entity), stops at terrain, handles multiple volumes per shot; `PenetrationMarks`
  inspection gizmos (entry/exit/through-span). `cargo test` green.
- **Velocity cost cut:** `capability(speed) = K·speed^N` (DeMarre) and its inverse; a crossing spends
  `cost = LOS_metres × material_factor` and drops to the residual speed (Lambert–Jonas shape). When
  capability ≤ cost the shell **embeds** partway and stops. Placeholder plates now show
  perforate-then-embed. `0` joins `P` as a pause key.
- **Path bending cut:** `integrate_projectiles` is now a true ray-march carrying position +
  direction + speed, so bends survive across the step. **Normalization** straightens the round toward
  the inward normal on entry (a share of the incidence); **ricochet** deflects (specular, speed
  bled) off faces past ~70° without entering — the deflected segment lives in world space and can hit
  the next surface. Ricochet points drawn as cyan markers.
- **Information layer:** `ballistics` exposes a per-shell `ShellReadout` (speed, remaining
  capability); the sandbox draws a keybindings legend, a status line (time scale + shell count), and
  pooled labels floating beside each shell (speed / capability / plates crossed) via
  `world_to_viewport`. "Slower" is now a number.
- **Overmatch cut:** the shell carries a `caliber`; at each crossing the march probes the plate's
  thickness *along its normal* and, when `caliber ≥ 3 × thickness`, suppresses ricochet and nearly
  fully normalizes (cancelling slope). Sandbox plates are now a steel thickness ladder
  (15/50/100/300 mm); overmatched crossings draw magenta.
- **Real model bound:** `on_tank_ready` attaches a query-only trimesh collider (`Armor` layer,
  `filters = NONE`, so no physics response) + a `BallisticVolume` to each `Armor_/Module_/Crew_/Ammo_`
  node; the march resolves the volume by walking up from the hit mesh-primitive to the named parent
  (`ChildOf`). Material factor is **provisional, role-keyed** (`ballistics::material_factor`) pending
  RON authoring. The sandbox now spawns the real Tiger as a **static** target (reusing `on_tank_ready`
  via `spec`), alongside the placeholder slabs. Game + sandbox bind with no panic.
- **Spall + HP cut (2026-06-28):** per-component HP pools (`ComponentHealth`, role-keyed
  `component_hp` — crew 3 / module 10 / ammo 2; armor 0), bound to `Component` nodes in
  `on_tank_ready`. On every perforation exit the march throws a fixed-shape cone (symmetric, exit-dir
  axis, deterministic golden-angle fragment pattern denser on-axis); budget `= MAX × (cost/ref) ×
  (v_res/ref)² × (caliber/ref)` (§5 product model). Each fragment ray-casts to the first ballistic
  volume, deposits 1 HP, and stops (armor shadows for free). The main penetrator's transit deposits
  `cost × TRANSIT_K` into a crossed component (embed deposits `cap`). Sandbox draws spall cones +
  fragment rays (hot = HP deposited, grey = shadowed) and floats HP labels over damaged components;
  `c` resets component HP. `cargo test` green, sandbox runs clean. Constants are sandbox knobs.
- **Air drag cut (2026-06-28):** shells now bleed speed in flight — quadratic drag `dv/dt = −k·v²`
  integrated analytically (`v ← v/(1 + k·v·dt)`, stable), lumped const `DRAG_K` (sandbox-tunable). The
  point is **range-dependent penetration**: `capability ∝ vⁿ` now falls with distance, so a far shot
  can bounce where a near one perforates — visible live via the shell's speed/capability label. One AP
  value for now; the per-shell ballistic coefficient (the APCR-vs-APDS range-falloff differentiator)
  joins the shell-data struct later. Fragments stay hitscan — their drag will be a distance term in
  the future energy-packet model, not a flight integrator. Deliberately *not* doing full exterior
  ballistics (wind, altitude density, spin drift) — out of slice.
- **Energy-packet fragment cut (2026-06-28):** spall fragments upgraded from 1-HP/no-pen tokens to
  energy packets. `spall_directions` now returns each fragment's on-axis position `t`; count scales
  with `cost × caliber`, per-fragment birth penetration = `FRAG_PEN_MAX × shot_energy(v_res²) ×
  (1−t)`. New `cast_spall_fragment` marches each as a mini-penetrator: deposits `pen ×
  FRAG_DMG_PER_MM` HP on a component, then punches through thin volumes (spending `span × factor`) or
  stops in thick ones; `pen` bleeds with distance via `FRAG_DRAG`. Thick volumes still shadow; thin
  ones can be defeated; strong fragments can exit + reach another tank. Sandbox draw unchanged (reads
  end + deposited; rays now run longer when a fragment penetrates). Consts `FRAG_PEN_MAX/DRAG/
  DMG_PER_MM` are sandbox knobs. `cargo test` green, sandbox runs clean.
- **Mass-driven penetration cut (2026-06-28):** capability went from speed-only to
  `PEN_K · mass^MASS_EXP · speed^PEN_N` (MASS_EXP 0.5) — mass is the primary driver, caliber stays for
  overmatch/spall only. `FireShell`/`Projectile` carry `mass`; `capability`/`speed_for` take it.
  Re-calibrated `PEN_K` (0.01853→0.0058) so the 88 (10.2 kg @773) = 250 mm *unchanged* (zero
  regression); a 13 g MG round now lands ~9 mm RHA, so small arms can't defeat real armour but chip
  exposed modules. The linchpin for small-arms-vs-main-gun tiering and future shell types. `cargo
  test` green, sandbox clean.
- **Ricochet-shock cut (2026-06-28):** a ricochet off a *component* (not armor) now deposits shock
  damage `SHOCK_K · capability · cos(incidence)` — scaled by impact energy and squareness. So a
  grazing main-gun bounce chips an exposed module's integrity without one-shotting it (~3.7 of a 10-HP
  module at a 71° graze, ~0.4 at 88°), a bullet graze barely registers (~0.1), and armor (no HP) takes
  nothing. Completes the gun-barrel damage story (direct = knockout, graze = chip, fragment = energy-
  scaled). `SHOCK_K` is a sandbox knob. Not yet drawn distinctly (HP labels show the effect).
- **Volume data → RON (composition) cut (2026-06-28):** retired the prefix-keyed `material_factor`/
  `component_hp` functions; the per-tank `volumes` RON map is now the source of truth. `VolumeSpec`
  = `material_factor` + optional `hp` facet (composition, not a `kind` enum — §12). `on_tank_ready`
  binds by iterating `spec.volumes` (node is a volume iff a key), `hp` present → `ComponentHealth`,
  absent → `ArmorVolume`; prefixes demoted to a drift lint; every RON key asserted to have a node.
  All 45 Tiger volumes authored + bound clean; resistance now decoupled from role (barrel = steel
  1000 *and* a module). Schema test covers it. **Consequences (§§7–8) now slot in as new facets.**
- **Single `Ballistic_` prefix cut (2026-06-28):** retired the four `Armor_/Module_/Crew_/Ammo_`
  role-prefixes (a second source of truth for role, now owned by RON) → all 44 volumes are
  `Ballistic_<part>`. Renamed across `.blend` (headless Blender), `.glb` (surgical JSON node-name
  patch — visual untouched, no re-export), the RON keys, and the bind's drift lint. The model's mesh
  kinds now split by *purpose* (ballistic / visual / collider / rig), not role. Bonus: the merged
  barrel mesh (Blender kept the name `Armor_Gun_Barrel`) became `Ballistic_Gun_Barrel` and the RON's
  `hp` makes it a module — name no longer matters. Binds clean, zero drift.
- **Model restructure cut (2026-06-28):** rig nodes (`Hull`, `Turret`, `Gun`, `Gun_Barrel`, 16
  `Wheel_*`) were meshes doing double duty as visual + pivot; converted to **Empties** (pivots) with
  their geometry demoted to `Visual_*` leaves — done headless via Blender (`mesh→empty` + transform-
  preserving re-parent), re-exported, verified (74 meshes / 3 materials / 3 images unchanged, modifiers
  0). Running gear → `Visual_*`; `Hull_Collider`→`Collider_Hull`, dormant `Turret_Collision`→
  `Collider_Turret` (now a proper convex proxy). Code: collider match → `Collider_` prefix. Three
  shape kinds (Visual / Ballistic / Collider) now split by purpose; rig is a pure empty skeleton. Binds
  clean. **Visual skin needs a human eyeball in the sandbox** (counts match, so geometry should be
  intact; backups in scratchpad).
- **Next:** crew position capabilities + per-crew capability gating (§7a). `CrewStation` (enum)
  → `Position { capabilities }` (composition). v1 capability set = {Drive, Traverse, Fire, Reload,
  GunnerSight, CommanderView}. Capability available iff staffed by an active crewman AND relevant
  module HP > 0. `Reload` becomes a `Gun` component (per-gun, not a singleton resource). View
  auto-fallback on a view crewman's knockout. `TankKnockedOut` retired as a gameplay gate (§1a-A); the
  cookoff turret-launch hook moves from `TankKnockedOut`-triggered to `CookedOff`-triggered. Slice
  2 (§7b — backfill/position swapping) is the next design interview.
