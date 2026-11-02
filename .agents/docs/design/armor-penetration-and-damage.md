# Design sketch: armor penetration & module damage (the ballistics simulator)

Subject: the game's damage model; game state only. Crew are HP pools at stations.

**Status: SPEC for the in-progress build (2026-06-27).** Decided in a design interview; being
implemented now, starting with an isolated sandbox (§11). Graduates to ADR(s) if it survives
contact — the `ballistics`-as-library-plugin + `shooting` split (§10) is the most ADR-worthy part.
Vocabulary from `.agents/GLOSSARY.md` and `.agents/skills/codebase-design` (seam, depth, leverage).

This is the next vertical-slice mechanic after the gunner's sight (`design/gunner-sight.md`). It is
deliberately deep and physically-grounded; §9 names what is *out* of the first slice so the depth
doesn't masquerade as scope.

## 1. The knockout model — crew is the only health

There is **no tank HP**. A tank is knocked out when **fewer than two crew members are in action**
(§1a). Everything else is
emergent and *repairable* — only ammunition is terminal. Three paths empty crew:

- **Direct hits** — a penetrator or spall fragment reaching a crewman.
- **Engine fire** — damages nearby crew slowly, by proximity, over time.
- **Ammo cookoff** — detonation of an ammunition volume; **instantly knocks out all crew**.

Module damage (engine, breech, optics, transmission, …) *never* knocks the tank out — it degrades
capability and can be repaired. Ammunition is the one exception.

### 1a. Knockout threshold — fewer than two crew in action (#227)

(Playtest fork F2, resolved: `.agents/scratch/playtest-forks/README.md`.)

`knockout_from_counts` (`src/damage.rs`) is the one rule: a tank is knocked out when fewer than two
of its crew members are in action, whatever emptied their HP pools. A cook-off is a cause the rule
decides, not a second rule: it empties every crew pool of its tank at once, and the label names it.
Capability stays per requirement (§7b), composed from station competence and module condition;
`TankKnockedOut` is a label (HUD, scoring, the Battle), never a capability gate. A tank with every
module out of action stays in the Battle while two crew members are in action.

The rejected alternative, knocked out only at zero crew in action, let a lone survivor staff one
position (drive or shoot); the owner chose the earlier knockout, so that state never arises.

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

For a fixed round (mass, calibre, nose factor), bulk work is a monotonic, invertible function of
velocity. So **velocity is the stored state; penetration *capability* is its derivative.** The
march, per volume crossed:

