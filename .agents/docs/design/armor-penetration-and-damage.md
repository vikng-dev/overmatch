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
   The nose factor `N` is the frictionless nose factor of the cavity-expansion form, in `(0, 1]`. It is **shell data authored as geometry**: each weapon in the tank RON carries a
   required `nose` (parameters in calibres), and `spec.rs` derives `N` once at load — `Ogive(crh:
   ψ)`, `ψ ≥ 0.5`: `N = (8ψ − 1) / (24ψ²)`; `Cone(length_calibres: φ)`, `φ > 0`:
   `N = 1 / (1 + 4φ²)`; `Cap(radius_calibres: φ)`, `φ ≥ 0.5`: `N = 1 − 1 / (8φ²)`; `Flat`:
   `N = 1`. A parameter outside its range fails to load. The derived scalar travels with mass and
   calibre through `FireShell`, `Projectile` and the `FireEvent` wire; the shape never leaves the
   spec layer. Fixture noses are labelled: `Ogive(crh: 1.0)` for the main gun,
   `Cap(radius_calibres: 0.5)` (a hemisphere, `N = 0.5`) for the machine-gun rounds. Calibre enters raw penetration through `A`: at equal mass a wider round
   presents more area and goes less deep. The law's invariants: strictly increasing in speed;
   replica scaling (mass ×s³, calibre
   ×s gives capability ×s); the local speed exponent is 2 at low speed (the inertia term is
   negligible) and falls below 2 as it grows; and spending composes, because a crossing spends
   reference-mm and the inverse is exact. A crossing whose limit speed is `v_l` leaves
   `v_r² = (v² − v_l²) / (1 + k v_l² / a)`.
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

**Strength.** `S = spend × κ × (v_exit / v_entry)²` (reference-mm, on the axis). `spend` is what
the round's mechanism spent crossing that body (its readout drop, η-weighted by the disc);
`v_entry`, `v_exit` are the round's speeds entering and leaving it. `κ = f_rear / f_ref` is the
debris competence of the exit material (owner, 2026-10-17): `f_rear` the resistance factor of the
body's last layer before the exit — its highest, since a body never steps down — and `f_ref` the
reference factor (RHA, 1000). κ is exactly 1 for the reference, so a steel exit's `S` keeps its
bits; every other material scales by its own data, and soft bodies fall to noise on their own
(`.agents/scratch/damage-cam-2026-10-08/research/debris-strength-law.md`). A barely-through round
has a small energy ratio. Calibre has no role.

**Profile.** Shipped: **even** — every direction of the cone carries the same strength, `S/2`
(owner, 2026-10-17): the player controls the shell and learns the cone's shape; a fade across the
angle is nuance outside his control. Kept as a data dial (playtest fork F6, `profile: Linear`):

> `s(θ) = S · (cos θ − cos α) / (1 − cos α)` inside the cone, `0` outside,

strongest on the axis and zero at the rim, the measured fall-off of fragment fluence off the debris
axis; the same emitted total.

**Axis.** At a genuine exit (material to air through a rear face), `normalize(n_rear + d_exit)`,
`n_rear` the face's outward normal and `d_exit` the round's direction; at an internal step (no face
of its own), `d_exit`. ENGINEERING ASSUMPTION with MEASURED support (Verolme 1997; Recht & Howell
1967: 25° measured against 22.5° predicted at 45° obliquity).