1. `capability = f(round, velocity)` — its bulk-work readout in reference-mm, by the
   force-law (Poncelet) form:

   ```
   A        = π c² / 4                              presented area (m²), c = calibre
   k        = B · N · RHO_REF                       inertia coefficient (kg/m³)
   depth(v) = m / (2 A k) · ln(1 + k v² / a)        bulk reference work, metres
   C        = 1000 · depth(v)                       bulk capability, reference-mm
   v(C)     = sqrt((a / k) · (exp(C/1000 · 2 A k / m) − 1))
   ```

   The bulk form reads material facts. The reference steel is armour-class rolled homogeneous steel, given by its
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

   **A sub-calibre round meets the plate as its penetrator** (owner, 2026-10-23;
   `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/decision-2026-10-23.md` #4).
   A round may author a `penetrator` (mass and calibre; its nose is the round's), no heavier and
   no wider than the round; without one the round is its own penetrator, bit for bit. Flight reads
   the flying body: the whole round for a kept carrier, the penetrator for a discarded one
   (§3a). Everything at the plate reads the penetrator: the disc the shell samples the world with,
   contact, this law and the core's work (§5, §6). On entering material, a kept carrier stops
   at the first exposed resisting contact: that construction retains its kinetic energy and
   momentum once, while only the core flies straight on. A discarded carrier contributes none.
   The core's penetration law is unchanged. A skid keeps the current flying assembly (§4a). Penetration stays rigid: the penetrator's
   material is named (`core`) and read by no law. The 88's Pzgr. 40 reads about 180
   reference-mm at the muzzle (930 m/s) as its 1.93 kg, 35.7 mm core, against about 112 as its
   whole 7.3 kg body and the Pzgr. 39's about 115 (DERIVED, shipped RHA facts); the record gives
   it more than the Pzgr. 39 at the muzzle by about 1.4× (Jentz & Doyle), the law 1.57×.
2. `cost = LOS_distance × material_factor × slope charge × plate-capacity share` (§3b), also reference-mm.
3. If `capability > cost` → **perforate**: spend `cost`, invert `f` back to a reduced **residual
   velocity** (the Lambert–Jonas shape — barely-penetrate exits slow, big overmatch barely slows),
   and throw spall (§5).
4. Else → **embed** and stop.

Properties:

- **Modules tax the penetrator too** — they are volumes. A shell can run out of steam *inside* the
  tank after crossing the outer plate plus an engine block.
- **The path is a multi-segment world-space ray** that stays straight through bites and deflects
  at a ricochet. A segment can leave one tank and strike another (skip off a glacis into a neighbour).
- **Deliberate omission:** *shatter* (penetration going non-monotonic at extreme velocity against
  hard armor) is out of the slice.

### 3a. Flight-nose drag

Subject: the game's damage model; game state only. Issue 114 keeps the quadratic flight law
`dv/dt = -k v²`, with `k = form × calibre² / mass` (1/m), and changes the form to
`base + nose_share × flight_nose.factor()`. The shape factor is the same generic geometry
function the hit nose uses. No law reads a round type's name. The range table, live flight and
network catch-up all read the one coefficient resolved at load.

The TUNED blunt anchor retains form **0.263** on a **1-crh ogive**, the previous hand-set
**88 mm, 10.2 kg** fixture (anchor data). The TUNED base share assumes drag coefficient **0.20**
(ASSUMED); at uniform air density **1.225 kg/m³** (ASSUMED), this is **0.0962** form units
(DERIVED, coefficient × density × pi / 8). The nose share is **0.5718** form units (DERIVED from
those anchors and the blunt factor **7/24**). The blunt form is preserved to the bit; the
coefficient has no speed dependence. These are provisional game tuning anchors.

The data gives APCBC a **4-crh** flight ogive over its **1-crh** hit ogive (TUNED, accepted
starting point); AP keeps its fixture hit nose and flies on a **6-crh** ogive (ASSUMED typical
pointed bullet, not a measured shape). APCR flies on its pointed ballistic cap: **4 crh** is
Unknown pending a dimensioned cap drawing; its blunt hit nose and kept carrier remain independent.
Flight and hit proxies are independent data; a round's own nose override changes only the hit
proxy. A discarded carrier resolves the runtime flying body to its penetrator for drag, visual
size and delivered momentum. The launched assembly remains available for recoil and domain
validation: a carrier discarded after launch still contributes to the firing impulse.

Both the energy dose (§6) and flight-nose drag ride unshipped protocol REV 36: peers must agree
on dose units and trajectories. The scenario recorder and field-cast tool regenerate their
pins from the combined simulation, while the guns gate checks every resolved round field.

### 3b. Plate capacity below one calibre (#263)

Subject: the game's damage model; game state only. `PlateCapacity` owns the potency work share.
For covered normal geometric thickness `t`, calibre `d` and `x = t/d`:

```
g(x) = 1                                      x >= 1
       min(a x, max(b, x^p))                 0 <= x < 1
work charge = g(x) * slope charge
```

The shear branch makes thin-plate work proportional to thickness squared. The opening plateau
limits the middle of the series; the power branch returns continuously to bulk. This is an
**empirical StandIn**, not a sourced physical account of plate opening.
`assets/ballistics/physical_laws.ron` supplies **StandIn** `a = 1.35`, `b = 0.49`, `p = 1.5`:
`a` fits the low-thickness F band; `b` fits middle work; `p` fits the upper sub-calibre rows while
preserving calibre ordering. Inverting the shipped bulk law on the comparison's F values gives
the work shares these three branches approximate. No per-round or per-tank number is added.
Parameters are required and admitted before play: a >= 1, b in (0, 1), p in (0, 2), all finite.
The exponent bound follows from area proportional to calibre squared: `g(x)/x²` must decrease
with x so widening at fixed mass cannot lower the limit. Each branch, and their min/max,
preserves that ordering. Capacity reads geometric thickness; softening a thick body cannot turn
it into a thin plate.

Rejected: `min(a x, b + (1-b) x^6)` fits the F band but violates that class invariant. With the
15 lb, 1-crh fixture on 76.2 mm plate, widening from 76.2 to 77.724 mm lowers its limit from
653.524476 to 646.524042 m/s (**DERIVED**). The return branch, not the firing record, is wrong.
The shipped tests sweep small widenings as well as doubling; no losing arm remains in code.

`DiscEvent::resistance_charge` owns that read for the force and advance
consumers. `ResistanceCharge` scales resistance coordinates, including retained work, emitted
work and readout cost; geometry, consumed progress and flight time remain on the real chord.
The bulk capability/speed inverse still describes reference steel work. For a head-on reference
plate, its limit is now `speed_for(body, 1000 t g(t/d))`. Every plate at least one calibre thick
keeps its previous potency, beyond floating rounding; tests reject a mutant charging that bulk.
Replica scaling, thickness monotonicity, continuity and mass/nose/calibre ordering are class
invariants. Firing rows remain comparisons, never test pass conditions.

**Shared-pressure stop rule.** The existing skid term is `P = min(R, 4τ x sqrt(x/s))`,
`s = R/(4τ)` from the reference facts. Treating P as the potency law's strength gives the 1-crh
fixture limits **24.40 / 73.49 / 186.81 / 457.87 m/s (DERIVED)** at x = 0.10 / 0.24 / 0.50 /
1.00. Treating its normalized plug work as a chord share, `g = sqrt(x/s)`, gives
**97.39 / 188.53 / 330.53 / 573.07 m/s (DERIVED)**. Both miss the ±1,000 F band; the latter
misses by about **+7,500 / +3,300 / +2,400 / +7,500 F (DERIVED)** on those rows. Its neutral
point is `s = 1.607022 (DERIVED)`, whereas potency here must be neutral at one calibre. Sharing
one value would require a reader-specific normalization and, physically, a relation between
integrated opening work and contact holding pressure that the reached record does not supply.
The skid stand-in therefore **stays**, unchanged, with its existing reason; the potency fit does
not claim to retire it. A shared physical work/pressure relation and the bulk mismatch remain
for the owner. No search outside the repo was made and **no numeric M79 source series was reached**.

The ignored `measure_plate_limit_speeds` tool uses a **Fixture** body of 15 lb and 3 in and the
two existing steel hit-nose proxies (1-crh and hemisphere). NPG 1125 Fig. 3 provides **BY EYE**
F (±1,000) at x = 0.10–0.50; the repo note takes x = 0.65 and 1.00 from the printed NPG 16-43
rows (**MEASURED source values**). x = 0.24 linearly interpolates the 0.20 and 0.25 F rows
(**DERIVED**). The fit uses ±1,000 F as its shape budget. Record speeds
are **DERIVED** from F, not measured by this game. M79's own nose was not reached. The issue's
195 / 302 / 435 / 616 m/s estimates are **DERIVED low-speed limits** omitting target inertia;
the shipped inverse includes it. The gap is present with either proxy.

| x | Record F (BY EYE unless marked) | Record speed m/s (DERIVED) | Before m/s, 1-crh | After m/s, 1-crh | After F minus record F |
|---|---:|---:|---:|---:|---:|
| 0.10 | 23,800 | 74.04 | 195.847824 | 71.595268 | -785.3 |
| 0.20 | 33,000 | 145.18 | 278.604340 | 143.530457 | -375.1 |
| 0.24 | 35,880 (DERIVED interpolation) | 172.91 | 305.916504 | 172.476593 | -91.4 |
| 0.25 | 36,600 | 180.02 | 312.409058 | 179.732849 | -59.2 |
| 0.30 | 39,500 | 212.83 | 343.239685 | 216.149994 | +615.8 |
| 0.50 | 45,100 | 313.72 | 448.414429 | 309.177917 | -652.8 |
| 0.65 | 46,300 (MEASURED source) | 367.21 | 515.876221 | 366.625885 | -73.9 |
| 1.00 | 50,800 (MEASURED source) | 499.74 | 653.524536 | 653.524536 | +15,632.8 |

Before and after values are **MEASURED** by the ignored inverse-law instrument; F differences
are **DERIVED**. The required unchanged one-calibre
boundary cannot also match its firing row: that residual belongs to the bulk law. The hemisphere
proxy gives **71.635674 / 173.039932 / 312.399109 / 682.900269 m/s (MEASURED)** after the fit
at x = 0.10 / 0.24 / 0.50 / 1.00. Across the seven genuinely sub-calibre rows, the largest F
residual is **821.2 (DERIVED)** with that proxy; both proxies remain in the by-eye band. This
comparison does not establish M79's nose, deformation or a cross-material opening law.

**Where the player sees it (MEASURED normal-reference-plate limits).** A 7.9 mm AP round at
**475 m/s** now perforates **7.11 mm**: its limit falls from **496.706665** to **454.956970 m/s**.
A spent 88 mm AP at **400 m/s** now perforates **44 mm**, whose limit falls from **454.887787**
to **313.509369 m/s**; at **100 m/s** it now perforates **8.8 mm**, whose limit falls from
**198.544815** to **72.571091 m/s**. These are labelled illustrations from shipped round data,
not pass conditions. Fast rounds through already-defeated thin plate retain more speed.

**Recorded consequences (MEASURED).** Original aims and scenario identifiers stay. Nine records
regenerate; the other seven remain unchanged. Counts below are skids / armour crossings / spall
fields, excluding a burst field. Only s16 changes verdict: less work in the wheel and near side
lets the armed round enter the compartment and burst in flight rather than stop and burst inside
the near plate. Its nose piece stops in the opposite side; its stable identifier keeps demo links
resolving and its caption describes the new shot.

| Changed record | Before counts | After counts | Verdict and visible change |
|---|---:|---:|---|
| s1_through_crew | 0 / 2 / 1 | 0 / 2 / 1 | Penetration; rear-plate rest and timing change, pool losses unchanged |
| s4_spaced_then_stopped | 0 / 5 / 4 | 0 / 5 / 4 | NonPen; still stops in the near side after the wheel |
| s5_oblique_through | 1 / 3 / 1 | 0 / 1 / 1 | Penetration; later ammunition-box skid disappears, its HP pool still empties |
| s6_overpen | 0 / 2 / 2 | 0 / 2 / 2 | Penetration; retained speed changes, no pool dose |
| s7_engine_shadow | 0 / 1 / 1 | 0 / 1 / 1 | Penetration; engine dose unchanged, weaker field behind it |
| s10_gun_barrel_struck | 0 / 0 / 2 | 0 / 0 / 2 | Damaged; barrel reduced instead of knocked out |
| s13_ring_through_crew | 0 / 2 / 1 | 0 / 2 / 1 | Penetration; nose's rear-plate rest and timing change |
| s15_unarmed_overpen | 0 / 2 / 2 | 0 / 2 / 2 | Penetration; stays unarmed, retained speed changes |
| s16_restart_at_rest | 0 / 3 / 2 | 0 / 4 / 4 | NonPen → Penetration; burst at 98.42526 m/s inside the compartment |

MEASURED pool-unit losses (1,000 pool units per HP): s7's two ammunition pools take
**11,857 / 9,523**, formerly **13,584 / 10,910**; the engine still takes **336,000**. The s10
barrel takes **83,015**, formerly **250,000**. The s16 burst now reaches ammunition, station and
module pools instead of having its field stopped by the near plate; the complete receiver list
lives in its regenerated record. The s5 ammunition pool still takes **72,000**. No other scenario's
pool-unit loss changes. Four field-shot outputs change (`driver`, `rear`, `left-mid`,
`turret-left`); all inputs and the other **2,005** records stay unchanged. The **2,009** record
pin is **0x15eb25370bef2b50 (MEASURED)**.

The five-field moving-burst record reaches the pen cam atlas budget. Its generic **Resolution**
capacity is now **eight fields**, bounding atlas allocation and shader work while allowing
multi-exit bursts; Rust/WGSL counts move together. This is a view resource budget, not a physical
pass condition. The body-drawing proof supplies a stationary-burst variant at the terminal path
node instead of requiring a scenario verdict to supply one. No camera-plane solver is changed.
The shader guard retains every captured algorithm fingerprint while normalizing only the checked
field count, tail storage extent and neutral search-array padding. Count agreement is checked
separately; shading-bound and neutral-value mutants still trip the fingerprints. No shader math,
colour or band law changes and no new fingerprint is claimed to come from a capture. No captures
were taken; any visual review belongs to the owner's batched capture session.

The asset-specific burst snapshot is deleted; generic fuze timing, field-origin and receiver tests
own those contracts. Armed fixtures derive thresholds from their own plate's charged work. The
rest-delay proof reads each construction's front work instead of carrying a speed from a different
backing. Contact delivery distinguishes real endpoints from charged readout chords. The executed
bulk mutant changes the unchanged neutral arm to g = 1.01 at x >= 1 and fails its equality check;
restoring it passes. Firing rows remain outside pass conditions. No backlog row names an edited
file. `PROTOCOL_REV` is **81**, deliberately separating clients deriving the presented shell march
from this resolved-work meaning; wire layouts do not change. No switch, per-round field or
vehicle-specific rule is added.

Final verification (**MEASURED**): formatting and CI workspace/all-target clippy with
`dev_ui,bitprobe` pass. The final targeted batch passes **654 tests**, including all **44**
whole-tree guards; **12** measurement/pricing tools are ignored. The final whole library passes
**2,255**, fails **zero**, with **30** ignored tools/beds. The earlier whole-library attempts
exposed the view resource bound and scenario-dependent rest coverage; the calibre review exposed
the rejected return curve. All are resolved in the final run. The source guard's captured
fingerprints are unchanged; its resource-only normalization and shading-change mutants pass.
All binaries used unattended, muted headless settings under one machine lock, with compilation
through `serial-cargo`; final related gates were grouped under one lock without nesting locks.
No capture, pricing run, web search, issue comment or push was taken. The closing validation
extended beyond the requested **60-minute** wall-clock budget; no further lane item was started.

## 4. Boundary interaction — contact and the straight bite

Subject: the game's damage model; game state only. The accepted proxy and its evidence live in
[the penetration proxy note](../../scratch/research/penetration-proxy-2026-11-02.md).

At the first crossing, read the covered-sample steel-equivalent thickness along its normal `t_n`,
the entrance factor over the reference `f`, contact calibre `d`, speed `v` and type density `ρ`.
A round skids iff `ρ v² cos²θ < R_eff sin θ`, where `R_eff = min(f R, 4τ x sqrt(x/s))`,
`x = t_n/d` and `s = R/(4τ)`.
`R` is the march reference steel's cavity strength, `τ = Y/√3` its shear flow stress. The exact
constants 4 and √3 are plug shear and von Mises respectively. The axis-normal dot gives cos θ;
`sqrt(1 − cos²θ)` gives sin θ without runtime trigonometry in the decision. The thin branch is a
StandIn for plate capacity below one calibre, named in the proxy note; it joins the reference bulk
resistance continuously at `x = s`.

A skid retains the shipped mirror direction and speed `v sin θ`, blended by coverage; it does
not enter or throw spall. Work and impulse both read the current flying assembly. Directional
slide and finite-disc consequences belong to the footprint rebuild (#260).

Every bite goes straight. Resistance reads the real LOS chord times the slope charge (§4a)
and plate-capacity work share (§3b);
geometry and flight time retain the real chord. Reference-mm, remaining potency and energy-lost
dose all read that one charged state. An exhausted round rests inside; a perforation exits and
can throw a spall cone.

### 4a. The contact and slope proxies

Subject: the game's damage model; game state only. The required per-type `contact_density` and
`slope_penalty_deg` in `assets/ballistics/round_types.ron` stand in for deformation and shatter;
the penalty is converted once at load to tan β. No per-round construction field or separate cap
term is used. Capped and uncapped steel share density 6000 kg/m³: every surveyed firing row is a
bite and bounds density only from below. A capped/uncapped pair at t/d ≥ 1.6, θ ≥ 40° and speed
≥ 600 m/s would settle a capped premium. APCR's 16.5° penalty restates the survey ratios in this
charge's form, with no new firing rows; the proxy note owns the basis.

The resistance charge is `cos θ / (cos θ − sin θ tan β)`, exactly one head-on. At a denominator
at or below `cos θ / CEILING`, it is `CEILING`: Resolution 5 keeps it finite as θ + β approaches
90 degrees. A softer entrance pays `1 + min(1, f) (charge − 1)`.

Overmatch is the contact law's thin-plate result, not a verdict or a slope exemption: thinning a
plate raises its skid limit toward grazing. For the reference face, past `t_n/d = R/(4τ)`,
thickness stops moving the skid decision. Density and speed raise the limit; material strength lowers it. Replica scaling
preserves it. The tests compare this decision against `sin θc = sqrt(1+x²) − x`,
`x = R_eff/(2ρv²)`. The lane's evidence record compares the proxy note's labelled illustrations;
firing rows stay in the research record, outside the tests.

## 5. Spall — the spall cone

Spall is a **field**: strength over directions from an origin, stopped by the hard bodies it meets,
dosing the boxes it reaches (glossary: *Field*, *Spall cone*). Adopted by the owner 2026-10-12;
decision record `.agents/scratch/damage-cam-2026-10-08/product/in-battle/owner-direction-2026-10-10.md`
("The dose model"), evidence in `research/cloud-falloff.md`, `research/cloud-angle-deep.md` and
`research/codex-cloud-angle/verdict.md` beside it.

**Which steps throw.** A cone is thrown at every **body exit** the round reaches, sized by the
energy the round lost across that body alone (**Strength**), whatever the round does afterwards. A body is a maximal stretch of
hard material along the path (a witness, §6, is no material of the walk) (welded voids inside it included) over which no layer resists less than
the one before it. It exits where the next medium resists less — a lower factor, or air — and the
next body starts there. A step to an equal or higher factor continues the body without a throw,
whatever the substances: plates of equal factor in contact are one plate and throw once at the far
face with the energy lost across both (owner, 2026-10-12), and a step up into harder material adds what is
crossed behind it to the same body, since no softer step intervened. Exits reached before the round
stops still throw; the body the round stops inside throws nothing. A bounce throws nothing.

**Accumulate before rounding.** Every nonzero field is walked, recorded and added to each
receiver's shot tally at its mechanism's unit. The pool rounds the shot's accumulated total;
no individual field is filtered at the pool quantum. DERIVED fixture counterexample: two fields
of 0.0003 HP each sum to 0.0006 HP, which rounds to one thousandth of an HP point, although
rounding either field alone gives zero. Only a field carrying no energy is not thrown.

**Strength.** The game's damage model emits `H_C = u_C × ΔE × r` HP points from a
cone. `u_C` is `cone_emitted_hp_per_joule` in `assets/ballistics/spall_cone.ron`, a provisional
`Unit` of emitted HP per joule lost at the plate, DERIVED as `0.88 × ½ / 3000 =
0.00014666666666666666`. `ΔE` is the kinetic energy the round lost crossing the body,
`½ m (v_entry² − v_exit²)`, with `m` the mass meeting the plate (a sub-calibre round's core, §3).
Each stretch's energy drop is shared among the disc samples as its spend is and summed over
samples reaching the exit. `r` is the exit material's strength ratio, its factor over the
reference steel's factor, capped at unity (`walk::exit_strength_ratio`). More energy lost across
one substance gives more emitted HP; no energy lost gives none. A softer exit reduces that
emission by its material ratio.

**The energy partition.** The energy the round lost across a body is spent once: the cone
carries `E_C = κ r ΔE` of it out, and the line deals the body's volumes the rest,
`(1 − κ r) ΔE`, sample by sample over the stretches inside the reached body
(`walk::SpallSource::field_share`); a body the round stops inside keeps all it lost. `κ` is
`cone_debris_energy_share` in `assets/ballistics/spall_cone.ron`, a `StandIn` 0.06: the share of
the energy lost that the debris carries out through an exit of the reference steel's strength. Its
order of magnitude is DERIVED from a plug of the reference steel (7850 kg/m³) the 88 mm reference
round's disc wide and its 100 mm plate thick, 4.774 kg, leaving at the round's 259.69 m/s exit
speed: 161.0 kJ of the 2.7035 MJ lost, 0.0596. Measured behind-armour debris (fragment masses and
speeds against the energy the plate absorbed), together with construction and material inputs,
are missing from the current debris-production law. The global share and `r`'s emission role
stand in for that missing law; resistance is not a debris-yield fact. The cone's runtime unit is
`u_C / κ` HP per carried joule, preserving `H_C = u_C r ΔE` as `κ` changes. This is HP calibration,
not evidence for the physical partition. The walk accumulates retained and outgoing work directly,
without subtracting separately rounded debris from a summed loss. Each source's outgoing joules
are read from its addressed exchanges, rather than recomputed from a second loss ledger.

The unit is a balance scale, not a physical energy share (#258). It was set against the retired
reference cone's target: an 88 mm APCBC round (10.2 kg, nose factor 7/24) at 773 m/s perforating
100 mm of reference steel at normal impact (ASSUMED reference geometry). DERIVED from the force
law: its capability is 114.93 reference-mm and its exit speed 259.69 m/s; the retired strength
`100 × (259.69 / 773)² = 11.286` reference-mm emitted 395.0 HP at its field conversion. The
round loses 2.7035 MJ. Matching the target gave the former transport coefficient about 0.877,
rounded to 0.88; including the field weights' half-strength normalization and the transport
conversion gives `u_C`. The reference fixture still emits 396.513336 HP, or 396.5 to one decimal
(MEASURED by the ignored bed `spall_cone::tests::the_shipped_emission_totals`). The retired
hole-calibre and speed-ratio terms have no part in this law: a thin plate yields a weak cone,
and a barely perforating round yields the strongest cone that plate gives.

The walker and records carry the **energy the field carries**: strength `S = E_C / q`,
`q = EMITTED_SHARE = ½` the exact normalization of the field weights, so the field emits
`qS = E_C = κ r ΔE` joules. No HP unit enters `S`. A building reads a direction's `S f_k / N`
joules as crushing energy; a box's pool reads the same joules times `u_C / κ`
(`spall_cone::FieldUnit`, `spall_cone::cone_unit`, `ballistics::field_units`), so
`H_C = (u_C / κ) qS`. The three mechanisms' units and their replacement law are stated in §6.

**Preserved scenario record and open exit-material call** (#116, #182). Subject: the game's
damage model, HP pools at stations. The earlier energy-law record reported these as MEASURED:
s1's transport strength was 2.40 MJ, emitting 400 HP against the retired law's 362 HP; s12's
cone was 0.19 of s1's, against 0.62 under the retired hole-calibre term. These are retained
scenario observations, not newly measured results or a current asset gate. The softer-exit
ratios were DERIVED from the then-authored material factors: cast armour 0.9, mild steel 0.8,
ammunition 0.2, engine block 0.15 and rubber 0.05. Applying those ratios to their exits remains
an open owner call under #182; declaring the emission unit did not settle it.

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
field over the whole sphere, is the same law's closed form, which no shipped field reaches (a plate
exit's `λ` is at least 1, and every burst is a ring, §5a). The on-axis weight is half the centre weight `(1 + 3λ²)(1 − cos α)`: through RHA (26.2°) the axis
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

**Shadowing and reach.** A direction's strength is constant in air — no drag, no range —
and ownership does not restrict its receivers: neighbouring tanks and environment matter take
part in the same walk. Full shadowing means the first hard body entered stops the direction:
armour takes no HP dose, a hard box takes its whole weighted strength, and either shields what
lies behind. A direction still leaving the emitting body has not entered it; after departure,
genuine re-entry transfers the returning debris energy to that body, including its far wall.
The retained line work and returning debris are separate shares, so this does not spend the
same energy twice. A direction entering no hard body ends at the field's candidate horizon.
A witness (§6) never stops a direction: each one entered before the stop takes its strength,
so crew stations never shield other crew stations. The `1/r²` fall-off is geometric: a farther
box covers fewer directions.

**Walk.** A direction is walked like the line's corridors: the collector and the pairing walk along
the ray through spatially reached volumes, from outside every candidate's bounds through the throw
point, with nothing declared. So what it starts inside is established, never assumed, and every
volume along it is present in order, nested and overlapping ones included. No nudge displaces the
ray: the volumes the exit leaves are recognised by identity, so a direction starting inside one of
them has not entered it and is neither stopped nor dosed by it there. A direction that starts inside
any other volume has entered it at the throw point.

**Dose.** A box's dose from one cone is `D_b = (1/N) Σ s_i` over the directions that dose it — a
hard box those stopped in it, a witness every one entering it short of its stop — each direction
dosing a box once, in the field's joules, never scaled by the box's own factor; armour (no count) takes
nothing. Doses go through the addressed `Dose` effect, like the line's. A box's dose from the whole
shot (the line, a bounce's shock and every cone) converts to pool units once, on its total (§6).
Every field deposit, to a hard box and a witness alike, is a share of the cone's emitted HP: it
enters the tally as its joules times the cone's unit (§6), the one energy-to-HP scale a field has;
`joules_per_hp` never touches it. Every contribution reaches the shot total before pool rounding.
A box of face `A` fully inside the cone, `θ` off its axis, has the knockout radius
`R = sqrt(2 f(θ) E A / (pool Ω_c))`, `E = u_C r ΔE` in HP points, `Ω_c = 2π(1 − cos α)` and `f(θ)` the
ball's weight there (½ averaged over the cone). A bigger box is a bigger target: it takes more of
the cone's directions, and nothing scales its pool to match (§6). On the reference cone through
RHA (`E` = 396.5 HP points, MEASURED fixture), for the 30 HP crew pool at a seated crew member's 0.2 m² mean face,
it is about 2.0 m at an average spot and 2.6 m on the axis (DERIVED from the preceding formula); the seed
tank's crew box, 0.4 × 0.3 × 0.7 m and MEASURED at a 0.2033 m² mean face, moves each radius out by
under 1 % (DERIVED: `sqrt(0.2033 / 0.2)` = 1.008). Dose summed over boxes is not conserved (a direction doses every witness it enters short of its stop as
well as the hard box that stops it); the emitted budget is.

**Potency colour.** The pen cam colours a field by its potency, never by a display gain (decision
record `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/decision-2026-10-23.md`,
#8). Each of the `N` directions stands for `Ω / N` of the field's support (`Ω` its solid angle: a
cap's, a ring's band's or the sphere's) and deposits `s / N`, so a direction of strength `s` emits
`u s / Ω` HP points per steradian (`u` the field's mechanism's unit, `spall_cone::FieldUnit`: the
HP points a joule of the record's strength converts to, the sim's own rate, `u_C / κ` for a cone
and `u_B` for a burst field), and at distance `r` along it, on
the support and short of its reach, the **local intensity** is `I = u s / (Ω r²)` HP points per m²,
zero past the reach; a thin unshadowed box of face `A` there takes `I · A` (resolver test
`the_pen_cam_intensity_times_a_face_is_its_dose`). Doses add, so where fields overlap the bands
classify the sum of their intensities. Three **potency bands**, parted at thresholds of the
struck tank's own crew boxes (`BoxClasses::potency_thresholds`): **knockout** at `I ≥ T_mid`,
`T_mid = crew_hp / A_crew`, `crew_hp` the shared crew pool (`assets/ballistics/box_classes.ron`)
and `A_crew` the mean face of the struck tank's crew boxes at rest in its sim geometry, the mean of
the three axis-aligned face areas of each box's bounds (`TankShapes::crew_face`; the seed tank's
0.2033 m², MEASURED, so 30 / 0.2033 = 147.5, DERIVED); **reduced** from `T_floor` up to it;
**nothing** below `T_floor`, the display's whole point over the same face: the least dose a full
pool shows a lost point at (§6, **Shown health**), half a pool unit short of a point
(`ballistics::least_shown_hp`, 0.9995; 0.9995 / 0.2033 = 4.92, DERIVED). The face is read from
the geometry and authored nowhere else, so the bands follow the boxes they describe. The game's
damage model authors one shared crew box size within a tank, so the thresholds describe each
station alike; a plan with no crew box has no bands and draws no field. The promise, for a full
pool: a crew station whose face lies wholly in knockout is knocked out; one wholly in reduced
shows a lost point and keeps its pool, short of the last half pool unit below `T_mid`, which
converts to the whole pool; one wholly in nothing shows no lost point, though its pool keeps the
dose. A box only partly inside a field takes that intensity over that part
alone. Knockout takes the field teal and reduced the deep teal (`timeline::FIELD`, `FIELD_DIM`;
relative luminance 0.50 and 0.12, DERIVED, so they part in every colour vision type), nothing no
colour; neither is the state scale's red. Along a direction the intensity reaches mid at
`sqrt(u s / (Ω T_mid))` and the floor `sqrt(T_mid / T_floor)` ≈ 5.5 times farther (DERIVED); s1's
strongest direction (1,038 HP points per steradian) reaches mid at 2.65 m and the floor at 14.5 m,
past every reach of the shipped records (5.2 m at most); the weakest field, the road wheel's
rubber cone in s4 and s16 (11.5), reaches its floor at 1.5 m, past its 0.79 m reach (DERIVED from
the records). So the reduced band ends at a reach or a shadow of the struck tank's hard bodies,
rarely at the floor, and its edges are theirs.

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
2160). s14's atlas, then the largest shipped (the burst's ball, a record retired with it, as are
the s14 frames measured below), was 1618×3872 texels at a 1920×1080 target and 3335×7976 at
3840×2160 (MEASURED, the expanded corner at a scale factor of 2). A change of the details asked
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

**Data.** The axis rule, `N` and the cone's emitted-HP-per-joule unit live in
`assets/ballistics/spall_cone.ron`, parsed and validated while the app is built (ADR-0014: built from data, synchronously). A cone's ratio is
its exit layer's substance's: every hard substance authors `debris_half_angle_deg`
(`assets/materials/materials.ron`, `λ = 1/sin α`), refused at load outside `(0°, 90°]`. The exit
layer is the body's last before the exit; where volumes overlap in it, the cone takes the one of
highest factor, the larger ratio between equals. RHA's 26.2° is the constant-angle
fit of the MEASURED Qi et al. 2026 normal-impact debris extents (shots 5 and 6, `shape-law.md`
§2.1–2.2); those are 97 % contours, so the ball's rim may sit up to about 2° wider. Every other hard
substance authors RHA's value as Unknown until its own extent is measured. A spread whose rim's
`cos α` rounds to 1, or which does not emit `S/2` within the rounding of its weights, is refused,
and a throw whose derived ratio the law refuses is an invariant failure before any direction is
walked. The axis is playtest fork F5 (its
half-angle settled by this; `.agents/scratch/playtest-forks/README.md`).

**Class invariants** (tests hold them for any tank and any cone; fixture numbers are fixture data).
Exact emission `S/2` at every N from 1 to 1024 and every ratio from 1 to 10⁴; under the ball at a
fixture ratio: resolution stability; homogeneity in `S`; inverse square for an unshadowed thin box;
factor independence of a box's own dose; shadowing monotone (adding a volume never raises a dose).
A cone emits its declared HP-per-joule unit times the energy lost across the body, weighted by
exit material, and of two rounds
through one plate the one that loses more energy throws the stronger cone, whatever its mass,
calibre or speed; every nonzero cone contributes; a shot's dose to a box is converted
to pool units once on its total. A direction stops at the first volume
it enters, a box there taking its whole strength, armour nothing and shielding what lies behind;
a plate thinner than any nudge stops it; it is neither stopped nor dosed by a volume its throw
is still leaving; after departure, re-entry can dose it. Any other volume holding the throw point
is entered there. The ball's weights are its
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
flight direction. That field is the **ring**, for every burst round: playtest fork F7 compared it
with the **ball** as two rounds on the main gun's selector and closed for the ring (2026-10-27),
and the burst's ball retired from the data and the code (#115). Brief: `.agents/scratch/damage-cam-2026-10-08/research/ap-aphe-2026-10-23/brief-4b-aphe-sim.md`,
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

**The parts and the ring.** A round's parts are DERIVED at load from its body (mass `m`, calibre
`d`, its body steel's density from `materials.ron`) and its burst block (core mass `C`, its core
substance's density `ρ_c` and speed constant `K`, the `burst` facts of the `materials.ron` row the
block names, the base parts' mass `B`, the cavity's diameter `d_c`): the **cavity** is a cylinder
of the core's volume `C / ρ_c` and diameter `d_c` just forward of the base parts; the **wall** is the steel annulus around it over its length, outer diameter `d`;
the **nose** is the rest, `m − wall − B − C`. A burst gives a driven mass `M` the greatest sideways
speed `v_s = K (M / C + ½)^−½` (`shape-law.md` §4), DERIVED per round and never authored, and the
field's ratio is `λ = v_c / v_s`, `v_c` the carried speed.

- **Ring.** The wall is driven. The field is uniform per steradian over the band `θ_c ∓ h` of polar
  angle from the flight axis, `θ_c = atan(1/λ)` (90° at rest: a flat band), its near edge held at
  the axis where `h > θ_c`; `h` is the band's half-width about its centre (`ring_half_width_deg`).
  The nose piece flies on from the burst point at the carried speed as the shell's round, inert:
  its mass the derived nose's, its calibre and nose factor the round's, its drag re-derived, its
  crew line rate the round's. It meets the world as any round's penetrator does (§3: the disc,
  contact, the force law, its own line and its own spall cones). A ring that bursts at rest
  throws its band and nothing flies on.
- **Retired: the ball.** The round less its core driven as one lobe under the ball law (§5,
  **Profile**), the whole sphere below `λ = 1`, the round ending at the burst: F7's other shape.
  A round's burst block names no shape, and the field law keeps the ball for the plate exit's
  cone alone (§5).

Every plate exit reached before the burst throws its spall cone as a solid shot's does, and nothing
of the round goes on past the burst but the nose piece. The shipped 8.8 cm Pzgr. 39 body and
burst data are the fixture here; the construction law is generic: `assets/rounds/88mm_APHE_ring.round.ron` supplies mass 10.2 kg and calibre 88 mm
(MEASURED in its provenance), core mass 0.113 kg and cavity diameter 35.3 mm (DERIVED there),
and base mass 0.33 kg (Unknown, a declared proxy). Its material rows supply body density
7800 kg/m³ (MEASURED), core density 1650 kg/m³ and speed constant 2680 m/s (DERIVED).
The load-time construction law gives, DERIVED from those inputs:

| part | value |
|---|---|
| cavity | 68.48 cm³, 69.98 mm long |
| wall | 2.786 kg |
| nose (no source gives it: provisional) | 6.971 kg |
| ring: `v_s`, `½ M v_s²` | 534.4 m/s, 397.7 kJ |

**The field.** The burst emits `H_B = u_B × E_wall` HP points, `E_wall = ½ M v_s²` the
kinetic energy driven into its wall (`Burst::energy`, DERIVED at load). `u_B` is
`burst.emitted_hp_per_joule` in `assets/ballistics/spall_cone.ron`, a provisional `Unit` of
0.035 emitted HP per joule driven. Neither energy lost reaching the burst nor the cone's unit
enters it. Its strength is `S = E_wall / q`, as for the cone the energy it carries over the
emitted share; the ring's equal-solid-angle strata emit `qS = E_wall` at every direction count.
They march, shadow and dose as a cone's do (§5), through spatially reached volumes, leaving none,
every contribution reaching the shot tally; a deposit's joules times `u_B` are its HP, and a
building reads the joules themselves. The record and wire carry the field descriptor and that strength. The data
block also holds the ring's half-width and direction count, independently of potency.

**Potency.** The ring's unit was set against the retired even-sphere target: a crew station's
0.2 m² face, fully inside the field at 2.0 m, has its 30 HP pool emptied. DERIVED: a uniform
sphere needs `30 × 4π × 2² / 0.2 = 7540` emitted HP. The previous reference-mm field-strength
scale, 1.0 reference-mm per kJ at 70 HP per reference-mm, was 0.07 HP per joule of strength;
the field weights emit half, giving the current 0.035 emitted HP per source joule. Equivalently,
the former transport coefficient was 210, so `210 × ½ / 3000 = 0.035`. This is a game unit,
not a claim that the wall emits more physical energy than it carries.

That target calibrated the **retired ball**, which drove 214.8 kJ (DERIVED in the retained
construction record). The unit then stayed fixed when the ring drove the current construction's
397.7 kJ. DERIVED: its 13,920.84 emitted HP is 1.85 times the even-sphere target, an even-sphere
crew knockout radius of `sqrt(13920.84 × 0.2 / (4π × 30)) = 2.72 m`. Keeping the unit preserves
the chosen ring's game potency; it does not claim the ring was re-tuned to the old sphere target.

MEASURED by the ignored bed `spall_cone::tests::the_shipped_emission_totals`, the shipped ring
emits 13,920.837891 HP (13,920.837333 HP in the earlier transport representation). The
previously quoted 13,922 HP is an approximate derived figure, not the measured total;
preserving behaviour takes precedence over tuning to that quotation. A thin unshadowed face `A` wholly inside the band has
knockout radius `R = sqrt(H_B A b / (4π pool))`, `b = 4π p_Ω`, its brightness relative to a
uniform sphere. DERIVED from the shipped construction above, with the 30 HP crew pool and
0.2 m² face (a module follows from its own authored pool and face):

| field | where | `b` | crew (0.2 m²) |
|---|---|---|---|
| ring at rest | in the band, 80–100° | 5.76 | 6.52 m |
| ring at 244.6 m/s | `λ` 0.458, band 55.4–75.4° | 6.33 | 6.84 m |

The retained s13 measurement from the earlier field-unit conversion compared the ring alone:
346.2932 versus 346.29324 HP deposited into `Ammo_R_0` (MEASURED in that scenario record,
within f32 rounding). It is historical evidence for that conversion, not an asset-specific
pass condition for the current law. The energy-law provenance is #116; the exit-material
weighting provenance is #182.

The carried speed in the second row is the recorded s1 fixture speed (MEASURED); the ratio,
band and radius are DERIVED. The seed crew box's 0.2033 m² mean face (MEASURED from geometry)
extends those radii by a factor `sqrt(0.2033 / 0.2) = 1.008` (DERIVED).

**The record.** The shot record carries the burst (`damage_cam::Burst`): when (the path reaching the
burst point, its last leg timed at the speed the burst trigger counts it at, so a burst in flight
lies the delay after the exit that last started it; for a round at rest, the delay's remainder after
it stopped there), the burst point, the carried speed, the nose piece's mass, and the field as a
cone record with every addressed contribution (absent only when the field carries no energy).
The path runs on with the nose piece's line
to its own end, its open air timed at the carried speed. A struck tank's record stays open while a
fuze that tank armed runs, so a burst past its last body lands in it. The wire round-trips the
burst; its 256 directions add 0.3–1.3 KB to a message (MEASURED, `src/damage_cam/wire.rs`). The pen
cam draws the burst field as it draws a plate cone (`src/damage_cam/pen_cam/picture.rs`): thrown
when the shell's nose reaches the burst point on the corner's clock, its front running out each
direction's reach, over the band alone. It reads as a band about the line with nothing drawn inside
its near edge, and the line runs on with the nose piece. A box the burst doses is reached when the
first field that dosed it, a plate cone's or the burst's, reaches it.

**Scenarios** (`assets/dev/pen_cam/`, fixture data; each record's `shows` says what it shows).
s13 flies s1's line with the burst round: the upper front plate arms the fuze, and the round bursts
0.37 m past its exit at 244.6 m/s, short of the driver's station. The band's dose empties the HP
pools of the bow gunner, the transmission and two ammunition racks, and the nose piece flies on
through the driver's station to the rear plate. The plate's cone knocks out the driver, as in s1,
which empties no other HP pool and leaves the gunner and the commander reduced. s15 flies s6's
line with the burst round: no body holds the arming spend, so its record is s6's. s16 flies s4's
line with the burst round: the road wheel arms the fuze, its rubber and far disc restart the delay,
and the round stops 21.5 mm into the lower hull side and bursts at rest there, a flat band with no
nose piece to fly on; every direction starts inside that plate, which stops it, so the field doses
nothing (MEASURED, from the records).

**Class invariants** (tests hold them for any round and tank; fixture numbers are fixture data).
The fuze arms only at the threshold; the delay runs from the exit and restarts at a later body or
a bounce; the burst lies the carried speed times the delay past the exit; a round at rest bursts
where it rests after the remainder; a round no body arms, or without a burst block, flies as a
solid shot. The ring's weights emit `S/2` at every `N` the data allows, `λ < 1` included; its
directions lie in its band, centred `atan(1/λ)` off the axis, so the field leans forward with the
carried speed. The nose piece flies on at the derived mass from a burst in flight, and nothing else
does. The parts sum to the round, and a burst that leaves no nose is refused. The record and the
wire round-trip a burst.


## 6. Component damage — HP per component, never per tank

Every component has its own **HP pool** in whole **HP points**. A crew station's is the one shared
crew pool (30 HP, Unit, provisional, `assets/ballistics/box_classes.ron`), which a tank file
cannot author: a crewman is one entity across tanks, and the pool has no relation to the size of
the crew box. Every other pool is a plain integer authored by hand in the tank data (`hp` in
`<tank>.tank.ron`), tuned in play; no class rate or size rule derives it (owner, 2026-10-28, #104;
a pool derived from class × volume is deferred to tank #2, if hand-authoring proves inconsistent).
The field needs no such rule: a bigger box is a bigger target, crossed by more of a field's
directions, so it takes more dose (§5). A crew box is person-sized, one shared size within a tank
(the seed tank's 0.4 × 0.3 × 0.7 m, a 0.2033 m² mean face, MEASURED from its sim geometry), close to
the 0.2 m² face the fields' knockout radii are quoted at (§5), and the pen cam's potency bands are
read from it (§5, "Potency colour"). A spall cone doses it by the strength reaching it (§5); the
**main penetrator transiting** a hard module deposits its retained work after outgoing debris;
a carrier's first resisting contact also retains the carrier's kinetic energy. A bounce
deposits the energy the round lost there, the motion pointed into the face, into the face it
struck.

**The energy dose** (issue 113, decided 2026-10-28, provisional; owner direction: keep it simple).
A hard box's line dose is retained work. The core loses `½ m_c (v_in² − v_out²)` over a
crossing; reached exits carry their assigned `κ r` shares, and the crossed volumes retain the
rest. A core that stops inside a body emits no debris through that body's unreached exit.
At first resisting contact a kept carrier adds `½ (m_f − m_c) v_in²` retained joules there;
later crossings contain only the core. Consequently, retained work plus emitted debris plus
the core's outgoing kinetic energy equals the incoming assembly's kinetic energy.

Each stretch's work and axial momentum go to its strongest exposed volumes, with tied maxima
sharing the sample's contribution equally (§13.2). Each line reaches its own body once;
entrance ownership is fuze attribution, not an impulse destination. The core transfer is
`m_c (v_in − v_out) i` for the straight bite direction `i`; a stopped carrier
adds `(m_f − m_c) v_in i` at entry. Buildings distribute axial transfer along the line, but keep
the carrier impulse at the entry chunk. Trunks likewise keep entrance impulse at entry.
The emitter receives the whole resolved transfer, including debris launch: the field carries
work without a sourced free body's mass or velocity and gives receivers no second blow.
This closure remains provisional until debris production supplies those facts.

A seam between touching volumes of one substance does not change the force-law advance; each
volume takes the work and speed loss at its own depths, never a length share of the whole
crossing. Unowned-matter bounce delivery to construction consumers remains tracked in the lens backlog.

**One line/bounce unit** turns retained line work or bounce loss into HP for a hard pool's line dose or
bounce shock, a tank's modules and a tree's trunk alike: `joules_per_hp` in
`assets/ballistics/energy_dose.ron`, Unit 3000 J per point. Cone and ring deposits use their own
emitted-HP units below; this line/bounce scale does not define their game potency. It keeps the scale
the pools were authored at for the main gun: the reference-mm law charged 10 HP per reference-mm,
and the reference 88 mm APCBC round at its 773 m/s muzzle speed loses about 3090 J per 0.1 mm of
reference steel (DERIVED from the force law, §3). Each pool's authored HP alone says how sturdy it
is; a slower or smaller round deals less per reference-mm crossed. Reference-mm stays a readout —
of a round's capability and of a chord, the steel it is worth — and is no hard box's dose.

The law it replaced charged chord × factor, the thickness defeated, whatever the round: an 88 and a
rifle round dealt the same HP through the same chord. MEASURED 2026-10-27
(`.agents/scratch/destruction-2026-10-26/shot-dose-measurement.md`): a perforating 88 dealt 155 HP
through a 0.40 m spruce at its base, 8 shots to empty a 1200 HP pool, while a machine-gun round
that came to rest dealt its whole capability, so the belt felled a tree faster than the main gun.
Under the energy dose (MEASURED 2026-10-28,
`.agents/scratch/damage-model-2026-10-28/energy-dose-results.md`) the 88 deals 156 HP there
(466 710 J) and the machine-gun round 1 HP (2 872 J, all it had): 2 shots against 240 to empty the
shipped fir.

**Three energy mechanisms, three declared units** (#258, provisional owner decision).
Each mechanism has one balance knob whose name says what it converts:

| mechanism | authored unit | dose or emitted HP |
|---|---|---|
| hard-pool line and bounce shock | `energy_dose.ron::joules_per_hp`, Unit 3000 J/HP | retained line joules or bounce loss / unit |
| spall cone | `spall_cone.ron::cone_emitted_hp_per_joule`, Unit 0.00014666666666666666 HP/J | unit × lost joules × material strength ratio |
| burst ring | `spall_cone.ron::burst.emitted_hp_per_joule`, Unit 0.035 HP/J | unit × driven wall joules |

The field units are DERIVED from the preserved transport coefficients and normalization (§5,
§5a); the line unit's derivation is above. There are three because these are three mechanisms
with independent game potency, not three physical shares of one energy budget. Changing the
line's unit does not touch a cone or ring: a field carries joules, which a building crushes
with, and converts them to HP only through its own unit. A witness's line is a chord law with the round's crew line
rate, not a fourth energy mechanism.

The cone and ring units stand in for a missing **fragment-hit law**: fragments empty a crew
station's HP pool by hits, rather than by energy caught on a face. That law must derive fragment
production and hits from material and construction data and replace these two provisional
field units when it lands. It is recorded here, not implemented. Their `Unit` labels declare
where balance lives today without pretending the values are physical shares. Data notes point
here; this section owns the rationale. The line's energy unit remains its own declared scale.

A bounce's shock uses the line unit: the energy lost there is `½ m (v_in² − v_out²)`. The
shock this replaced was `0.045 × the round's readout × cos(incidence)` reference-mm at 10 HP
points each, at most 18 for the 88 (DERIVED).

**A bounce deals only the energy pointed into the face** (issue 145, decided 2026-10-28,
provisional). The striking velocity splits into its part along the struck face's normal and its
part along the face. At full coverage the shell retains speed magnitude `v sin θ`, `θ` the
incidence from the normal, along the existing exit direction (the reflection, blended by η,
§13.5). The energy it loses there, the shock, is therefore `½ m (v cos θ)²`, and a struck box
with a pool takes it at the one rate; armour takes none. This energy budget does not settle the
outgoing direction: the mirror and restitution-zero slide differ as recorded in §4a. Under partial coverage the
speed retained blends from all of it toward `sin θ` by η, as the direction does (§13.5), and the
shock is still the energy lost. This retired the fixed ricochet bleed, 0.6 of the speed kept
whatever the angle, under which the fixture 88 fired at 773 m/s struck a 0.4 m steel slab with a
pool at 771.25 m/s, left at 462.75 and jarred 1.9415 MJ, 647.2 HP points, into it at any graze from
72° to 85° (MEASURED), more than its penetration of a gun barrel (245 HP). MEASURED through the
march under this law (`resolve::tests::a_bounce_doses_the_energy_pointed_into_the_face`), the same
shot:

| incidence | leaves at (m/s) | shock (J) | HP points | DERIVED HP |
|---|---|---|---|---|
| 72° | 733.50 | 289 686 | 96.6 | 97 |
| 75° | 744.97 | 203 215 | 67.7 | 68 |
| 80° | 759.53 | 91 475 | 30.5 | 31 |
| 85° | 768.32 | 23 044 | 7.7 | 8 |

The most a bounce can deal off a given part is just past its pressure-contact limit (§4a),
`cos² θc` of the striking energy. It rises with thickness until thin-plate capacity reaches the bulk
resistance. A thin part's dose cannot bound a thicker one's; each reads its own material and
contact thickness. Armour bounces take no HP.

A witness's chord alone keeps its own unit, one HP point per 0.1 reference-mm
(`ballistics::HP_PER_REFERENCE_MM`, a unit with Resolution provenance, not a balance value), the
round's crew line rate its whole scale.

**Trees are just wood.** Every trunk is one substance, `Wood` (`materials.ron`), and every trunk
segment is a volume with a pool of its own (issue 149, decided 2026-10-29): the wood row's HP per
square metre times the segment's cross-section at mid-height, `π r²` for `r` the mean of its end
radii at the instance's scale, rounded, at least one point (`hp_per_m2` in
`assets/trees/wood.ron`, TUNED 1500). A dose charges the segments it crosses, each the energy lost
at its own depths; the segment whose pool empties is where the tree breaks, so only the hits at
that height count toward that break, and a tree breaks once. The pool grows with the square of
the diameter while a line's dose grows with its chord, so thin wood breaks in fewer shots: the
88 mm APCBC breaks the shipped fir's base (193 HP, 0.405 m) at the second centred shot, its
0.29 m section 10 m up and its 0.13 m top 20 m up at the first, and a 0.2 m young tree or a
0.1 m sapling at the first; the machine gun breaks the sapling in 34 centred hits and the fir's
base in 187 (MEASURED, `.agents/scratch/damage-model-2026-10-29/segment-pools-results.md`). An
emptied pool opens the joint at its segment's base in the trunk's structural state, the one a
body's contact breaks too (`wall-destruction.md`, "Trees are structures by the same laws"). A
species' structural facts (grain, green density, root plate) are its own; its HP rate is not, and
for the dose a species is the generator's look and ranges only: a player cannot tell that a round lost a little more speed
in one species than another, while trunk thickness is simple and visible. A round entering a trunk meets the wood's share of the
reference resistance, so its slope charge pays 4 % of the reference steel's excess (§13.5); a round that bites keeps its direction. Out of scope, built only if play
shows it missed: the drag form under the energy law (epic #103); the field's dose is under it (§5).

**Witnesses** (ADR-0040) are dosed by a law of their own. A witness substance has no resistance and
is no layer of the walk (§13), so the shell and the field cross it as air. Once the hard world has
resolved the shot, each witness box takes:

- **the line:** `chord × wound`, `chord` the disc-mean chord the shell's legs cut through it in
  the open, none of it inside a hard volume (§13.2; the walk's own k-sample disc, §13.5, each
  sample that misses counting zero; a leg ends at the rest point of a shell stopped inside it),
  `wound` the **round's** crew line rate (reference-mm per metre, the round file, TUNED);
- **the field:** each spall direction entering it, at its strength there × its share (§5).

Both go through the same `ShotDose` tally, units and once-per-box conversion as a hard box's, and
a mark's deposits all land before its one settle. Crew carry a witness substance and only crew do,
refused at bake otherwise.

**Pools keep fractions** (Yan, 2026-10-28, issue 142). A pool's size is whole HP points, as data
authors it; what it holds is a fixed-point integer of **pool units**, a thousandth of a point
(`ballistics::POOL_UNITS_PER_HP`). A box's dose from **one shot** — the line, a bounce's shock and
every cone, summed in exact integer dose units — converts to pool units once, to the nearest unit
(ties away from zero, integer arithmetic), never per direction and never per source, and is taken
whole: no fraction of a point is dropped, so doses below a point accumulate across shots and
eventually empty the pool. The pool stops at zero, and is emptied, the component knocked out,
exactly when what it holds reaches zero. It falls as the shot is resolved, each step taking what
the converted running total has gained, so it lands on the converted total when the shot ends.
Pools replicate, and records carry them, in pool units: deterministic on the wire, never rounded
there. The resolution keeps a single machine-gun hit's dose within 1 %: a dose loses at most half
a unit, 0.0005 HP, so every dose from 0.05 HP up keeps 1 % (DERIVED); on the shipped mature fir
(240 HP) the machine gun's hits run from 1.037 HP at the base to 0.483 HP centred 20 m up (kept
within 0.10 %) and 0.058 HP in a 0.9-radius graze 25 m up (within 0.86 %), and a hundredth of a
point would put the 0.483 HP hit at 1.04 % (MEASURED with the flight-nose drag). A u32 pool holds
4 294 967 points (`MAX_POOL_POINTS`); a tank or class file authoring more is refused.

Per-shot rounding to whole points took nothing from the 0.48 HP hit, so the belt never felled the
fir high on the trunk, while a hit at the base took a whole point: whether the machine gun worked
depended on where it hit. Kept whole, every hit counts where it lands. MEASURED hits to fell the
fir under its one 240 HP pool, before trunk segments had pools of their own
(`.agents/scratch/damage-model-2026-10-28/fractional-pools-measured.txt`): 232 at 1 m
centred (240 under the rounding), 268 at 10 m, 497 at 20 m centred and 553 at 20 m half a radius
off (never under the rounding); the 88mm_AP's cells keep their 2 to 7 hits.

**Shown health.** Only the display rounds: every view of a pool (the HUD's damage flash and hit
marker, the damage panel, the pen cam) shows whole points, rounded up (`ballistics::shown_points`).
A pool that has lost 0.8 HP shows full and is not painted damaged; it shows damage once a whole
point is gone, and shows none exactly when it is emptied. The pen cam's verdict and path follow
the display: a component is dosed (`PathStep::Dosed`) only where the shot took a shown point,
and a shot's hit confirmation counts shown points. Both count them settle by settle, each against
what the pool held at that mark (`ballistics::Settle::shown_lost`): another round dosing the pool
between two of the shot's marks then neither lends the shot a point it crossed nor hides one the
shot crossed, which what the pool held at the shot less the shot's whole take can do (a 3-point
pool dosed 0.4 HP by the shot, 0.4 HP by another round, 0.4 HP by the shot again shows its point
lost at the shot's second mark, DERIVED). A record carries that count where the two readings
differ (`BoxDose::shown_taken`).
This is *local function state*, not a global health bar. A tank is knocked out when fewer than two
crew members are in action (`knockout_from_counts`, `src/damage.rs`; §1a).

- **Crew are witnesses** (low HP) — the knockout currency; a graze of the cone's edge leaves a station
  reduced, a station inside the cone's knockout radius is knocked out, and a clean main-gun
  transit's line alone empties the crew pool (10 cm of chord at 30 reference-mm/m is 30 HP,
  DERIVED). (HP, not binary, so fire-over-time and near-misses have somewhere to accumulate.)
- **Modules are tougher and repairable** — one stray fragment scratches an engine; a faceful or a
  direct transit wrecks it.
- **Reduced module performance:** current quality is the continuous held share of the pool;
  empty is out of action. There is no per-function checkpoint table. The owner chose this
  simplest pre-playtest law under #228; the checkpoint alternative is preserved in F18 and §7b.

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
optic; GunnerSight out of action → third-person). Both view stations out of action → dark;
the game's damage model labels the tank knocked out when fewer than two crew remain in action (§1a).

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
- **Module** → condition: `held / capacity`, continuously; an emptied pool supplies zero.
- **Crew** → `competence(crewman, station)` while in action: native full, foreign reduced.
  The current foreign competence is 0.6, a `StandIn` declared in `assets/damage/crew.ron`
  until crew/station training data exists; later × skill.
  A partly emptied crew pool does not change competence.
  Competence is **relational** (crewman × station), *not* an attribute of either — the seam the
  skill/training system plugs into.

**Combine (per frame, against the live world):**
```
member  = coeff × Π(quality of each Part it needs)
group   = Part/Pool: min(1, Σ members)   |   Backup: max(members)
effectiveness = min over groups          // 0 = unavailable, 1 = full
```
Subject: the game's damage model, game state only. Effectiveness is a **rate**. Load work is
a checked Q63 integer share in [0, 1] of the weapon's admitted base, with its retained positive rate and
reference base. Elapsed working ticks consume work at that rate; a pause freezes the remaining
share. A positive rate change derives `ceil(local base × remaining share / new rate)` once.
Unchanged rates retain their stable absolute deadline. Cached deadlines decide eligibility;
cached paused ticks serve the frozen readout, and neither can become unbounded rescalable work.
The local spec gate proves the full base fits at the requirement's smallest positive rate, so
any accepted work share fits too. Integer work makes non-finite shares unrepresentable.
Decoding refuses out-of-range work units, invalid rates, a zero reference base, and load work
beside a nonempty belt with typed errors. Exact integer ceiling arithmetic avoids a float
rounding residue adding a tick at pause/resume; Q63 is a `Resolution` choice whose work quantum
is below 2^-32 ticks over the admitted signed horizon.
Zero authored work uses only the one-tick duplicate-emission interval, independent of rate,
including zero. Cyclic intervals use the same work-free phase; neither interval nor Ready has
a load rate. The scalar work representation preserves actual work rather than turning tick
rounding into extra work. This is ADR-0029's stable absolute-deadline contract with bounded work.

Subject: the game's damage model, game state only. A module supplies its held HP share;
a crew station supplies its occupant's competence while in action and zero when knocked out.
A task resolves its requirement once: multiply within complete authored members, capped sum
across Pool members, best complete Backup alternative, and minimum across required groups.
The resolution retains each group's contributions and the selected Backup member beside the
rate. Equivalent alternatives use coefficient and canonical part order, never staffing, to
break ties. Empty graded groups are refused before play; an empty member is a pure ceiling.

Control authority is separately authored as `drive_controls`: `Any` lists complete staffing
alternatives, while `NotRequired` explicitly permits controls without a station. The load gate
refuses empty paths and undeclared stations. This authority permits axes, repair braking and
park-lever changes independently of task rate. An unavailable production path cannot erase a
staffed operator's intent. Drive consumes the resolved task rate directly, alongside the
engine's and transmission's own held shares for the idle governor and mechanical capacities.
No source condition is recovered by dividing projected rates. Brakes, engine
drag and compression braking keep their construction. The authored full-load curve is net
of construction losses: the pedal blend reconstructs its gross combustion torque, only that
source scales with delivered supply, and construction drag is charged whole. At whole supply
the gross/drag pair retains the prior healthy net torque at every command. An emptied engine
therefore keeps its friction at full pedal, including under declutched rev-match commands.
A small positive share may still pull away from the crank's stored energy; its sustained
capability must also pay construction losses. The free-hull evidence below replaces the
stationary-hull stall claim. The scheduler uses the crank's torque law, pricing sustained reserve at no less
than idle rather than spending the governor's temporary sub-idle recovery.

Both sprockets' midpoint work, including geared steering, is charged against combustion and
the crank's stored energy. The crank retains only unspent energy; neither acceleration from
rest nor a clutch re-anchor creates work. Inner-to-outer transfer pays the declared
recirculation loss; pure engine braking has no such transfer. An engine that cannot sustain
idle can run down to rest, and the recovery guard never supplies energy of its own. The
healthy idle-guard band is a StandIn for the missing stall/restart law, authored in
`assets/driving/engine.ron`; it does not also shape engine friction. The separate numerical drag-sign band is Resolution: one rpm,
at the public readout's whole-rpm resolution; above that band the run-down follows the authored
drag and crank inertia. A stopped public crank pauses its running sound loop.

A supply change takes effect on its tick. Through a locked clutch the torque step reaches
the belts on that tick; transmitted capacity and the Unmanned reset also step immediately.
Crank inertia limits speed changes, not those supply steps. A phase hold imposes brakes as an
operator rule; a repair hold requires the authored control authority. The panel labels the
resolved task throughput **Drive rate**, independently of holds and current control authority.
Net sprocket force also depends on crank speed, gearing, drag and absolute clutch capacity.
Module rows and simulation read one held-share function: the integer ratio is computed before
narrowing, and a reduced ratio that rounds to whole is held at the adjacent f32 below whole.
Servo slew keeps its rate scale. Automatic cyclic cadence, fire eligibility and sight access
keep their mechanism or on/off gates. The panel reports actual weapon deadlines and explicit
load pauses; the sight-derived Gunner percentage has no rate consumer.

The RON declares only the static *recipe* (structure + coefficients); HP, who's in action, and
backfill assignment are live inputs — knockout, a changed pool or a swap changes the inputs,
never the recipe. The **preserved checkpoint alternative** would author per-function quality
steps (for example, ≤50% held share selects lower power), with admission rules for the tables.
Its revert cost is medium: replace the module resolver, author and validate those tables, and
change the class-invariant tests and readout assumptions. The rate consumers and wire shape can
stay; no checkpoint arm or switch remains in code. Playtest fork F18 owns the feel question.

Subject: the game's damage model, game state only. **Provisional repair call (#228):**
repair may start on any module below capacity. It refills the missing HP at the shared HP/s
work rate and returns the pool whole on completion; the existing held share remains available
until then. Cancellation discards unfinished work. This avoids making a nearly empty module
less recoverable than an empty one. F18 carries this call into playtest.

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
(entity separation), the skill/training system, `min`-vs-`product` at the AND
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
  "empty your ready rack" play). In the game's damage model, an emptied ammunition HP pool cooks
  off and empties every crew HP pool. Fewer than two crew remain in action, so the one rule in §1a
  labels the tank knocked out. Turret launch is triggered off
  `CookedOff` directly, not via `TankKnockedOut` (see §1a — KO is retired as a gameplay gate).
- **Fire** — an **engine hit by a direct penetrator** (not fragments) has an ignition chance. Fire
  does **not spread**; it does range-per-tick damage to nearby crew/components and can be **put out**
  (a crew repair action). A dedicated **fuel** volume comes later.

## 9. Open tabs (deferred, named so they aren't lost)

- Spall strength is settled (§5: `spend × (v_exit / v_entry)²`); remaining tuning = the box counts
  (the owner's feel pass), the half-angle and axis fork, and whether a dedicated brittleness
  (plug vs debris) selects the failure mode.
- Contact proxies: density, speed, calibre, material factor and normal steel-equivalent thickness
  set the pressure-contact limit (§4a); deformation and shatter still need a law.
  Thin-plate potency uses the empirical work fit in §3b; a shared work/holding-pressure law and
  the one-calibre bulk-limit mismatch remain unresolved.
- HE: fuse minimum-thickness trigger + delay + dense wide cone (penetration-independent).
- Numeric magnitudes (contact presets and material factors): labelled data (§4a, `materials.ron`).
- Data homes: ballistic-volume geometry on the model (watertight); material factor + shell specs in
  RON (ADR-0010 spirit — ADR-0010 already flags per-plate armor as a genuinely node-attached case);
  crew/module/station definitions TBD.
- Repair detail (who, how long, occupies which station).
- Player feedback / legibility — how the player reads *what happened* to their tank.
- **Capability model (§7b) deferred edges:**
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
> x-ray), not from names. Paint is a **runtime livery** composed onto every surface whose
> material's visual takes paint (`tank-surfaces.md`) — never a material assignment in the source.

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
- **Boundary laws** (at transitions): contact, spall, entry/exit reads fire where
  the field *steps*.

**Cost law:** `cost = ∫ max(factor over covering volumes) dt`. The combiner `max` is **forced** by
the invariants, not chosen by taxonomy:

- **Idempotent** — the same steel claimed twice charges once (the plate-junction fix).
- **Monotone** — adding any volume never *reduces* protection anywhere (a crew-station box clipped
  into the turret wall cannot dilute the steel).
- **Commutative** — authoring order can't matter.

This is CSG union evaluated lazily on the 1D ray (Roth '82): overlapping *individually-manifold*
plates behave exactly as their boolean union, without ever building the merged mesh. Boolean union
is the special case of `max` over {absent, factor}; "steel beats rubber" is `1000 ≫ 50`, a
consequence rather than a rule. A witness (§6) contributes no factor: it is no part of the field.

**Damage law:** where volumes overlap, **only the strongest material counts** (Yan, 2026-10-31,
issue 191: a dosable part poking into armour is a modelling error). At each depth of each disc
sample, the volumes of the highest factor present there take the energy the round lost at those
depths (§6, the energy dose). Each sample contributes its maximum factor once to the cost weights;
its tied strongest volumes split that sample's energy share equally. A volume a stronger one
overlaps takes none of it, and its chord there is no part of its dosed chord. Sequential pools
take their own energy loss. A module half inside a plate is dosed over its open half only; of two
overlapping modules, the stronger takes the overlap's whole dose. A witness's chord inside any hard
volume takes no dose, and its chord in the open is dosed by its own law (§6): two crew overlapping
each other each take their whole chord, a crewman clipped into a wall only what lies outside it,
and neither charges the shell anything. Field directions use the first-entry stop law (§5): a
part is hidden when the plate's face is entered first. A weaker part whose face comes first stops
the direction itself, even if a stronger volume begins immediately behind it; near-equal factors
do not blend those ordered faces into a strongest-only field stop. The line rule this replaced
dosed every overlapping volume over its own presence chords, shared by cost.

**Spall law:** at every **body exit** — a step to a lower factor, or to air — a spall cone is thrown
with that body's own spend and speeds (§5), and its directions use §5's first-entry field law.
Everything on the wishlist emerges with no class rules:

- Crew never spall: a witness is no body (§6), so it throws nothing.
- Crew pressed against an inner wall: the steel backface is a body exit into air as far as the
  field is concerned, and the cone it throws doses the crewman leaning on it (§6).
- Buried steel faces (overlapped plates) are no exits — no seam spall, no seam VFX.
- A small drop into other steel (RHA→cast) throws a cone the material behind absorbs in
  millimetres — self-correcting, no suppression rule needed.

### 13.3 Surface-law amendment — factor-weighted thickness (required for classlessness)

Subject: the game's damage model; game state only. Contact (§4) reads the covered-sample
steel-equivalent thickness along the normal: the integral of material factor over real chord,
projected onto the face normal. Coverage does not dilute this thickness; it scales the resolved
skid's deflection and speed loss. Witness stations have no material surface law.

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
   - **It merges event topology, not material.** One entry face (contact/incidence
     once, at the outer face), one exit at the true inner face, contact against the run's summed
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

Entry-face semantics (incidence, contact test, the terminal/`Impact` reads) evaluate
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

- **Entrance:** `n̄` is the area-mean entry normal over covered samples. The pressure test and
  slope charge (§4) read it unchanged. A skid blends the mirror's deflection angle and retained
  speed by coverage η; a bite keeps the incoming direction. Whether it skids reads covered-sample
  thickness, independent of η. Every bite pays the slope charge, with a softer entrance paying
  only its reference-strength share.
- **Transit:** `cost = (1/k) Σᵢ costᵢ`, each sample's cost its union-max-factor chord integral times the slope charge,
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
  **existing ricochet law** fires with η-scaled speed loss *and deflection*: a graze IS a partial
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

**The hand-off: each sample resumes on the piece it crossed (RULED 2026-10-31, #191).** Only
that is the owner's ruling; the crossing's join, the in-air depth and the read built on them are
PROVISIONAL until he ratifies them. The crossing is staged — the entrance is read along the
incoming axis, and transit stays on that axis — each sample stops its entrance walk and resumes
from its own hand-off. It resumes on the face of the armour piece
it actually crossed, at its own crossing point, never on one plane shared across the disc. The
shared plane it replaces was the first piece's mean entrance plane, extrapolated across the whole
disc, and at a graze it lies far from any face: across an 88's disc it spread the hand-offs over
more than a metre of the incoming axis in two of the three replayed failures (MEASURED, #191
replays). It put samples past the face of a second piece the rest of the disc met behind or
beside a grazed first one, past both faces of a thin curved wall (a barrel's far wall, crossed
side-on from its bore), and behind a sample's start. A transit seeded there met an exit it never
entered or skipped the piece, and the round stopped at first contact (`CorridorMismatch`, three
per 240 s at 30 bots, MEASURED 2026-10-31 on the build after the turret-scale fix).

- **The crossing** (PROVISIONAL) is the disc walk's first event, then every event that begins
  before the crossing so far ends. Which of two pieces that overlap in depth is struck first, by
  however little, decides nothing: they are one crossing in either order.
- **Who hands off on a face.** Every sample whose first run is in a piece of the crossing, at
  that run's start.
- **Everyone else** — a sample whose first face, if it has one, is in a piece that begins at or
  past the crossing's end — hands off in air on its own ray, at the crossing's DEPTH
  (PROVISIONAL): the read's mean entry point, measured square to the incoming axis, clamped to the
  span between the sample's start and the crossing's end. The uncovered part of the disc stays
  where the disc is; a hand-off lies on ground the sample's walk covered and never depends on how
  far the corridor reached. No sample hands off past the first face on its incoming ray. The
  game's damage model certifies transit membership on the continuing rays at those hand-off points:
  a face behind the start's boundary band is owned by the seed, and a face within that band by
  the corridor. Membership belongs to the reached hand-off.
- **The entrance read is the crossing's faces** (PROVISIONAL). `n̄`, η, the steel-equivalent
  thickness and slope charge are read over the first run of every sample that hands off on a face, so
  a disc meeting two pieces that overlap in depth reads both surfaces, whichever it meets first. A
  run behind that face on the sample's own ray (a hull side behind a fender) is the transit's,
  never summed into the plate the round strikes. With one piece met face-first by every sample the
  read is that piece's alone.
- **A graze can bite**, under the provisional join and read. A graze that would ricochet alone
  bites when the disc also meets a second piece that begins before the grazed piece's crossing
  ends: the second face enters the read, tilts `n̄` toward the round and can bring the incidence
  into the biting side of the pressure-contact law (§4a). Corners and overhangs are where this happens
  (`a_graze_and_a_plate_struck_together_bite_as_one_crossing`).
- **The transit crossing** is the transit corridor's crossing, by the same rule, and it begins at
  the hand-off. The flight resumes past the crossing's end, so a run that began inside it and was
  resolved apart from it would be skipped, or charged only from the resume point on.
- **The game's damage model refuses contradictions.** A transit corridor that is not the one its
  entrance asked for is a broken invariant and panics in every build, naming the invariant and
  structured diagnostic (`WalkError::kind`). Invalid geometry is refused at load. Neither case
  becomes an armour verdict or a skipped dose.

Rejected: extending the entrance walk until it covers the shared plane wherever it lands (no less
shared, and a longer walk meets more pieces to land past — failures rose from 3 to 7 in the same
240 s run, MEASURED 2026-10-31); clamping the shared plane to the walked span (an outcome that
depends on the corridor's length: moving a hand-off by `δ` along the incoming axis `u` shifts the
bent ray `b` sideways by `δ (u − b (u·b))`, DERIVED).

**The game's damage model: the walk's clock and moving-body law (#191).** The walk runs
*after* the physics step. Each body is tested in its own frame, from its known tick-start pose
`from` to its known tick-end pose `to`: the round's world segment `p0 → p1` becomes
`from⁻¹ · p0 → to⁻¹ · p1`. The segment parameter is a moment in the tick, and the earliest entering
face over all colliders is first contact. Nothing is predicted. Between the known poses the body
moves on a straight chord (lerp/slerp); the departure from its true path is graze-sized (DERIVED
≤ 1 cm for a turning tank, ≤ 3 cm for a turret at an ASSUMED angular speed of 10 rad/s). Adjacent
intervals share both endpoint poses, so their chords agree at the boundary.

**One sampling instant.** First contact freezes every collider at that same tick fraction.
Contact and straight transit consume no world time while deciding the immutable
crossing. A third body whose entry has that same fraction belongs to the same contact set and
union walk: ordinary simultaneous geometry, rather than an inside-start error. Bodies do not
advance between these queries. The playback clock then delivers the already-decided effects.

- **Seconds, not metres.** A round spends the tick's seconds. Each mechanism stretch costs
  `Δt = ∫ ds / v(s)`, including the slowing inside material; open flight consumes the remaining
  seconds at the resulting speed. Allocating the tick's metres at its initial speed lets a slowed
  round run ahead of its clock (DERIVED up to 6.25 m in a tick in the counterexample). No distance
  debt or later payback can repair consequences already delivered ahead of time.
- **Decide once, then play back (option B, decided).** First contact decides an immutable crossing:
  its world path, speed along that path, and effects addressed by moment. Playback walks it at the
  round's real speed across tick boundaries. Each effect lands in the tick containing its moment;
  a later tick does not re-test moving armour or decide the same crossing again. Cumulative
  rounding is retained so changing the tick partition changes neither total dose nor path.
  The immutable result addresses a line dose at the last exposed point of its volume, a witness
  dose at the end of its read chord, and a cone at its exact corridor-progress exit. Doses convert
  to integer units once; a tick boundary cannot split and re-round one. The crossing's impulse and
  verdict are delivered at its physical exit or rest point, before any geometric resume nudge.
- **Clients consume outcomes.** Clients never test armour. They rebuild the path from the fire
  event and the server's outcomes; an outcome arriving late is shown when it arrives.
- **A start inside a collider is a bug**, and panics. Playback inside its already-decided crossing
  is not a new contact-discovery start. Bodies move rather than being placed: a felled tree falls.

**Runtime contract.** The endpoint-local segment discovers contact, then every collector,
transit probe, witness line and field reads the same lerp/slerp scene at that fraction.
The struck point's velocity is collider-origin translation plus angular velocity crossed with its
rotated local contact offset; child servo and recoil motion therefore participate. The mechanism
uses the round's velocity minus this velocity. Its immutable relative path is carried back to world
by the struck point's constant contact velocity, including each timed effect and sampled recording
frame. No second world clock advances while that crossing is decided. The crossing completes the
struck physical body's actual material along its sample rays,
including gaps between its parts, in that scene. A reflection ends that immutable contact:
playback spends its approach and outgoing flight time before discovering another face, so an
unbounded future of slowing reflections is never planned in one tick. Completion reads real exit
faces; a finite support bound only sizes that collection. Other bodies return to relative discovery at the
elapsed tick fraction. This preserves the static body's immutable path without a baked envelope.
Ground
is only the declared height grid; geometry-only fixtures have no ground. An Armor collider without
ballistic-volume ancestry is a bug at discovery. No envelope or future-pose prediction participates.
Discovery uses the existing topological
boundary band to recognize a represented point on a face after f32 coordinate conversion; an
interior start beyond that band panics. This is face classification during the sweep, not a
containment certificate supplying tick-start membership.

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
- The game's damage model doses a component's line chord only outside stronger walls (§13.2).
  Field directions still stop at the first entered hard face: a protruding component can take a
  field dose before a wall hides it. The line's overlap tolerance is not a field-fit guarantee.
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
needs no tie-break for cost, and damage shares a tie by cost (§13.2).
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
  (Superseded 2026-10-28, #116 and #145: the shock is the energy the round lost at the bounce,
  the motion pointed into the face, §6.)
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


## Review record

### Labels (#245)

Subject: provenance on the game's authored data and damage model, game state only. Crew are HP
pools at stations. Reviewer: Claude, `review-labels-1-claude.md`; finding IDs below follow its row
order. Decisions are checked against the code, rather than inherited from the audit's prose.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | L1: shipped files admit Fixture under cfg(test) | verified: replacing the embedded crew_hp Unit label with Fixture returned a 30-HP pool in the red regression | provenance: refuse fixtures at every shipped load boundary (#245) |
| Claude | L2: filename assertion adds its own prefix | verified: the old test constructed `fixture.ron: {err}` itself; the regression now reads BoxClasses' real admission error, including crew_hp and repair | provenance: refuse fixtures at every shipped load boundary (#245) |
| Claude | L3: 37 test values carry fact/resolution labels | verified: five Measured fixtures in substances, five Derived fixtures in attachments, three Resolution fixtures in spall_cone, and 24 Unknown fixture values in rounds/effects | tests: label inline authored data as fixtures (#245) |
| labels-2 | N1: five more Unknown("f") test values | verified: the drawn-body fixture helper labels five absence/model values Unknown; these have no external fact source | tests: label inline authored data as fixtures (#245) |
| Claude | L4: rate-strength StandIn sits on an unread rate | verified: Compaction multiplies compressive_strength by compressive_dif for both Brick and Wood; only the load gate reads the reference rate | data: align provenance labels with their readers (#245) |
| Claude | L5: resistance and presentation siblings disagree | verified: the four resistance values fill compare.md's A9/A10/A11/X24 law gaps; 60 material-feedback and 21 round-effect Unknown values feed view choices | data: align provenance labels with their readers (#245) |
| Claude | L6: Taste's rule excludes absolute magnitudes and choices | verified: ribbon_vertices reads absolute width/growth/lifetime; fuze resolve matches name against default_mode, so the mode identifier now takes the Derived job of the delay it selects (the schema requires Sourced) | data: align provenance labels with their readers (#245) |
| Claude | L7: RHA Young modulus is not Measured | verified: its own note selects 205 GPa inside a 190–210 GPa range with no RHA-specific source; Unknown now states the missing measurement | data: align provenance labels with their readers (#245) |
| Claude | L8: anchor_nose and axis are not computed | verified: anchor_nose selects the fixed drag stand-in's calibration convention (StandIn); axis is an assumed physical direction proxy awaiting angular measurements (Unknown) | data: align provenance labels with their readers (#245) |
| Claude | L9: shipped round notes say fixture | verified: four core notes, the APHE display note and two reference/proxy comments describe shipped choices using test-only vocabulary; wording now says provisional/reference | data: align provenance labels with their readers (#245) |
| Claude | L10: exact algebra is caught; job list adds law coefficient | verified: the lens had no exact-algebra exemption and omitted stand-in from the job list; AGENTS and the lens now state the law-form exemption, and the lens uses the five jobs | data: align provenance labels with their readers (#245) |
| Claude | L11: four non-bisectable main commits | verified by historical source: the new enum lacks Tuned while assets still use it; muzzle retains Tuned through the next two subjects, and bore_length fixtures retain Tuned through the fourth | not fixable; noted: provenance: name each authored value job (#245); data: label authored values by their job (#245); agents: enforce one number one job (#245); data: label the muzzle magnitudes as taste (#245) |

The refusing policy surrounds shipped file readers, including nested registry parses and the raw
embedded-sheet readers used by CI. Tests authoring inline fixture text (including temporary-file
I/O fixtures) use the test policy; a scoped policy on each test thread restores its caller on return
or unwind. Ordinary builds always refuse Fixture.
No historical commit is rewritten.

**Admission gates (MEASURED):** the initial embedded-value regression failed by accepting Fixture.
After the fix, the broader targeted run passed 915 tests (12 existing ignored measurement tests),
including the requested provenance/spec/rounds/materials/substances/ballistics/buildings filters,
headless, pen-cam and field-cast checks, and whole-tree guards. The final tree admission/unwind
assertions, provenance tests and whole-tree guards passed another 82 tests. Formatting and CI
clippy (`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`) passed.

**Fixture-label proof (MEASURED):** 42 metadata labels changed across five source files: the
review's 37 plus five same-class `Unknown("f")` values. Canonicalising provenance-kind tokens,
formatting both texts with rustfmt and removing whitespace makes every file identical to its
parent. Formatting also removes one optional trailing tuple comma. Values and assertions are
unchanged.

**Fixture-label gates (MEASURED):** 83 targeted tests passed, including all five changed test
modules, provenance and doc-citation guards. Formatting and CI clippy passed.

**Classification decisions:** absolute presentation magnitudes and nonnumeric view choices are
Taste. A physical-size-driven effect still scales that physical quantity. The fuze mode name is
an identifier selecting its derived delay, rather than presentation taste; the required Sourced
schema stays unchanged. The drag anchor selects the existing Mach-drag stand-in's calibration
convention. The spall axis remains an unknown physical proxy with the measurement that would settle
it stated in the note. Rate-dependent strength is absent from compaction: its constant multiplier
owns the StandIn, and the unused reference rate keeps only its datum's provenance.

**Overlap reserved for law lanes:** `assets/ballistics/spall_cone.ron` changes only `axis`'s label
and note. Its `energy_share` and `driven_energy_factor` notes are untouched for law-energy-units.
`assets/ballistics/round_types.ron` changes only `drag.anchor_nose`'s label and note; hit noses and
their notes, including the header's hit-nose wording, remain for law-walk-angles. The admission
commit also scopes the existing readers in spall_cone.rs and energy_dose.rs: preserve those
refusing boundaries when the law lanes rewrite the fields. The round proxy header changes only
its obsolete fixture wording; no nose value changes.

**Authored-data proof (MEASURED):** a RON token comparison of all eight touched asset files leaves
284 provenance labels before and after. With comments, provenance-kind tokens and their note
strings removed, every remaining value token is identical. The round files contain no remaining
fixture wording. The two spall energy notes and the round-type hit noses/notes are byte-for-byte
unchanged; their law lanes retain ownership. No authored value or schema changes.

**Final gates (MEASURED):** 915 tests passed, with 12 existing ignored measurement tests. The run
included the requested provenance, spec, rounds, materials, substances, ballistics and buildings
filters; the headless, pen-cam and field-cast checks; and the whole-tree `doc_citations`,
`ui_ascii`, `net_boundary` and `bake::` guards. Formatting, `git diff --check` and CI clippy passed.
The eight-file, 284-label value-token comparison passed again on the final authored data.
All eleven review findings are verified: L1-L10 are fixed, L11 is not fixable and noted. N1's five
additional fixture labels are fixed. No owner decision remains; the named law-lane overlaps above
are intentionally reserved.


### Energy-unit review (#258)

Subject: the game's damage model, game state only; crew are HP pools at stations.
Both supplied reports were read before this table. The Codex report was present initially and
reported no findings; the Claude report arrived during this lane. E1–E8 follow Claude's order.
The Codex conversion check agrees with the code: field encoding includes the line unit and
HP decoding cancels it. That does not establish independence of physical environment loads.

The dispatch also reserves `src/ballistics/` for other lanes. That explicit path boundary is
kept: verified fixes needing those files remain open, rather than being called fixed by prose.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | E1: transport strength is consumed as physical crushing energy | verified: with `joules_per_hp = 6000` instead of 3000, the cone coefficient and `read_shell` ray energy double, so `Load::field` doubles crushed volume while emitted HP stays fixed; the cone record has no separate physical-energy carrier | `ballistics: carry each field's energy and convert it to HP at each edge (#258)`: a field's strength is the energy it carries over the emitted share, a box's dose converts through its mechanism's `FieldUnit`; MEASURED, a fixture house chunk under a 2.7035 MJ ratio-1 cone crushes 0.019958 m³, was 0.008781 m³; the shipped cone and ring still emit 396.51 HP and 13,920.84 HP. E1's "doubling `joules_per_hp` changes crew dose" is tested as the field's own unit at two fixture values, since no field reads `joules_per_hp` after this change; the crushed volume is superseded by the energy-partition review below |
| Claude | E2: emission tests pin shipped balance and construction | verified: editing either emission unit or the named round's construction breaks `the_reference_emission_totals_are_unchanged`; the 1,800,000,000 fixture also depends on the shipped line unit | `ballistics: carry each field's energy and convert it to HP at each edge (#258)`: `a_field_emits_its_sources_energy_and_hp_through_its_own_unit` and `a_fields_hp_unit_moves_crew_dose_and_never_crushed_volume` hold fixture units; the shipped totals print from an ignored measurement bed that asserts nothing |
| Claude | E3: deleted scenario evidence and open material call | verified against the predecessor of `ballistics: declare per-mechanism energy-to-HP units (#258)`: s1/s12, s13, #116/#182 and the sub-reference exit call were removed | `docs: preserve the game field-law scenario record (#258)` |
| Claude | E4: the sphere target does not explain the ring's emitted total | verified: DERIVED 7,540 HP target versus 13,920.84 HP ring emission; the missing bridge is the retired ball's 214.8 kJ calibration | `docs: explain the ring unit's sphere-target calibration (#258)` |
| Claude | E5: share terminology survives the unit conversion | verified: `energy_share` now derives an encoding coefficient from HP/J and J/HP; the walk API and test prose still call it a kinetic-energy share | `ballistics: carry each field's energy and convert it to HP at each edge (#258)`: `energy_share` and `strength_at` are gone; the walk, the throw and their tests speak of the energy a field carries |
| Claude | E6: sampling notes present old transport factors without the emitted-HP derivation | verified: notes use the reference line conversion and coefficient 210 although emitted HP is the authored quantity | `docs: express field sampling notes in emitted HP (#258)` |
| Claude | E7: one-rate paragraph overstates the line unit's scope | verified: `energy_units` handles hard line/bounce deposits, while fields encode their separate units through `field_strength_per_joule` | `docs: limit the hard-pool energy unit to line and bounce (#258)` |
| Claude | E8: the encoder test does not itself sum live deposits | not a defect: that test checks the encoder seam; `a_thrown_cone_is_its_rear_layers_field` separately asserts every live ray equals cone strength times its exact weight, so the proposed extra 1.1 factor fails that runtime test; the face/intensity test checks deposition | none |

### Energy-partition review (#258)

Subject: the game's damage model, how the energy a round loses at a plate is shared between the
line's dose and the spall cone's field; game state only, crew are HP pools at stations. Both
reports on the energy-unit lane were verified against the code before this table.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude P2 | the energy a round loses across a body is spent twice: the line deals the whole ΔE to the body's volumes and the exit's cone carries ΔE × r more, which buildings crush with; `r` did a partition job with no label | verified: `SpallSource::strength` was `field_strength(energy × strength_ratio)` while `finish` dealt every stretch's whole loss to the line, and three walk tests pinned line deposits alone at ΔE on perforations | `ballistics: spend a body's lost energy once between its line and its cone (#258)`: the cone carries `κ r ΔE` (`cone_debris_energy_share`, then labelled `Unknown` 0.06; current classification is `StandIn`, §5 "The energy partition"), the line `(1 − κ r) ΔE` sample by sample, the cone's HP unit is `u_C / κ`; the walk tests now sum line and fields to ΔE |
| Codex P2, Claude P3 | the pool-quantum gate lets a field's HP unit decide whether its joules reach a building | verified: `throw_spall_cone` and `throw_burst` returned before walking a gated field, so the shell reader saw no rays | same commit: the gate withholds HP doses only; a gated field carrying energy is walked and recorded without doses; `a_cone_below_the_pool_quantum_doses_no_pool_and_is_still_thrown` |
| Codex P3, Claude P3 | §5's knockout radius still read `E = qS/j`, the gate "the energy rate", potency colour `ρ = 1/joules_per_hp` | verified: `qS/j` on the 2.7035 MJ cone gives 901 HP against the 396.5 HP quoted | `docs: state the energy partition and the HP-only gate (#258)` |
| Claude P3 | the glossary's Field, Spall cone, Dose and Energy-to-HP rate said every field converts at `joules_per_hp` and a cone carries "one fixed share" | verified against `spall_cone::FieldUnit` and `ballistics::field_units`; Line dose had the same gap | same docs commit, Line dose included |
| Claude P3 | nothing tested the pen cam's per-mechanism joule-to-HP conversion | verified: no test compared a planned field's HP with its mechanism's unit | code commit: `each_planned_field_reads_its_own_mechanisms_unit` compares every recorded field's planned HP with the sim's dose edge at its own unit |
| Claude P3 | the crew half of `a_fields_hp_unit_moves_crew_dose_and_never_crushed_volume` passes the `ShotDose::add_field` mutant, and the E1 row omits the unit substitution | verified by reading: the crew half called `field_units` itself | code commit: the crew half sums through `ShotDose::add_field` against `u × joules` at two fixture units, neither 1/3000 (the mutant gives 2.2× and 1.1× them, DERIVED, not run); E1 row amended |

MEASURED after the partition, `spall_cone::tests::the_shipped_emission_totals`: the reference
cone (2.7035 MJ lost, ratio 1) emits 396.513336 HP, unchanged, and carries 162,210 J; a Brick
chunk every direction stops on (135.46 MJ/m³) crushes 0.001197 m³. Neither earlier figure is now
true: 0.019958 m³ spent the lost energy a second time and 0.008781 m³ read the HP unit as
energy. The ring emits 13,920.84 HP, unchanged. In the re-recorded pen cam scenarios s10's
barrel line dose moves from 297.19 to 279.36 HP, `1 − κ` of it, since gun steel exits at ratio 1,
and still empties its 250 HP pool (the 245.18 HP §5 quoted was already stale); s9's breech moves
by 0.00015 HP; field doses move by at most an f32 step; no pool's state moves.

### Reduced modules and capability rates (#228)

Subject: the game's damage model, game state only. Module quality is its pool's held share;
crew quality remains occupant competence while in action. The production rate consumers are
reload, belt swap, servo slew and the drive torque supply described in §7b. F18 preserves the
checkpoint alternative in prose. Declared protocol revision is 65: the atomic weapon gate now
carries checked normalized work inside load phases. Its state hash includes the share, rate, reference base and work anchor;
Ready and work-free intervals carry no rate.

MEASURED in generic production-system fixtures: the half-full engine serves 0.5, an empty engine
zero; a foreign loader takes 54 ticks for 32 base reload ticks and 107 ticks for 64 base swap
ticks at the current 0.6 competence. The pause/rate-change fixture keeps its frozen gate stable
and reaches its re-derived deadline. Drive keeps its 0.8 throttle command while its torque
supply becomes the foreign competence or 0.5 under half engine condition. These are fixture observations, never shipped
asset pass conditions. The panel removes the unused sight percentage and labels paused work
even when the frozen interval is zero.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | P2: reduced modules cannot start repair | verified: the 1 HP held fixture cannot start; repair now owes only missing HP | `damage: repair reduced module pools (#228)` |
| Claude | P3: reduced module row says READY | verified: the row now shows its held-share rate | `damage: repair reduced module pools (#228)` |
| Claude | P2: scaling intent weakens service brakes | verified in `driver_intent`: opposite throttle is the service pedal; intent now stays full | `damage: preserve drive intent under reduced supply (#228)` |
| Claude | P2: full command below the clutch threshold cannot launch | verified: scaling intent crosses the neutral threshold; positive supply now engages under a full command | `damage: preserve drive intent under reduced supply (#228)` |
| Claude | P3: Drive readout misses phase hold | verified: the panel now consumes the production selector, including both brake holds | `damage: preserve drive intent under reduced supply (#228)` |
| Claude | P3: foreign competence is bare code data | verified: uniform competence is a labelled shared stand-in, not a per-crew fact | `damage: declare foreign competence and pin load floors (#228)` |
| Claude | P3: load horizon test misses each contributor-floor mutant | verified: separate crew and module fixtures each fit at full rate and exceed the horizon at their own floor | `damage: declare foreign competence and pin load floors (#228)` |
| Codex | P1: a decoded peer span panics when rescaled | verified red with paused work 2,147,483,647 at rate 0.5; bounded normalized work now parses at the wire edge and scales against the local admitted base | `damage: bound replicated load work before rescaling (#228)` |
| Codex | P2: zero work scales the duplicate-emission guard | verified red at rate 0.5; zero work now keeps a work-free one-tick interval at every rate, including zero | `damage: bound replicated load work before rescaling (#228)` |
| Claude | P3: Ready and cyclic gates carry meaningless load rates | verified: only Loading and Paused now hold `LoadWork`, including its retained rate | `damage: bound replicated load work before rescaling (#228)` |
| Claude | P3: stale gate, trace, protocol, panel fixture and drive prose | verified against their consumers: fire gates versus load rates, complete work hashing, REV 64/65, current Drive text and delivered torque | `damage: declare foreign competence and pin load floors (#228)`; `damage: preserve drive intent under reduced supply (#228)`; `damage: bound replicated load work before rescaling (#228)` |

MEASURED in regression fixtures: the original peer span panicked before the fix; its old wire
shape is now refused. A maximal cached paused span with one full work share resumes in 64 ticks
for a local 32-tick base at rate 0.5. Zero-work reloads mature after one tick at rates 0, 0.01,
0.5 and 1. A module holding 1 HP of 21 completes its missing-HP repair on the 64th fixture tick,
not the 63rd. The production driving adapter preserves full throttle and pulls away under a
30/336 engine share, with MEASURED belt speeds of 0.037314706 m/s reduced versus 0.20654215 m/s
full in the stationary-hull fixture after input slew. These are generic fixture observations, not authored vehicle pass conditions. No capture or pricing run was taken.

The wire check also exposed a related phase case: a peer could name load work beside a nonempty
belt and later reach the pause assertion. The load-phase parser now refuses both active and
paused versions at decoding (`damage: bound replicated load work before rescaling (#228)`).

MEASURED regression mutants: removing both contributor floors admits the fixture's Breech and
Gunner recipes, and the horizon test reports both; scaling intent makes full throttle read
0.08928572 in the 30/336 fixture; replacing the reduced row with READY fails the 14/142 fixture.
Those mutants are removed. A seven-tick native load paused with one tick left exposed a float
work residue that resumed as two ticks; the Q63 late-pause regression preserves one tick,
including at the largest admitted span.

A second outside-input case follows from missing-HP repair: an active progress fraction beside
a full pool has no work denominator. `ModuleRepair::from_snapshot` now parses the complete
prospective timer before the atomic health record is applied, refusing that contradictory
snapshot with a typed error and visible handling. Its regression preserves the prior valid
snapshot (`damage: bound replicated load work before rescaling (#228)`).

The same outside-input boundary now refuses active repair metadata on a non-module pool,
rather than dropping the timer while applying its HP. The membership check uses the shared
`repair::Modules` filter and a typed error; the regression preserves the previous complete
record (`damage: refuse repair work on non-module pools (#228)`).

A brake hold is an activity, not a change to the engine's available supply. Both phase and
repair holds carry the derived torque share through the same selector, so idle/drag and the
scheduler retain the supplied envelope while the brakes are held. The panel reads the hold
as zero applied Drive. The selector fixtures check full, foreign, half-pool and empty supply
under a phase hold; the repair fixture checks the same distinction while repair is active
(`damage: preserve torque supply during brake holds (#228)`).

Final gates: MEASURED 885 targeted tests passed, six intentionally ignored; all 44 whole-tree
`doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards passed. Formatting and the CI
workspace/all-target clippy feature set passed. The lane took no captures or pricing runs.

#### Who drives, and what a reduced engine delivers (#228)

Subject: the game's damage model, game state only. The second review of the reduced-supply
lane, decided against the code. Every fix landed in
`damage: let the Driver station decide intent and each part scale its own supply (#228)`.

| Reviewer | Finding | Verified / not a defect | Fixed in |
|---|---|---|---|
| Claude | P1: with the Driver station's crew member out of action, intent stayed live (full service brakes on S, park release, ladder swap, clutch latch) | verified: S held at 5 m/s stopped the tank in 2.09 s | the crew view decides intent; nobody at the station is `DriveIntent::Unmanned` |
| Claude | P2: at Drive 0 the crank froze, so the public engine speed held its pitch | verified: 2443 rpm held for 1 s after the engine pool emptied | the engine view scales only combustion and the idle governor; an engine that cannot idle has no stall floor |
| Claude | P2: scaling peak torque scaled motoring drag, and the clutch clamped engine braking again | verified: a half engine coasted a descent faster than a whole one | drag stays construction; only the transmission view scales clutch and steering capacity |
| Codex | P2 (same as Claude's first P3): the brake half of the reduced-supply test bypassed `step` | verified: `driver_intent` and `apply_brake_stop_forces` never read the supply | the test brakes through `step` and requires forces equal to the whole engine's |
| Claude | P3: the brake test bypassed `step` | verified, as the Codex P2 | as the Codex P2 |
| Claude | P3: supply now steps within a tick where the old command slew faded it, with no record | verified | recorded, not restored: the slew is input shaping, and an engine's torque falls on the tick it loses combustion; crank inertia limits speed changes, while a locked clutch passes the torque step on that tick (§7b, F18 watches it) |
| Claude | P3: rounding showed a serving module as 0 % and a short one as 100 % | verified: 1 of 336 read 0 %, 335 read 100 % | the panel clamps a serving share to 1–99 % |
| Claude | P3: `tp.clone()` copied five vectors per tank per tick | verified | each quantity scales where it is used; the scheduler takes the supply |
| Claude | P3: F18 "Lives in" omitted the drive-supply law, repair and crew data | verified | F18 lists them |

MEASURED through the production adapter on a fixture hull (data-derived running gear, Drive
`[Driver, Engine, Transmission]`, 336 HP fixture pools), before → after:

| Case | Before | After |
|---|---|---|
| Driver out of action at 5.00 m/s, S held | stopped in 2.09 s over 6.14 m | no command: bit-identical to released controls, 1.14 m/s and 43.7 m after 20 s |
| 10° descent from 5 m/s, throttle released, speed after 10 s | whole 16.53 m/s, half engine 17.88 m/s | whole 16.53 m/s, half engine 16.21 m/s |
| Engine pool emptied at 2443 rpm, W held, rpm after 1 s | 2443 (frozen) | 1284 (2061, 1786, 1520 at 0.25 s steps); on recovery 1341 rpm on the first tick, pulling away again |

The MEASURED −0.011 m/s belt reading for 30 of 336 HP came from a stationary-hull fixture
(half engine 0.160 m/s, whole 0.207 m/s); it did not establish free-hull pull-away. The corrected
free-hull measurements below supersede the inferred stall claim. A Drive requirement naming
a module other than Engine or Transmission is refused at load.
`capability_effectiveness` lost its last consumer and is deleted. No wire type changed.

Final gates: MEASURED 898 targeted tests passed, six intentionally ignored, including the
whole-tree `doc_citations`, `ui_ascii`, `net_boundary` and `bake::` guards; formatting and CI
clippy passed. No capture or pricing run.


#### Drivetrain review doubles: work, complete paths and unmanned controls (#228)

Subject: the game's damage model, game state only. The two sweep-dmg-2 reviews were checked
against the production adapter and `transmission::step`. Fixture pools, operating points and
hull inertias below are test data, never asset pass conditions. All fixes are in
`damage: conserve drivetrain work and preserve unmanned controls (#228)`.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | P1: stopped engine powers a pivot from zero combustion | verified red through `step` and a hull free in translation and yaw; pre-tick work missed acceleration | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Codex | P2: independent Backup views splice incompatible paths | verified red with the review's Driver/Engine and Commander/Transmission paths: neither complete, yet full supply | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Codex | P2: coast comparison passes if unmanned always brakes | verified by that mutant; the test now also compares a serving driver's full brake | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Codex | P2: reduced module rounds to RATE 100% in f32 | verified red at 19,999,999 of 20,000,000 pool units; integer units now clamp a reduced row below whole | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P2: DRAG_THROTTLE_RELEASE also prices reduced combustion | verified red at full pedal and quarter supply; release now reads command, produced torque reads supply | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P2: reserve pricing omits engaged drag | verified red: MEASURED lab reserve 42,676.574 N versus crank 14,814.572 N at the same operating point; crank and reserve now share the torque law | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P2: pinned-hull pull-away row proves nothing at 30 HP | verified; the row's assertion-free arm is removed and the free hull pulls away before and after | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P2: repair becomes a brake toggle with nobody at the station | verified red: MEASURED one-second coast 4.5573964 m/s versus repair 2.1325536 m/s; repair now leaves unmanned coasting bit-identical | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P3: an unmanned standstill sets park | verified red; the lever retains its state and the free grade run continues rolling back | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P3: stall-guard, drag and idle-floor comments are stale | verified against the guards and torque law; comments state conditional idle protection, command release and reachable rest | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P3: stopped engine retains its running loop | verified red at zero public rpm; birth is paused and live playback pauses at rest | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P3: share() and Drive line claim sprocket delivery | verified; both now identify nominal supply condition rather than net traction | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P3: brake test misses transmitted/competence mutants | verified: MEASURED mutants yield 0 N and 72,000 N instead of 120,000 N per belt; independent tests reject each | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |
| Claude | P3: inertia does not smooth a locked-clutch supply step | verified; the one-tick combustion and capacity changes are pinned through `step`, and the narrative records them as steps | `damage: conserve drivetrain work and preserve unmanned controls (#228)` |

MEASURED by headless production-adapter tests on free fixture hulls, before → after:

| Input | Before | After |
|---|---|---|
| 30/336 HP engine, full W from rest, speed after 5 s | 0.12977146 m/s | 0.5090819 m/s |
| Engine pool empty, crank and belts stopped, full steer, yaw rate after 1 s | −0.09843076 rad/s | 0 rad/s |
| Unmanned hull at 5 m/s coasts uphill on a 10° grade, observed for 20 s | brief rollback then automatic park; final speed approximately zero | continued rollback; park unset, final speed −10.139181 m/s |

Decision: nobody at the Driver station can command repair braking or move the park lever;
repair work may continue, and a phase hold remains an operator-imposed brake. A Backup chooses
one path before projecting source shares; an available occupant retains intent even if that
path's engine is out of action. No wire type changed; PROTOCOL_REV remains 66.

Additional verified finding: the unlatched near-rest brake-entry blend also commanded brakes
without an occupant. It now requires a serving station; a lever already set still holds.
The no-combustion energy tests cover stopped and spinning cranks, propulsion, pivots and geared
turns with asymmetric rolling starts. Every temporary mutant and before-measurement helper
was removed. No capture, pricing run, issue comment or push was taken.

Final gates after rebase: MEASURED 889 targeted and whole-tree guard tests passed, six intentionally ignored,
including `damage`, `tank::`, `track::`, `crew_ui`, `sfx`, `net::`, `trace::`, `headless_test`,
`doc_citations`, `ui_ascii`, `net_boundary` and `bake::`. Formatting and CI workspace/all-target
clippy with `dev_ui,bitprobe` passed. No screen, capture or pricing run was taken.

The before column uses the dispatch tree headed by `docs: state the carried-energy field law
in §§5–6 (#258)`. The after column includes the rebase onto `driving: record surface contact
traces and entry-dependent speeds (#146)`. To separate that incoming ground-law change from
this fix: MEASURED before the rebase, the corrected drivetrain gave 0.510038 m/s pull-away and
−10.141057 m/s rollback at the same observation times; the stopped-engine pivot was already
zero. The rebased measurements above and all gates were rerun on the combined tree. The lane
changed no `forces.rs` source and no wire type.


#### Drivetrain review follow-up: complete paths, construction drag and feature boundaries (#228)

Subject: the game's damage model, game state only. The sweep-dmg-3 review inputs were rerun
against production requirements and the adapter. The combine interpretation is stated only
in §7b above. The first fix group is `damage: unify complete crew paths and held shares (#228)`.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex | P2: empty Backup reaches a live expect | verified red: the fixture sheet accepted Backup([]); Pool([]) also refused by the fix | `damage: unify complete crew paths and held shares (#228)` |
| Codex | P2: zero-score Backup tie discards available Driver intent | verified red with the supplied paths and both authoring orders | `damage: unify complete crew paths and held shares (#228)` |
| Codex | P2: unconditional tests read optional bitprobe fields | verified: no-feature test compilation failed with E0609 | `damage: unify complete crew paths and held shares (#228)` |
| Claude | P2: zero-score Backup tie depends on member order | verified, same reproduction as Codex | `damage: unify complete crew paths and held shares (#228)` |
| Claude | P2: Pool views splice incomplete contributors | verified red: scalar effectiveness zero but projected product one | `damage: unify complete crew paths and held shares (#228)` |
| Claude | P2: member coefficient is squared and also cuts whole transmission capacity | verified red: supplied backup fixture read 0.6/0.6/0.6; fixed at 0.6/1/1 | `damage: unify complete crew paths and held shares (#228)` |
| Claude | P2: Backup expect cites absent load check | verified, same gate defect as Codex | `damage: unify complete crew paths and held shares (#228)` |
| Claude | P3: two implementations of group combination | verified by source; one combine core now serves scalar and projected reads | `damage: unify complete crew paths and held shares (#228)` |
| Claude | P3: integer module row and f32 simulation disagree near whole | verified at fixture 19,999,999/20,000,000 units; both now consume the same reduced share | `damage: unify complete crew paths and held shares (#228)` |
| Claude | P2: full pedal removes an emptied engine's construction friction | verified red through torques and free-hull production adapter | `damage: retain engine construction drag at every supply (#228)` |
| Claude | P2: idle-guard band also sets the motoring fade | verified: shared scale in source; declutched empty-engine fixture stayed at 500 rpm for 10 s because rev-match also released drag | `damage: retain engine construction drag at every supply (#228)` |
| Claude | P3: recirculation test bypasses production motor and bisection | verified by source; replacement calls step, binds the work budget, and compares recorded to physical midpoint output powers | `damage: retain engine construction drag at every supply (#228)` |

Additional verified build finding: without dev_ui, three unconditional headless tests named
sandbox modules excluded from the build. Those sandbox-only tests now share their modules'
feature boundary; the headless gameplay tests stay available. No wire type changed, so
PROTOCOL_REV remains 66. The scheduler, hill hold, park latch and input slew were not redesigned.

First-group verification: MEASURED 78 targeted tests passed, including doc_citations, the
Backup and Pool free-hull adapters, scalar/projection recomposition, the zero-upstream module
condition and the near-whole row. Formatting and CI workspace/all-target clippy passed.

Second-group MEASURED production results, fixture inputs (game state only):

| Input | Before | After |
|---|---|---|
| Engine pool empty at 5 m/s, free hull, distance over 3 s, W held / released | 14.995418 / 13.274488 m | 13.056864 / 13.056864 m |
| Declutched empty engine at 500 rpm, no transmitted work | 500 rpm still held after 10 s | rest after 0.828125 s; DERIVED base-drag upper bound J × initial angular speed / base drag = 1.1423973 s |

The healthy net-curve test spans fixture commands 0, 0.25, 0.5 and 1 at fixture speeds
500, 600, 1100 and 1550 rpm. The free-hull 30/336 HP pull-away now measures 0.11817121 m/s
after 5 s; the earlier review table's 0.5090819 m/s belongs to the prior loss-scaled law.
No scheduler, hill-hold, park-latch or input-slew behavior was separately changed; the shared
reserve calculation naturally consumes the corrected engine torque. No wire type changed.

Final implementation gates: MEASURED 728 requested-module tests passed with five intentionally
ignored; the only first-pass failure was the now-unused DRAG_SAT_SPEED citation allowance.
After deleting that allowance, all 44 whole-tree guards passed (doc_citations, ui_ascii,
net_boundary and bake::). The build without optional features (`test --lib --no-run`),
formatting and CI workspace/all-target clippy passed. The five focused production tests also
passed, including the physical-output bisection check. Explicit old-motor, old-fade and
near-whole rounding mutation execution remains to be recorded within the lane budget.
No capture, pricing run, issue comment or push was taken.

#### Drive task bottlenecks and physical sources (#146)

The game's driving and damage model uses one evaluated Drive task rate for commanded
combustion. A transmission-only reduction therefore lowers combustion as well as the clutch
and steering capacities its physical condition limits; these are separate work and capacity
constraints, not a reconstructed product of source shares. `TankSpec::validate` requires Engine
and Transmission on every productive Drive path. A staffed task at zero rate opens the main
clutch, so the engine's local idle catch cannot launch the hull while Drive reads zero.
`transmission_bottleneck_reduces_the_one_drive_task_rate` pins the generic bottleneck outcome.

### Exchange and receiver review (#258)

Subject: the game's damage model; game state only. Crew are HP pools at stations. Current
§§5–6 supersede the earlier records' source-body immunity and per-field pool-quantum gate.

The lane's steps delete these paths:

- `ballistics: give every crossed body its exchange once (#258)` removes entrance-owner summed
  impulse, the continuing carrier, parallel dose/exchange vectors, the second debris-loss
  computation and the lost-then-retained accumulator. One addressed exchange holds retained dose,
  emitted work and independent momentum. Entrance impulse stays at entry rather than rear chunks.
- `ballistics: let returning debris dose its emitting body (#258)` removes permanent source-body
  immunity and the hollow-body line-only contract; continuous departure remains geometric.
- `ballistics: tally every field before pool rounding (#258)` removes the HP gate, optional tally
  path and gate-only counters; every nonzero field preserves its receiver contributions.
- `ballistics: prove field units with controlled fixtures (#258)` removes shipped-unit inequality
  as a pass condition and gives each test its own invariant documentation.
- `docs: state the resolved exchange and receiver contract (#258)` replaces stale full-loss,
  source-immunity and quantum-gate prose in the design narrative and glossary.

The carried reviews were checked against source. MEASURED regressions: mixed overlap delivered
2116.354 N s to a hull whose own line carried 1058.177 N s; a fixture assembly of 40.8 kg retained
that mass after perforation despite its 10.2 kg core. The unequal-mass test now balances retained
work plus emitted debris against incoming assembly energy less outgoing core energy, head-on and
obliquely, and compares the vector transfer independently. The building whole-blow mutant and
trunk duplicate-entrance mutant each fail their nonzero `ShellMarched` consumer tests.

The hollow-body re-entry test was red with field dose zero despite positive retained line dose.
The accumulation regression was red with settles `[0, 0]`, and the restored HP-bearing receiver
was missing from the small cone's recorded contributions. Both pass after gate deletion; the
accumulation test observes `[0, 1]` for its two fixture fields of 0.0003 HP each. These are fixture
observations, not shipped balance conditions. The fixture-unit test rejects a planner reading
shipped units instead of the supplied mechanism units.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex ball-15 P2; Claude ball-15 P3 | partitioned line dose silently partitions momentum | verified; lane 1 separates momentum from HP, this lane also fixes mixed-body routing | `ballistics: resolve each crossing's work and momentum once (#258)`; `ballistics: give every crossed body its exchange once (#258)` |
| Codex ball-15 P2 | subtract-after-sum can retain negative work at a full debris share | verified and already fixed by direct retained contributions; endpoint fixtures remain green | `ballistics: resolve each crossing's work and momentum once (#258)` |
| Codex ball-15 P2; Claude ball-15 P3 | old gate test has no HP-bearing receiver | verified red with an actual receiver and two small fields | `ballistics: tally every field before pool rounding (#258)` |
| Codex ball-15 P2; Claude ball-15 P3 | shipped cone and burst units must differ to pass | verified; controlled fixture units replace asset inequality | `ballistics: prove field units with controlled fixtures (#258)` |
| Claude ball-15 P3 | drawable-picture doc is attached to the unit test | verified; each test now owns its invariant doc | `ballistics: prove field units with controlled fixtures (#258)` |
| Claude ball-15 P3 | comments say below-quantum fields are not thrown | verified; gate and counters are deleted, nonzero fields always contribute | `ballistics: tally every field before pool rounding (#258)` |
| Claude ball-15 P3 | full-loss/partition comments and section reference are stale | verified; line means retained work, returning debris is distinct, partition citation corrected | `ballistics: name the debris stand-in and its HP calibration (#258)`; `ballistics: let returning debris dose its emitting body (#258)`; `docs: state the resolved exchange and receiver contract (#258)` |
| Claude ball-15 P3 | debris-share upper bound lacks direct refusal coverage | verified and already fixed; parser tests refuse both excluded bounds and nonfinite shares | `ballistics: name the debris stand-in and its HP calibration (#258)` |
| Claude ball-15 P3 | global fixture-derived debris share is labelled as a fact | verified and already fixed in data; narrative and glossary now agree | `ballistics: name the debris stand-in and its HP calibration (#258)`; `docs: state the resolved exchange and receiver contract (#258)` |

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex exchange-1 P1; Claude exchange-1 P2 | hull gets the sum while environment gets its line again; reversed entrance loses hull transfer | verified red; each line resolves its own owner once | `ballistics: give every crossed body its exchange once (#258)` |
| Claude exchange-1 P2/P3 | carrier energy has no owner, carrier keeps flying, unequal-mass bend is untested, mass doc disagrees | verified red; first resisting contact retains carrier work and impulse; only the core bends and continues, with unequal-mass head-on and oblique budget tests | `ballistics: give every crossed body its exchange once (#258)` |
| Claude exchange-1 P2/P3 | building/trunk consumers have no nonzero line proof; entrance bend spreads into rear chunks | verified through the shipped message and explicit consumer mutants; entry impulse stays at entry | `ballistics: give every crossed body its exchange once (#258)` |
| Claude exchange-1 P3 | debris is unread while rays recompute it | verified; exit emission now sums addressed exchange work, and the duplicate loss field is deleted | `ballistics: give every crossed body its exchange once (#258)` |
| Claude exchange-1 P3 | parallel dose/exchange vectors are rejoined by address | verified; one exchange contains its dose, work allocation and momentum | `ballistics: give every crossed body its exchange once (#258)` |
| Claude exchange-1 P3 | one accumulator changes from lost to retained work | verified; retained contributions accumulate directly in a separately named field | `ballistics: give every crossed body its exchange once (#258)` |
| Claude exchange-1 P3 | overmatch comment falsely says no bend | verified against `begin`; comment now states overmatch cancels slope cost while normalization bends | `ballistics: give every crossed body its exchange once (#258)` |
| Claude exchange-1 P3 | ballistics-15 backlog row remains unnarrowed | verified; narrowed with the first step, closed with the re-entry contract | `ballistics: give every crossed body its exchange once (#258)`; `ballistics: let returning debris dose its emitting body (#258)` |
| Claude exchange-1 P3 | constant-speed bend can transfer momentum outward; bounce has no exchange on unowned matter | verified by source; explicitly deferred by the owner to ricochet rebuild | not fixed: backlog `ballistics-23`, `ballistics-24` |

Decisions: stop the carrier at first resisting contact; keep the core penetration law; route
momentum by crossed volume; retain the current debris-production `StandIn` and HP calibration;
permit genuine source re-entry; tally every nonzero field before pool rounding; use controlled
units at the planner's existing assembly seam. Protocol revision advances from 69 to 70 because
retained work and recorded field-hit meaning change. No momentum field is added to the wire.
The two ricochet findings remain the owner's decision; no ricochet alternative is added here.

Final verification (MEASURED): 2,136 library tests passed, zero failed, 27 ignored; all 44
whole-tree citation, UI-font, network-boundary and bake guards passed. Formatting and CI clippy
passed before every green commit. All runs used the machine lock, unattended headless settings,
and the required feature set; no captures, windows, pushes or issue comments were made.

### Carrier continuation review (#258)

Subject: the game's damage model; game state only. Crew are HP pools at stations.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude and Codex exchange-2 P1 | later crossings and a chained bounce read the unstripped assembly | verified red on two spaced plates of one body; whole-chain budgets close on one body and two, head-on and obliquely, and the chained bounce reads core mass | `ballistics: advance the flying body at each resolved crossing (#258)` |
| Codex exchange-2 P1; Claude P2 | carrier work reaches rear chunks | verified red through the shipped message reader and allocator: the fixture front chunk retained 3.5 J instead of 6 J | `ballistics: retain carrier work at the entrance chunk (#258)` |
| Claude exchange-2 P2 | first-contact marks read the core as the flying calibre | verified red through a real first-contact event: MEASURED 35.7 mm instead of the fixture's 88 mm flying body | `ballistics: preserve the flying body in first-contact marks (#258)` |
| Claude and Codex exchange-2 P2 | a replica's exit leg uses assembly drag | verified red: MEASURED fixture path drag 0.001 rather than the authority's 0.0004444445 per metre; replica and authority now coincide after perforation | `ballistics: publish the exit body's drag with its path (#258)` |
| Claude exchange-2 P3 | scenario field doses and hits are stale | verified red by comparing doses, complete fields and bursts in every scenario; s4, s7, s10 and s16 required regeneration | `ballistics: refresh exchange records and review guards (#258)` |
| Claude exchange-2 P3 | a blank line splits the backlog table | verified by a source check; all ballistics rows now share the table | `ballistics: refresh exchange records and review guards (#258)` |
| Claude exchange-2 P3 | the Spall cone entry lost its avoid terms | verified by a source check; the settled vocabulary line is restored | `ballistics: refresh exchange records and review guards (#258)` |
| Claude exchange-2 P3 | `SpallSource::carried` only forwards its field | verified by source; callers read `emitted` directly | `ballistics: refresh exchange records and review guards (#258)` |

MEASURED whole-chain fixture budget, two spaced bodies, head-on: incoming assembly energy less
outgoing core energy is 12,551,363.091411 J; retained work is 12,386,031.25 J and emitted debris
165,331.195312 J. The 0.646099 J residual is f32 rounding. Delivered and independently expected
momentum both read `(0, -29421.637, 0)` N s. The same-body arrangement reads the same budget.
Obliquely, the two-body loss is 12,644,696.330198 J against 12,473,748.625 J retained and
170,947.695312 J emitted; delivered momentum is `(11419.842, -27484.455, 0)` N s against
`(11419.883, -27484.445, 0)` N s independently expected from the entry and final exit.
These are generic fixture observations, not authored balance targets.

Decisions: `FlyingBody` owns flying mass, calibre and drag. Discovery advances it at each resolved
crossing; playback installs the frozen result. The penetration mechanism remains the bound core.
Each crossing also retains its incoming body for the contact event. The mark's contact footprint
still follows the verdict: core for a hole, flying body for a stop or bounce. Public paths publish
the outgoing body's drag for their exit leg. Protocol revision 71 names that meaning; revision 72
names corrected contact marks and checked packed directions, with the wire-definition pin updated.

The addressed exchange carries entrance-only retained work through `DeliveredLine`; the building
queue keeps that line directly. The entrance chunk takes this work as a point load, while core
work follows chord. Total retained energy and HP conversion keep their existing owners. The
head-on and oblique allocator fixtures replace the narrower momentum-only test and retain its
impulse assertions. The whole-chain budget extends the former single-plate test.

Touching the mark test also closes ratchet rows `view-02` and `view-06`: every positive calibre
uses the existing bounded FIFO, and a checked `PackedDirection` refuses zero at deserialization
while preserving valid directions' three snorm8 bytes. The calibre-suppression test is replaced
by the all-calibre contract; the sub-calibre test now consumes a real first-contact `ShellStrike`.
The scenario guard compares complete doses, field reaches and hits, and burst fields, so a stable
interaction sequence or field count cannot hide stale contents. Regeneration changes only the
four records the guard identified. The forwarder, split table and missing vocabulary line are
removed or repaired. `destruction-01`, `destruction-03`, `ballistics-23` and `ballistics-24` remain
with their expressly assigned lanes; the ricochet law and debris-production stand-in stay as they
were. No owner decision is introduced by this review.