**Sampling.** `N` directions in equal-solid-angle strata, `μ_k = cos θ_k = 1 − (1 − cos α)(k + ½)/N`,
golden-angle azimuth, each standing for `1/N` of the cone. The emitted total `(1/N) Σ s_k` is `S/2`
at every `N` under either profile (the midpoint rule is exact on a profile linear in `cos θ`): `N`
is resolution, never balance. Shipped `N` = 64 (owner, 2026-10-17), the throw's cost against its
resolution (one throw MEASURED at each count in `assets/ballistics/spall_cone.ron`'s provenance). The strata are generated with `libm` so their
bits match on every shipped target.

**Shadowing and reach.** A direction's strength is constant in air — no drag, no range — and it
doses only the struck tank's boxes (hitting another tank is a preserved alternative). Shipped:
**full shadowing** (owner, 2026-10-17) — a direction stops at the first hard body it enters: armour
takes nothing and stops it, a hard box takes its whole strength and stops it, so whatever hard
body lies first shields everything behind it, whatever its material. Kept as a data dial (F6,
`passage: Partial`): a direction spent by `chord × factor` through every hard body, stopping where
that exhausts it or where it leaves the struck tank, dosing each box it enters at its strength
there — graded shadowing (a plate nearly always stops it, an engine block mostly). Under either, a
witness (§6) never stops or spends a direction: each one it enters short of where it stops is
dosed at its strength there, so crew never shield crew. The `1/r²` fall-off is not a parameter: a
box farther away covers fewer directions.

**Walk.** A direction is walked like the line's corridors: the collector and the pairing walk along
the whole ray through the struck tank, from outside every candidate's bounds through the throw
point, with nothing declared. So what it starts inside is established, never assumed, and every
volume along it is present in order, nested and overlapping ones included. No nudge displaces the
ray: the volumes the exit leaves are recognised by identity, so a direction starting inside one of
them has not entered it and does not dose it there (under the partial passage it still pays for
that material). Under full shadowing a direction that starts inside another volume has entered it
at the throw point.

**Dose.** A box's dose from one cone is `D_b = (1/N) Σ s_i` over the directions that dose it —
under full shadowing those stopped in it, under the partial passage those entering it, and a
witness every one entering it, at their strength there — each direction dosing a box once, in the count unit (reference-mm), never scaled
by the box's own factor; armour (no count) takes nothing. Doses go through the addressed `Dose`
effect, like the line's. A box's dose from the whole shot (the line, a bounce's shock and every
cone) becomes whole HP points once, on its total (§6). Every field deposit, to a hard box and a
witness alike, enters that total at the field's own calibration (`hp_per_reference_mm` in
`spall_cone.ron`, TUNED 70 HP points per reference-mm against the line's 10) and nothing on the
line or a bounce's shock does; `S` stays in reference-mm, so the calibration sets every cone's
potency at once and the HP quantum gate tests `S/2` at it. A box of face `A` fully inside the cone
has the knockout radius `R = sqrt(E A / (pool Ω_c))`, `Ω_c = 2π(1 − cos α)`: the same for every
box of a class whose pool is class × `A` (§6), and on a weak ordinary perforation about 2 m for the
30 HP crew pool at the intended humanoid's 0.2 m² face, 3.2 m at the placeholder crew box's
0.516 m² (DERIVED in `spall_cone.ron`). Dose summed over boxes is not conserved under the partial passage (two thin
boxes in line both take nearly full strength); the emitted budget is.

**Potency colour.** The pen cam colours a field by its potency, never by a display gain (decision
record `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/decision-2026-10-23.md`,
#8). Each of the `N` directions stands for `Ω_c / N` of the cone and deposits `s / N`, so a
direction of strength `s` emits `ρ s / Ω_c` HP points per steradian (`ρ` = `hp_per_reference_mm`),
and at distance `r` along it, inside the cone and short of its reach, the **local intensity** is
`I = ρ s / (Ω_c r²)` HP points per m², zero past the reach; a thin unshadowed box of face `A` there
takes `I · A` (resolver test `the_pen_cam_intensity_times_a_face_is_its_dose`). The thresholds are
the classes' (`assets/ballistics/box_classes.ron`): **bright** `T_bright` = the hard class (300),
**mid** `T_mid` = `crew_hp / crew_face_m2` (30 / 0.2 = 150, DERIVED; the ammo class coincides
today, a data coincidence). The colour is one function of `I` alone (`ramp`, `Ramp` in
`src/damage_cam/pen_cam/potency.rs`) on `u = log2(I / T_mid)`, which the inverse square makes
linear in `log r`, so the colour runs evenly along a direction: the deep teal at `u ≤ −3` (an eighth
of mid and below; the span TUNED), the field teal exactly at mid (`u = 0`), the near-white exactly
at bright and above (`u = log2(T_bright / T_mid)`, 1 with the shipped classes, DERIVED), a monotone
cubic between, so it is smooth and never falls as `I` rises. Along a direction the intensity
reaches mid at `sqrt(ρ s / (Ω_c T_mid))` and bright at `sqrt(ρ s / (Ω_c T_bright))` from the origin,
where that is short of the direction's reach; on the s1 record (`S` = 10.18 reference-mm, flat
`s = S/2`) bright ends at 1.42 m and mid at 2.01 m (DERIVED), the crew knockout radius above. The
promise, white is bright and teal is mid: a hard box whose face lies wholly at white has its pool
emptied, and a crew station at the intended humanoid's face wholly at teal or whiter is knocked out.
A box only partly inside the cone takes that intensity over that part alone, so a white patch on a
box with HP left is no contradiction (s1's transmission: one direction ends on it, at 1.35 m, white,
DERIVED from the record; 6 of its 479 HP points lost). The pen cam draws each cone as one
translucent volume inside its reach surface (`src/damage_cam/pen_cam/field.rs`, `reach_mesh`;
`assets/shaders/pen_cam_look.wgsl`, `volume`): a pixel takes the mean colour along the eye ray's
path inside the field, at an opacity that climbs with that path's length, so the colour says the
potency and the opacity the thickness. Every surface the field met is tinted in the colour at its
own point and lined along the cone's edge. Volume and surfaces read one disc baked from the
record's directions. The calibration is provisional until the emergent shape lands.

**Data.** `α`, the axis rule, `N`, the profile and the passage live in
`assets/ballistics/spall_cone.ron`, parsed and validated while the app is built (ADR-0014: built
from data, synchronously); a half-angle whose `cos α` rounds to 1, or a shape not emitting `S/2`
within the rounding of its weights, is refused. `α` = 25° is TUNED/provisional (owner 2026-10-12;
MEASURED normal-impact outer extents ~26°, Qi et al. 2026). The half-angle and the axis are playtest
fork F5, the profile and the passage F6 (`.agents/scratch/playtest-forks/README.md`).

**Class invariants** (tests hold them for any tank and any cone; fixture numbers are fixture data).
Under every profile and passage: exact emission `S/2` at N ∈ {1, 7, 24, 97}; resolution stability;
homogeneity in `S` where no direction is exhausted; inverse square for an unshadowed thin box;
factor independence of a box's own dose; shadowing monotone (adding a volume never raises a dose).
κ is 1 to the bit for the reference and proportional otherwise; a cone below the HP quantum is not
thrown; a shot's dose to a box is rounded once on its total. Full shadowing: a direction stops at
the first volume it enters, a box there taking its whole strength, armour nothing. The partial
passage: a dose never falls as the strength rises through a plate; shadowing monotone along one
direction through randomized stacks; a direction pays for the material it starts inside, a nested
shield shadows, a box inside a traversed volume is dosed, a plate thinner than any nudge is
charged. The linear profile is maximal on the axis and zero at and past the rim. Witnesses: adding one never
changes the shell's state, any direction's reach or any hard box's dose; a witness behind a witness
is dosed as it would be alone. The shipped cone's
walks, and six whole main-gun shots at the bound tank (exit detection, spends and speeds, strength,
axis, walks, HP points), are digested on every target by `src/ballistics/field_casts.rs`; the force
law's transcendentals are `libm`'s, so a shot's spall sources are the same bits everywhere.

**Not built.** The fragment cone (a bursting round's own fragments, carrying its velocity) and the
blast sphere; plate failure mode (coherent plug vs distributed debris) selected by material and
impact conditions. **HE** stays a separate, penetration-independent mechanic: a fuse triggers on
minimum steel-equivalent thickness, then detonates after a delay.

## 6. Component damage — HP per component, never per tank

Every component has its own **HP pool** in whole **HP points**. A crew station's is the one shared
crew pool (30 HP, TUNED, provisional, `assets/ballistics/box_classes.ron`), which a tank file
cannot author: a crewman is one entity across tanks, and the crew box is a placeholder for a
humanoid shape. Every other pool is authored as an integer in the tank data, its sturdiness class
(HP points per m², same file, TUNED) times the box's mean face, the mean of the three axis-aligned
face areas of its bounds (an engine 336 on the seed tank, DERIVED); `src/tests/box_pools.rs` holds
every tank asset to both, which the field's calibration (§5) and the pen cam's potency colour rely
on. One
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
  `rounds.ron`, TUNED);
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
