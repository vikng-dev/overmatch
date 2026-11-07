# Construction destruction: model v2

Subject: Overmatch’s environment destruction and tank damage model, game state only.
Crew and modules are HP pools; dose can leave them reduced or their HP pool emptied and knocked out.

Design for [#189](https://github.com/vikng-dev/overmatch/issues/189), building on [#188](https://github.com/vikng-dev/overmatch/issues/188); the collision model is [#232](https://github.com/vikng-dev/overmatch/issues/232)'s.
This document owns the environment model.
[The tank damage design](armor-penetration-and-damage.md) owns the shared penetration, spall and burst narratives; amendments below state only what changes.

## Evidence and status

**PROPOSED** means the implementation choice recommended here.
**OWNER DECISION** marks a change to an accepted decision or an unresolved product choice.
Proceed with the recommendations when prototyping; they are not recorded as owner acceptance.

Every physical value below is **MEASURED**, **CLAIMED**, **DERIVED** or **ASSUMED**.
CLAIMED handbook values and source attributions are recalled knowledge or claims present in the supplied code, without external verification.
A MEASURED value names the count or the run it comes from.
Equation coefficients, unit conversions, indices, issue numbers and version numbers are mathematical identities or identifiers.

The supplied inputs do not contain a verified contact-charge breach example or an AP crater-size dataset.
Those empirical acceptance cases remain evidence gates; analytical predictions must not be presented as measurements.

## Principles

**Use the cheapest model whose differences cannot affect play.**
Bake convex blocks instead of simulating individual bricks.
Resolve support capacity and finite-mass rubble because rounds and tanks expose those differences.
Draw holes, because sight exposes them; rounds, mass and strength ignore them.
A stand-in becomes acceptable through bounded error against those observations, not through resemblance in a capture.

**Keep material values inside their physical ranges.**
A value outside its handbook range identifies a missing mechanism.
Name confinement, porosity, strain rate, attachment, grain or load-path error before changing the value.
A resistance factor is not permission to rescale density.

**Count energy and momentum separately, each once.**
Energy constrains work and motion.
Vector momentum and angular momentum constrain recoil and launch.
Neither determines the other without a response model.
Supports and surrounding air participate in the accounts.

## Structure and interface

Retain the offline bake, stable chunk identities, joint graph, authoritative ledger and server-owned carriers established by #188.

Use one deep destruction module with this interface:

```text
advance(layout, state, timestamped_loads, interval)
    -> state_delta, geometry_edits, body_transfers, receiver_deposits, audit
```

Occupied hull sweeps commit geometry and body transfers before contact solving (see Bodies in contact).
Other sources enter as loads; a hull a cut holds presses through Avian's contacts like any body.
The interface requires SI units, stable identities, event ordering and an explicit source budget.
Its implementation owns local response, structural response, support assessment and representation changes.
Callers do not decide which chunks fail.

Build authoritative state synchronously from baked data at spawn.
Lazy allocation may expand that state; loaded visual assets never initialize it.

An intact element remains one collision proxy until its first relevant load.
Resolve that load against the underlying geometry before atomically replacing the proxy.
A subsequent event sees the replacement geometry; holes stay out of it (see Local response and holes).

Extend #188’s `Swap`, `Dust`, `Crack` and `Fall` record with:

- Holes: negative volumes in a per-chunk list, each identified within its baked chunk.
- Partial cohesive failure and permanent compaction where they affect the view or collision geometry.
- Carrier membership changes, splits and settlement transitions.
- A topology revision and authoritative event time.

`Dust` remains a terminal representation change, not the only representation of partial removal.
A topology transaction publishes geometry, membership and the initial carrier pose together.
Clients reconstruct geometry from the ledger and interpolate authoritative poses on the shared presentation timeline.
Clients never repeat the structural calculation.

## Material and attachment data

Density, strength and toughness remain the author-facing summary.
They are insufficient as the complete constitutive input.

| Required datum | Meaning and initial provenance |
|---|---|
| Bulk density `ρ` | Mass per occupied volume; **CLAIMED brick 1845 kg/m³**, Engineering ToolBox’s London-stock entry, as cited by the supplied registry. |
| Local compressive strength `f_c` | Resistance of the material a round crosses; **CLAIMED brick-unit mean 36.5 MPa**, BIA Technical Note 3A, Table 1. |
| Assembly compressive strength | Bearing and crushing of effective masonry; must be sourced independently of unit strength. |
| Tensile strength `f_t` | Opening initiation; **CLAIMED masonry-interface order 0.1–0.4 MPa**, Lourenço’s masonry interface literature. |
| Shear cohesion `c` | Sliding initiation without precompression; **CLAIMED order 0.1–0.4 MPa**, the same literature. |
| Friction `μ` | Sliding resistance under compression; **CLAIMED order 0.6–0.8** for masonry interfaces, the same literature. |
| Elastic modulus `E`, Poisson ratio `ν` | Compliance, stress transmission and response time; neither follows uniquely from strength. |
| Opening energy `G_I`, sliding energy `G_II` | Cohesive work per newly separated area; **CLAIMED `G_I` order 5–20 J/m²**, van der Pluijm’s tests as discussed by Lourenço. |
| Compaction curve `p_c(ε_v)` | Pressure against irreversible volumetric strain, including densification; a strength alone supplies no crushing work. |
| Grain frame and directional properties | Required before timber shares the structural model. |
| Attachment type | Bonded, bearing-only or mechanical attachment; geometric contact does not imply adhesion. |

The supplied **CLAIMED 0.69 MPa** value is a BIA flexural result.
It is not automatically a direct tensile strength.

The registry’s `toughness` is `K_IC`, in Pa·√m.
Connection toughness is `G_I`, in J/m².
Where linear-elastic fracture mechanics applies, `G_I = K_IC²/E'`, with the appropriate stress-state modulus.
A brick-unit `K_IC` does not determine a masonry interface’s opening energy.

**OWNER DECISION — expand the material model.**
Recommend optional, sourced mechanical profiles containing the additional properties above.
Keep mortar implicit in effective masonry.
Keep attachment properties separate from bulk material properties.
Never derive every strength and fracture mode from one scalar.

The bake supplies contact polygons, area moments, depth subdivisions and attachment classification.
Mixed-material connections use an explicitly authored, symmetric attachment profile.
Reversing chunk order must leave the connection law unchanged.

## Loads

### Kinetic rounds

Retain the march and sampled-disc geometry described in tank-design sections **3** and **13**.

Use separate strength and inertia terms:

```text
F = A [R + k v_rel²]
k = B N ρ
m v_rel dv_rel/dx = -F
```

`A` and `N` come from projectile geometry.
`ρ` is target bulk density.
`R` is the applicable cavity or crushing resistance, including confinement where the chosen law requires it.

For a homogeneous, stationary target with constant coefficients:

```text
v_out² = (v_in² + R/k) exp(-2 A k L/m) - R/k
ΔE = m (v_in² - v_out²)/2
Δp = m (v_in - v_out)
```

Stop at the first zero of the speed solution.
For moving matter, resolve relative motion and transform the complete energy–momentum transfer back to the world frame.

**Inputs.**
Use the cavity-expansion inputs referenced by tank-design section **3**.
**ASSUMED `B = 1`** is the existing inertia approximation; its validity must be checked separately from material strength.
The compressible elastic–plastic cavity formula is a metal prior, not a validated porous-masonry law.

**Stand-in.**
Use the analytical advance where a rigid projectile and homogeneous material adequately describe the crossing.
Use a local finite-thickness response near free faces, major voids and developing perforations.
Accept the analytical adapter only where residual speed and resulting geometry agree with that response.

**Server cost.**
**DERIVED `O(SH)`**, from `S` disc samples and `H` encountered homogeneous spans, excluding local refinement.

**Tanks.**
This replaces fixed-factor scaling of both reference terms for every substance.
Reference steel retains its own density and strength terms.
Other metals and homogenized modules need independent inertia inputs; unsupported entries must not silently inherit steel density.
Reference-mm remains a reference-steel readout, not an additive currency for arbitrary materials.

### Standoff blast

Use **CLAIMED Hopkinson–Cranz scaling** and **CLAIMED Kingery–Bulmash/UFC pressure–time data**:

```text
Z = R / W^(1/3)
p_s = P(Z)
t_a = W^(1/3) T_a(Z)
t_+ = W^(1/3) T_+(Z)
i_s = W^(1/3) I(Z)
```

`W` is the applicable explosive-equivalent mass for the selected source model.
Pressure and impulse equivalence need not be identical.
Store each table’s units, charge geometry and validity interval.

Represent the positive phase by a pulse matching both peak pressure and impulse:

```text
p(τ) = p_peak (1 - τ/t_+) exp(-b τ/t_+)
∫ p(τ) dτ = i
```

Solve `b` from the selected impulse.
Retain the negative phase when it changes rocking or an acceptance result.

For normal reflection in the ideal-air approximation:

```text
p_r = 2 p_s (7 p_0 + 4 p_s) / (7 p_0 + p_s)
```

This expression assumes **CLAIMED air `γ = 1.4`**, standard ideal-gas shock relations.
Use oblique-reflection data for incidence away from normal.

Apply pressure traction normal to the actual exposed surface:

```text
F(t) = ∫face Δp(x,t) n(x) dA
M(t) = ∫face (x - c) × Δp(x,t)n(x) dA
J = ∫ F dt
W_wall = ∫∫face Δp u_surface·n dA dt
```

Clearing begins when relief from an exposed edge reaches the loaded point.
Its timescale is `t_clear ≈ edge_path/c_relief`.
Use UFC finite-face clearing histories; do not maintain infinite-wall reflected pressure after relief arrives.

**Inputs.**
Use **CLAIMED ambient pressure 101325 Pa**, the standard-atmosphere reference.
Use the source tables’ atmospheric and explosive conditions.
Charge position, face orientation, visibility and newly opened holes determine exposure.

**Stand-in.**
Adaptive face quadrature supplies pressure, resultant force and moment.
A prescribed pressure history is acceptable while wall motion weakly perturbs the blast.
A large deformation, venting event or significant gas-work demand selects the coupled near-field response.

**Server cost.**
**DERIVED `O(QVK)`**, from `Q` face samples, visibility-query cost `V` and `K` pulse intervals.

**Tanks.**
The same pressure loads exterior tank geometry and produces physical push.
Pressure does not become an HP dose through a chemical-energy interception fraction.
Any future pressure response of internal HP pools needs its own receiver law.

Room buildup and repeated diffraction are outside the direct-field adapter’s validity.
Such cases remain unsupported until a transport model supplies their pressure histories.

**Built** (`fracture::Blast::push`, `Blast::peak`): pressure and impulse carried by the blast field's 4096 rays (Resolution).
- `W` is the TNT that carries the blast's energy, `E / 4.184 MJ/kg` (**ASSUMED**: impulse equivalence is energy equivalence).
- `i_r(Z)` and `i_s(Z)` are Swisdak 1994's metric fits of the Kingery–Bulmash surface-burst impulses (**SOURCED**: DTIC ADA526744, Table 1, PDF pp. 9–10, read off the page images; the fits restate Kingery–Bulmash with a mean error under 1 %, worst 1.5 %, Table 5); a burst in air reads them at `W / 1.8` (**CLAIMED** UFC surface-burst reflection factor).
- Oblique: `i(θ) = i_r cos²θ + i_s (1 + cos²θ − 2 cos θ)` (**CLAIMED** ConWep blend, stated for pressure, read on impulse: **ASSUMED**).
- A ray stopping on a chunk face pushes it along the face's inward normal with `i(Z, θ)` over the patch it stands for, `4π share d² / cos θ`, `cos θ` floored at 0.1.
- A ray stands for a cone of section `4π share d²`; the cones that stop on one chunk or body cover at most its own silhouette across them (a chunk's from its faces, a body collider's from its bounding box), so each keeps `min(1, silhouette / Σ section)` of its energy and push (`fracture::intercepted`). A far ray then pushes the face it lands on with `i × A_face`, not with its patch: without this, one ray landing on a house 535 m from a 5 kg charge pushed one chunk with 1797 N·s (**MEASURED**, the shipped house at unit scale).
- Every ray stops at `Z = 40 m/kg^⅓`, the reflected fit's edge (`Blast::reach`), so no law reads the fits past their range. Within it a ray stops on the nearest of every construction's standing chunks and unbreakable elements, every dynamic body's collision proxy (Avian colliders on the `Vehicle` and `Debris` layers: a loose piece, a hull, a launched turret; never its armour or witness volumes) and every other surface on the `Terrain` layer (the ground). A ray that stops on a moving body pushes it the same way, at the point it stops, through Avian's impulse, which wakes a sleeping body; it deposits nothing. A ray on any other surface does nothing. A collider holding the burst point shades nothing and takes nothing: a burst inside a body has no law here.
- The peak overpressure rides beside the impulse (`Blast::peak_pressure`, `Blast::peak`): the same ray, patch, interception and ConWep blend give each blast load a peak force `F_peak` (N), its rays' summed (**ASSUMED**: one chunk's rays arrive together). The section and rest laws read it (Law 3 below).
  - `p_s(Z)`: Swisdak 1994's incident rows, Z 0.2–198.5 (**SOURCED**: ADA526744, Table 1, PDF p. 8); its branches meet within 0.7 % (**DERIVED**).
  - `p_r(Z)`: Swisdak's two reflected rows, Z 0.06–2 and 2–40 (**SOURCED** likewise); they meet within 0.1 % at Z = 2, and the reflected peak stays at least 2.01 times the incident over the field (**DERIVED**). At Z = 10 they read 31.54 and 14.89 kPa (**SOURCED**: Table 3, PDF p. 12).
  - A far reflected row recalled with two wrong coefficients (9.2 kPa at Z = 10, under the incident) once stood in the #237 study; the printed row is the one built.
  - **DERIVED** from these: a shell's own filler sphere touching a face sits at Z ≈ 0.064 m/kg^⅓, inside the fits, where `p_r` ≈ 0.78 GPa; the detonation products' own pressure is 10–20 GPa, so the fits under-read the last decade before contact (the near-contact floor below).
- Not built: the pulse shape, clearing, the negative phase, wall motion's feedback, and `Z` below 0.06 m/kg^⅓ (held at that edge).
- It replaced the detonation products' momentum `√(2 C E)` shared over the rays: for the 88 mm HE fixture burst (1.47 MJ) on a 0.25 m brick cube square on, **DERIVED** launch speed 0.28 → 0.55 m/s at 1 m and 0.031 → 0.15 m/s at 3 m.

**Owner decision (2026-11-01, [#237](https://github.com/vikng-dev/overmatch/issues/237)): a blast on a structure is one field and three material laws.**
The law is stated on the field, whatever finds the surfaces it reaches ([#204](https://github.com/vikng-dev/overmatch/issues/204)).
- **The field.** On each face point it reaches, a peak pressure `P` and a specific impulse `i`, from the fits above at `Z = R / W^⅓`, blended by incidence.
- **Law 1, crush.** Material crushes where `P` beats its dynamic compressive strength `σ_c,d`; the impulse decides how much, by momentum through a compaction front: `Δm = J f(σ_c,d / P) / u_c`, `u_c = √(σ_c,d ε_v / ρ)`, `ε_v` the volumetric strain to lock-up. `f` (`fracture::crush_factor`) is the snowplow front's factor against a triangular pulse: `√(1 − 4r/3)` below ½, `√((16/3) r (1 − r)³)` to 1, zero past it. It replaces the borrowed cavity-strength crush pool.
  - **Built** (lane 4): each blast ray crushes the chunk it stops on by Law 1 at its patch's peak overpressure and the push's impulse, scaled by the share of its cone the chunk intercepts (`fracture::Blast::crushed`, `Material::crushed_by`); a chunk's load sums its rays' volumes. `σ_c,d` is the row's quasi-static compressive strength times its stated DIF, read as the row's reference-rate datum (**ASSUMED**: no strain-rate law; brick's datum is at 100 /s, wood's at 2400 /s). `ε_v = 1 − ρ / ρ_s` (**DERIVED**: the locking idealization compacts the pores shut): brick 0.18, wood 0.70; `u_c` brick 91.6 m/s, wood 229 m/s (**DERIVED** from the shipped rows; wood's `σ_c,d` is 33.4 MPa against the study's recalled 3.8 MPa, so timber crushes far less than the study's table shows). The reference rate records the source measurement; it does not supply a per-pulse rate law or separate intrinsic rate effects from the compaction front's inertia. That applicability remains a validation gap.
  - **The pool is a volume (m³).** A chunk's pool is its volume; every load wears it by the volume it crushes (`fracture::Load::crushed`). A round's line and a fragment crush their energy over the cavity resistance (`Material::crush`), as before; a blast crushes by Law 1 alone, its energy never entering the pool. A blast's crater dish holds its load's crushed volume. Blast loads carry zero kinetic cavity energy and never source the Lamé crack field. Intercepted chemical-energy shares remain diagnostics in the wall sandbox alone; they drive no removal, hole size, load point or joint stress. Its loads are at each face's impulse-weighted centroid, preserving angular impulse; opposing faces remain separate.
  - **Representation.** Law 1 is linear in the impulse at a fixed pressure, so a face's crushed volume is its patches' sum however it is cut (`fracture::tests::crushed_mass_is_representation_independent`). **MEASURED** on fixture bursts near a 2 m brick face (the 88 mm HE burst at 0.1 m, a bare 5 kg at 0.3 m): 1024 to 65536 rays, one block or 8 × 8 chunks, agree within 3.17e-4 (maximum relative spread 3.160834e-4, printed at full reporting precision). This certifies those fixture casts only. It does not certify larger charges, other material rows or a far face sparsely covered by rays (#204).
  - **The source and its radius.** The environment airblast starts at the filler centre (`Burst::filler_center`), the tip minus the flight axis times the burst block's required, sourced `filler_setback`. The shipped setbacks are **DERIVED, with ASSUMED axial geometry**: a cylinder-equivalent nose plus half the cavity, rounded in the round data; no verified axial drawing is claimed. The charge radius is the equal-volume sphere from filler mass and density, independently of the casing/blast energy split. `Blast::distance` floors every field distance and cone section at that positive radius, including a geometric stop at the source, while the actual face point stays unchanged. The fuze still times and records the tip. A contact burst therefore has a nonzero patch and crushes wherever its pressure beats the material threshold.
- **Law 2, back scab.** The front face's compression reaches the free back face, reflects as tension, and lifts a scab where `σ_b min(1, 2 ligament / λ) ≥ σ_t,d`, `λ = c τ` the pulse's length, `τ = 2i / P`; it leaves at `(2σ_b − σ_t,d) / ρc`, its momentum taken off the push.
  - **Built** (lane 5, `fracture::scab`, run by `Load::blast` on every loaded (chunk, face)): the back point lies the full hull depth `H` behind the face's impulse-weighted centroid along its inward normal, at the convex hull's actual exit. The ligament `L = H - crater_depth` is the intact material left behind Law 1's local front; through-crush leaves no ligament and emits no rear scab. **ASSUMED:** the ray patch nearest the face centroid supplies the crater depth at that one back point. The patch's depth holds Law 1's volume in the cone expanding behind its entry (`fracture::Crushing`); at normal incidence `d = (R³ + 3 R² V/A)^(1/3) - R`, **DERIVED**, with oblique depth projected onto the face normal using the field's incidence floor. This depth is intensive, unchanged by patch share or silhouette interception. `σ_b` is the solid-angle mean of the face's transmitted stress, the sum over its ray patches of `H / (2π R³) · min(P, σ_c,d) A` (a uniform load over a plane gives back the load). The elastic Boussinesq weighting `3 L³ / (2π R⁵)` was the rejected alternative: it reads three times this on a concentrated load's axis and threw scabs two to three times the study's speeds. `τ = 2J / F` from the face's blow and peak force; `c = √(E / ρ)` (**DERIVED** from the row). The scab is `δ = λ σ_t,d / (2σ_b)` thick (where the net tension first reaches the strength, at most `L`) over the area its transmitted force covers at `σ_b`, `F_t / σ_b`, bounded by the chunk's exposed rear area projected along the normal; contacts between standing matter subtract their projected area on the rear plane: every seam, cracked or not (a crack opens its tension, never its contact), and every rest not yet opened, ground anchors included. No free-surface reflection is created inside a contact. **Gap:** the pulse is not propagated through that contact to a farther free surface. **DERIVED**: it then carries at most `J F_t / F` of the face's momentum. `σ_t,d` is the row's `bulk.tensile` times its sourced `blast.tensile_dif` (brick 0.69 MPa × 3.1 = 2.14 MPa, the DIF **CLAIMED**; wood's ratio 1, **ASSUMED**, the missing truth), refused at load when absent.
  - **The scab's carrier.** Its momentum comes off the face's blow, so the pushed chunk and the scab share the face's momentum (`fracture::tests::a_scab_takes_its_momentum_off_its_faces_push`). Its volume leaves the chunk's pool as one more load at the back point with no blow, and that load's dish marks the back face (the existing crush-removal path). **Gap:** no thrown body or debris carries the scab's mass and velocity away; the momentum leaves the simulation with it. A moving carrier waits on a debris kind for pieces smaller than a chunk.
  - **ASSUMED, named:** the gate reads one back point per face, with the nearest sampled patch supplying its crater depth. **MEASURED** on fixture brick slabs, the 88 mm HE fixture burst 0.178 m off the face (4096 rays): 0.12 m throws 5.76 kg at 14.29 m/s (82.2 of the face's 589.7 N·s), 0.25 m 27.9 kg at 6.19 m/s (173.0 N·s), 0.5 m 202 kg at 1.96 m/s (397.1 N·s). The speeds agree with the study's **DERIVED** 14.2, 6.2 and 2.0 m/s. A thick wall's slow scab takes most of the face's momentum (at the gate's threshold, `δ = λ/2`, it would take all of it), so the wall it leaves is pushed less. The footprint `F_t / σ_b` is the back stress's equivalent area (**ASSUMED**): it reaches the 0.5 m slab's whole 4 m² back, where the stress off the axis falls under the gate; a footprint read from where the gate passes is the refinement if thick-wall scabs look too heavy in play.
- **Law 3, push.** The impulse the face keeps is the load's blow, `F_peak` its peak force; a section fails when `J² g(F_R / F_peak) / 2m ≥ W_section`, `g` (`fracture::plastic_demand`) the rigid-plastic demand of the same pulse: `1 − 4r/3` below ½, `(16/3) r (1 − r)³` to 1, zero past it. Its ends are the two limits below: `g → 1` impulsive, `g = 0` where the peak force cannot beat the strength.
  - **Built** (lane 2): every energy limit a blow meets reads a blast's blow by `√g`. Its `r` is the share of the group's peak forces at which that failure mode's strength law, with gravity and the presses, first fails (`fracture::strength_ratio`, by bisection: each law holds on a set of loadings convex in that share). A bonded section's turn reads its seam's Navier tension (`fracture::section_pulse`); a seated part's turn reads tipping, its lift lifting, its shove sliding. A load stating no peak (a round's line, a body's contact) keeps `g = 1`; one group's sources add their angular impulses each so weighted (**ASSUMED**: a line's blow and a pulse act in phase). A group lying on both sides of a bonded section's plane has no Navier reading and keeps `g = 1`.
- **Energy is audited, never an input.** The round's chemistry splits once, casing against blast (`burst.rs`); the blast-energy-to-wear and blast-to-Lamé paths are absent. Kinetic lines and fragments retain their energy-to-volume and cavity-stress laws; blast removal, scab and section loading read the pressure/impulse laws alone.
- **Material data.** `σ_c,d` with its rate factor, `ε_v`, the stiffness and `σ_t,d`, sourced per breakable material; a breakable row lacking them is refused at load.
  - **Built:** `BulkFacts::blast` stores compression, its reference-rate DIF, fully dense grain/cell-wall density and Young's modulus. Every breakable bulk row must state the complete sourced block at load. Law 1 reads the compression, its DIF and the fully dense density; Law 2 reads the modulus and the tensile DIF, which raises the row's `bulk.tensile`. Source domains and analogue limits are in `.agents/scratch/blast-b3-material-sources.md`.
- **Near contact.** A products floor inside `Z < 0.6` corrects the fits' ~1.7× under-read of contact impulse (**DERIVED** in the study).
  - **Built** (lane 8, `fracture::Blast::products_floor`): the detonation products' scalar momentum `p = (2/3) C v` (**DERIVED** from the burst law's own Gurney profile, `burst::products_momentum`; `v` the casing's `v_s`, or `K √2` for a bare core) spreads over the sphere and reflects off the face (factor 2, **ASSUMED**, the study's): `i_p = p / (2π R²)`. The normally reflected impulse is `max(fit, i_p)`; where the products win, the reflected peak takes the same factor, keeping the fit's pulse duration `2i/P` (**ASSUMED**). The ConWep incidence blend then applies as before. The branch lies only above the fit and only inside its crossing, and the field outside is the fit bit for bit (`fracture::tests::outside_its_crossing_the_field_is_the_fit_bit_for_bit`). It reads only data the burst block already requires (core mass, `speed_constant_mps`, the casing), refused at load where the product is not positive. **MEASURED** by the tests on a bare fixture core (`K` 2400 m/s): the crossing sits at `Z = 0.594` m/kg^⅓ for 0.5, 5 and 50 kg alike, the floor 1.63× the fit at the charge's surface. **DERIVED** for the shipped data: bare cores 1.63–1.82× and `Z ≈ 0.59–0.72`, against the study's ~1.7× and ~0.6; the cased 8.8 cm HE round 1.44× at its filler's surface, meeting the fit at `Z ≈ 0.44` (≈ 0.25 m).
  - **Not built:** the brief's literal clamp of the peak at the detonation-product pressure (`ρD²/4`). A constant clamp never meets the fit (the fit's highest reflected peak, ~0.78 GPa at its edge, lies far under any product pressure), so it cannot join continuously; any continuous product-pressure branch needs a decay law no datum gives; and a peak floor alone leaves the contact impulse, which Laws 1 and 3 read, at the fit's under-read. The study's 1.7× and `Z ≈ 0.6` are the impulse branch's numbers, reproduced above.
  - The Law 2 slab fixture numbers above were measured on the free-air fit alone (their test charge's products are an **ASSUMED** fit-only fixture); the shipped round's floor raises its field at that 0.178 m standoff.
- **Fragments.** The environment adapter normalizes the recorded angular profile to the casing's physical kinetic budget, independent of the HP field's potency multiplier (`breakage::RayDeposit::casing`). Each ray delivers its ejection momentum as a blow, **DERIVED** `p = 2E/v_s`; their summed scalar ejection momentum is `M v_s`. Each stop retains its own Load so nonparallel pushes preserve angular impulse. This carries ejection momentum, not carried flight energy; the existing ring remains an angular sampling approximation. The chemistry split is audited at f32 storage precision (`burst::tests::each_joule_of_chemistry_is_counted_once`), and inconsistent known chemistry is refused at load. Law 1's work is audited as `sigma_c,d epsilon_v volume = rho u_c^2 volume`, never fed back as an input or energy cap.
- **Validation.** The repo holds no verified blast breach case (**code fact**, "Evidence and status"): the laws are gated on class-invariant tests and the prototype's measured qualitative matrix.
- **Lanes**, in order: 1, the peak-pressure fits and each load's peak force (built); 2, `g` in the section law (built); 3, the material data (compression/compaction-density/stiffness block built); 4, Law 1 and the volume pool (built); 5, Law 2 (built); 6, the field's origin at the filler centre and fragment momentum (built); 7, the dead energy paths (deleted); 8, the near-contact products floor (built); then pricing.

### Contact and near-contact blast

Never extrapolate free-air tables to a charge touching a wall.
Resolve detonation-product expansion, venting and target response in a local coupled patch.

Use compressible conservation laws with product and air equations of state:

```text
∂t ρ + ∇·(ρu) = 0
∂t(ρu) + ∇·(ρu⊗u + pI) = 0
∂t E + ∇·[(E+p)u] = 0
```

A product equation of state may use **CLAIMED Jones–Wilkins–Lee form**, Dobratz and Crawford’s LLNL explosives handbook:

```text
p = A_J(1 - ω/(R_1 V)) exp(-R_1 V)
  + B_J(1 - ω/(R_2 V)) exp(-R_2 V)
  + ω E_v/V
```

`V` is volume relative to the reference state.
`E_v` is internal energy per reference volume.
All coefficients must come from one internally consistent explosive dataset.

**Inputs.**
Required: charge shape, density, detonation state, product equation of state, confinement and casing.
**CLAIMED TNT-equivalent energy 4.184 MJ/kg**, NIST’s unit convention, is not a substitute for those explosive properties.

**Stand-in.**
Prototype with a coupled local reference calculation.
Replace it with a response atlas only after interpolation preserves breach topology, force, moment, ejecta and budgets over its certified domain.
No scalar contact multiplier is selected.

**Server cost.**
**DERIVED reference cost `O(N_c N_t)`**, from local cells and stable time steps.
**DERIVED atlas cost `O(Q)`**, from evaluated surface samples, plus local geometry changes.
Runtime viability remains an explicit measurement gate.

**Tanks.**
The same adapter can load a thin metal plate.
Plate rupture follows its material and geometry, without an authored HE penetration number.

### Fragments

A fragment carries material, mass, shape, position and velocity:

```text
E_f = m_f |v_f|²/2
p_f = m_f v_f
```

Its encounters use the kinetic law.
A bundle represents a stated number of physical fragments; it is not one oversized projectile.

For casing fragments, retain the source geometry and speed derivation referenced by tank-design section **5a**, but normalize the mass and velocity distribution to its actual budget.
Include carried translational motion, the nose, base and product momentum.

**Inputs.**
Casing mass, explosive properties and fragment mass/shape distribution require sourced data.
A Gurney speed does not determine fragment size.
Use the local fracture response or a validated casing distribution; do not let sampling resolution choose physical fragment mass.

**Stand-in.**
Bundle fragments only while interception probability, penetration and deposited energy converge for the smallest gameplay-relevant target.
Finite flight time is authoritative.
Neglect air drag only inside a certified distance over which its energy error is negligible.

**Server cost.**
**DERIVED `O(FH)`**, from fragment bundles and their encountered spans.

**Tanks.**
Remove the burst potency multiplier from physical transport.
Fragment dose follows actual absorbed energy and the shared receiver conversion.
Existing ring geometry may remain an angular sampling stand-in if it passes the interception checks.

### Bodies in contact

Subject: the game's environment destruction, game state only. A moving body's occupied hull
makes the hole before the physics contact solver runs (`construction::sweep`). Structural
collapse remains the part graph's decision. Neither law names a vehicle or construction.

One sweep source per rigid body owns motion, inertia and drive. A query-only binary bounds tree
holds its convex contact hulls in exact body-local frames. Free groups prepare this tree from
admitted data synchronously with spawn and split; articulated bodies snapshot their current
collider frames. Each body makes one spatial query, then traverses each candidate construction's
bounds tree. Buried matter remains soil-held. The permanent colliders and ordinary Avian contacts
keep their existing jobs; no enclosing convex hull invents matter between the leaves.

At each target leaf, visit source siblings by lower entry time. Rotation-only envelopes
translate with the COM, so a slab query gives a conservative entry time. Only an inward exact
convex hit may prune later source leaves. Every occupied target joins the body's patch once;
the earliest inward witness supplies its contact response. Parry 0.27's
`CompositeShapeRef::cast_shape_nonlinear` returns the first geometric hit, which can be tangent
and hide another leaf's later inward hit; its first-hit composite query therefore cannot own this
patch. Each touched construction resolves its sources in one task, in body entity order, because
sources can compete for the same standing matter: all of them resolve on the construction's state
at the tick's start, a released plug's cut commits to its topology at once, and every later
source resolves again on the topology that cut left, with the gravity field solved on it
(`parts::Assessment::resolved_on`), so every committed plug is a minimum cut for its own demand on
the matter it meets. Constructions never meet in this sequence. Each source then exchanges once
against all its plugs; departures prepare per construction, and their effects commit in (body,
construction) order.

The patch is the forced source of a directed graph over standing law chunks. Ground endpoints
and chunks the sweep does not cover (including buried matter) are fixed sinks. Live, unseparated
joints supply directed capacities for rigid translation along the contact direction `d`. With
`n` pointing toward the moving side, let `o = n dot d` and `s = |d cross n|`.
Both opening and compression measure the force resisting that translation:
`C = R_n |o| + R_s s`. For compression, `R_n = max(B + p, 0)`:
weaker-side bulk capacity `B = min(sigma_c,a, sigma_c,b) A` minus carried compression,
plus the tension a closing load first unloads.
A ground endpoint uses its chunk's strength: the ground is one rigid surface whose settlement is
not simulated (the one-surface doctrine).
For opening, `R_n = max(A T - p, 0)`. The shared `bondsolve::JointStrength` law supplies
`R_s = A cohesion + mu max(-p, 0)` and tensile strength `T`; cracked seams and rests have
zero cohesion and tension. `p` is existing normal force, positive in tension.
DERIVED: for unit rigid translation, resistance equals the sum of independently limited
normal and tangential traction work (Yastrebov, *Numerical Methods in Contact Mechanics*, 2013,
[Eq. 4.53 and normal/tangential split Eq. 4.56](https://www.yastrebov.fr/LECTURES/Yastrebov_NMCM_Wiley_ISTE.pdf)).
Both branches tend continuously to `R_s` at grazing. A purely normal translation has no sliding
term. `sigma_c` is the data's quasi-static `bulk.blast.compressive_strength`, carried by
`fracture::Material::compressive_strength`, not cavity work or the dynamic compaction plateau.
The standing construction's gravity field supplies every joint's preload before optimization:
part-law interface forces between parts, and `parts::Kernel::hold` under gravity plus those
interface forces inside each part. The part forces are one more column of the owner's one
equilibrium: `parts::Assessment` solves gravity alone on its own factor and contact active set
whenever that factor changes, so contact loads (this body's press included) never enter the
preload. The column is the structure law's last; the state can run one stage ahead of it (one
stage per tick): a joint that cracked or opened since reads it at its current strength, and
matter that left since still loads the interfaces below it. A sweep after an earlier cut in the
same tick reads the column re-solved on that cut's topology instead.
Each owner caches the preloads until that column or its standing chunks, cracks or separated
joints change. Preloads stay fixed during optimization, including on supports beyond the seed
patch; contact demand does not manufacture one.
Intact kernels are the design's factored chunk kernels; kernels that differ from their design
refactor. One interface pass assembles all boundary
loads; kernels visit their chunks' own joint adjacency, with local hash indices. DERIVED: expected
`O(chunks + joints)` indexing/traversal work per cold field, plus chunk triangular
solves/factorisations; unchanged fields need only the linear topology signature.
The grounded-singleton operation witness counts visits and forbids intact-kernel refactorisation.

Goldberg–Tarjan push-relabel finds the minimum cut, with FIFO discharge and periodic global
reverse-residual distance updates ([1988 paper](https://doi.org/10.1145/48014.61051)).
Capacities round down and demand rounds up on a shared power-of-two force grid; push-relabel
stores residuals and excess as `u128` grid units. Resolution: a 120-bit total capacity budget
leaves integer headroom for reverse capacities and inlet supply. This chooses numerical
precision, never material strength. Let `K` count positive finite directed capacities, `M` be
their maximum, `e = floor(log2(M))`, and `l = ceil(log2(K))`. The quantum is
`q = 2^max(e + 1 + l - 120, -1074)` N; an all-zero graph uses the smallest positive f64.
DERIVED: their scaled integer sum is below `2^120`; each capacity loses less than `q`.
Power-of-two scaling is exact except subnormal underflow, which also loses less than one unit.
Every residual operation is exact integer arithmetic. One unit above the finite integer sum
replaces infinity and bounds inlet supply; no finite minimum cut can cross such an arc.

A synthetic inlet supplies the rounded demand, capped by that finite bound for larger
demands. Sink inflow reaching supply certifies holding against the original demand:
finite capacities were only lowered, and a cut below the inlet bound exists. Otherwise surplus
returns to the inlet; residual reachability selects the smallest source-side grid minimum cut
`S*`. No graph region is omitted. Its capacity is reported by summing original crossing
capacities, rather than rounded flow. DERIVED tolerance: the chosen cut exceeds the true
minimum by at most `K q` before reporting roundoff. The held/released uncertainty band is
at most `(K + 1) q`, always biased toward release; holding has no upward tolerance.
For comparison against exact decimal fixtures, including their
input conversion and the reported sum, use `T = (K + 1) q + gamma_(K+1) Sigma`, where `Sigma`
is the sum of all positive finite directed capacities, `gamma_j = j eps / (1 - j eps)`, and
`eps = f64::EPSILON`. This bounds numerical resolution relative to the graph's capacities,
not to an asset-specific force. A held cut's capacity and threshold telemetry remain certified
lower bounds. The fractional-graph reference and single-ULP deficit witnesses guard both contracts.
The plug can therefore include chunks beyond the occupied patch: compression transmits the push
through a back layer or around a corner until perimeter joints or bulk crushing give way.
Internal joints of `S*` carry no relative motion and contribute no cut capacity. A resting floor
on anchored joists holds below its remaining bulk crushing reserve; sliding or lifting still reads the
shared translation-resistance law.

The demand is the peak force of the hull's impact on the patch plus its drive:
`F = P(m_eff, v_n) + F_drive,n`, with `P` the Hertz peak force (Johnson 1985, §11.4) that
landings use too (`contact_law::peak_force`): the witness chunk's material and volume-sphere
radius on one side, the hull rigid and flat on the other (StandIn: no body data carries a hull's
stiffness). DERIVED: the first inward surface witness supplies the speed and contact mass
`m_eff = 1 / (1/m + (r cross n) dot I_world^-1 (r cross n))`, as in Avian 0.7's
`ContactNormalPart::generate` against a fixed target. The lever `r` starts at the moving centre
of mass; inertia is in world coordinates at contact time, with Avian's locked-axis response.
No physics step enters the demand. The inward drive adds to it: all Avian integrates before
its contact solve, accumulated and constant forces and gravity, on the body's free axes (a hull
its belts hold at rest on level ground drives nothing vertically). The cast reads the same
motion: DERIVED, Avian's `N` substeps carry a body no contact touches `v dt + a dt² (N + 1)/(2N)`,
which the cast's one velocity reaches at the tick's end. If `F > C`, break the minimum cut and
remove its whole plug `S*` from the standing construction. Otherwise the sweep does nothing to
the hull: **Avian owns the stop** (owner decision, 2026-11-07). Its contact solver, speculative
points included, stops the hull at matter whose cut holds as it stops a hull at any obstacle, and
the structure check reads that contact as its mean force like every other; the peak force decides
only the cut. Its reach stops at the cut's: a collision hook (`sweep_intake::SweepContacts`)
drops every look-ahead point, one whose shapes are still apart, on covered standing chunks
outside this tick's exact cast, and keeps them on cast members, so Avian still stops a hull at
matter whose cut held; it adds no hold, press or pose. Without it Avian's linearised look-ahead
braked and lifted the hull at standing matter no cut judged: MEASURED on the tracked ram
(`tracked_ram_sweep_opens_within_two_ticks_and_accounts_for_momentum`, the 57 t hull at
10.8 m/s), both ticks' cuts released, yet standing chunks outside the cast took 0.77 m/s off the
hull and lifted it 0.63 m/s at impact +1. With the hook the house moves the hull through no
unjudged matter, and the hull reads 10.82 m/s at +1 and 10.37 m/s at +2 (playtest fork F19,
decided by this measurement). The rejected alternative: the sweep stopping the hull itself (a time-of-impact pose, a stop
impulse and a press of its own), with a collision hook that dropped Avian's look-ahead points
on covered chunks so the two stops did not meet. That answered one contact with three readings
of it (the cut's `m_eff`, the press's whole mass and the body's centre-of-mass stop plus a free
couple).

DERIVED: for a fixed patch, `v_threshold` is the speed whose peak force is `max(0, C - F_drive,n)`:
`δ = (3P / (4 E* √R))^(2/3)`, `v = √(16 E* √R δ^(5/2) / (15 m_eff))`; it grows as
`(C - F_drive,n)^(5/6)`. Motion just above it breaks the cut. Strength, mass and drive come from data and
rigid-body state; no authored speed threshold or construction-specific adjustment enters the law.

All chunks freed by one source in a tick share one plastic rigid-motion exchange, including
occupied inertia and off-centre torque. DERIVED: with chunk spatial mass matrix `K` about the
source centre and source inverse response `R`, solve
`(identity + R*K) * twist_after = twist_before` in six coordinates. Each chunk leaves with
`v_after + omega_after cross r` and spin `omega_after`; the source takes the opposite total
linear and angular impulse. DERIVED dissipated energy is half the exchanged spatial impulse
dotted with the incoming twist. Locked axes route their reactions to the hosting constraint.
The account exposes removed kinetic energy and outgoing chunk kinetic energy separately.

`breakage::sweep_intake::sweep_departure` is the departure boundary: construction identity,
chunk identities, world rigid motion and authoritative tick. Internal joints stay intact. The
cut also frees matter already disconnected from every ground path, before standing refit.
Swept matter inherits the hull's plastic exchange; an unsupported remainder starts at rest.
Freed server bodies join the same tick's sweep as sources, from their prepared launch and
gravity, under the entity each spawns as; a departure that releases more matter exchanges with
it before it spawns. Rounds repeat until one frees no server body, at most `DEPARTURE_ROUNDS`
past the first (MEASURED: the tracked ram's busiest tick needs 2); past that cap the newest
departures are swept from the next tick. Before this, the freed plug, pushed by the hull, met
standing remainder in the solver that no cut judged: 84 % of the hull's momentum change at +2.
Below-floor merging (`crumble::merge_fragments`) projects their combined linear and angular
momentum onto the merged rigid body.
All freed fragments use the shared size route described under **Size split**;
resolved rubble stays on the server and meets the hull through ordinary Avian contacts. The
standing owner removes the chunks immediately; clients derive the hole from the joint ledger
at the same presentation tick. Dust-only hull departures are rejected: they remove matter from
the tank's path.
A retired element's staged belt reactions are cancelled before integration; surviving surfaces
receive fresh solver contacts. A sweep never enters the structural pulse/work calculation.
Why the cancellation: the track law stages a belt's reaction against the surface it read
before the sweep runs, and a reaction from a surface the sweep removed has no surface to come
from. Staging belts after the sweep instead would move the drive the cut reads into the next tick
and reorder the track law's schedule for one consumer. A swapped element whose matter still
stands beside the cut loses its belt reaction for that tick too: its element collider retires
for the chunk colliders reconciliation builds after the sweep, which the staged reaction cannot
name. That costs at most one tick of that belt's support, `F dt`; restaging it onto the new chunk
colliders would carry each point through reconciliation for that one tick. The drive a cut reads
keeps the belt reactions staged against its own owner: the floor's traction drives a hull into a
wall of the same house, so taking out the owner's whole staged set would drop a real press, and
the plug that would bound the subtraction to the matter cut is what the cut itself decides.

Every remaining solver transfer enters the structure check as its whole mean force `J_solver/dt`.
Physics already changed those bodies' momenta; contact intake supplies no second blow, source
mass share, delayed cap or give-back. Sleeping contact forces persist until their contact ends.

Exact sweep membership and Avian's speculative contact envelope differ. The sweep casts admitted
hulls with the motion Avian integrates before its solve; Avian constrains separated hulls
against projected contact planes, and a body another body pushes within the solve can close a
gap its own free motion opens (a freed chunk stands only `LOOSE_GAP_M` off the faces it shared).
The hook drops those points on covered non-members, so no non-member moves solver momentum; the
tracked ram asserts it every tick. Post-contact speed is the sweep's plastic exchange, contacts
with cast members and contacts with released bodies.

Round and field impulse laws retain their own fracture-work and launch accounts.

DERIVED angular bounds: under rotation about a fixed axis, each corner coordinate of a leaf's
local box is `c + a cos t + b sin t`; its extrema over the tick are the endpoints and the
stationary phases the turn reaches, whatever the turn count (`sweep::rotation_bounds`). A body's
query box merges its leaves' rotation envelopes and adds its COM travel. Exact nonlinear casts
decide occupied membership.

DERIVED sweep work: bounds-tree traversal and exact casts choose the occupied source patch;
live construction joints build the directed graph only on an occupied contact event. Generic
push-relabel has a capacity-independent `O(V² E)` work bound and `O(V + E)` storage; monotone
labels and current-arc scans retain that bound. Global relabelling follows one graph-sized amount
of discharge scans, amortising its traversal. On the grounded compression chain, routing and
returning the surplus take linear visits, rather than one whole-graph search per support; the
operation witness covers up to 32,768 chunks. Independent constructions retain parallel geometry.
`SweepAccount::work.flow` reports nodes and residual edge pairs (including the synthetic inlet,
source and anchors), global/local relabels, residual arc scans, and pushes.
The plastic solve has fixed dimension and exchanges the whole plug's mass. Counts are separate
from structural-check and decoration work and are not server-time measurements. The owner's
hole-within-two-contact-ticks gate remains.

**Trees are constructions by the same laws.**
The prop generator authors one convex trunk element. The common cutter's grain-stretched
pattern alone partitions its interior into chunks; source mesh rings introduce no internal
break plane. Species data selects the shape, material, pattern and ground attachment.
`assets/trees/` holds generator inputs and inspection artifacts; the placed game reads
`assets/constructions/` geometry and dressing through the common construction library.

Green-wood density, strength and separation energy live in `assets/materials/materials.ron`.
Density sets occupied mass and inertia; no dry-timber density substitutes for the green wood.
A species' `anchor` selects a `RootPlate` kind from `assets/materials/attachments.ron`, whose
critical overturning moment per kilogram binds the declared soil and rooting-depth domain.
It states ultimate strength, no separation-work measurement. DERIVED: the equivalent circular
footprint's tensile hold is `C m / S₀ − m g / A₀`, so the Navier law opens it at moment `C m`
under its own weight. Zero separation work is a StandIn for a rigid-plastic release until a
source supplies that work fact; a blow still pays the lift over its hinge.

`construction::spawn` builds complete state synchronously from admitted data. Common wear,
cracking, contacts, connectivity and launch govern the trunk's chunks and every later split.
No HP pool, segment grid, tree schedule or cut-range ownership supplements those laws.
Wood exterior and foliage belong to the common dressing and follow their owning chunks.
The shared ledger carries wear, holes, dust and cracks; swaps and departures derive from those
causes. Pieces use source construction/chunk addresses, and the shared cursor controls their
membership as described in “One assessment path for fixed and moving chunk groups” below.

## Local response and holes

Represent surviving matter independently of baked chunk fate.

A locally activated patch contains deformable cells or sub-blocks through the wall depth.
Its constitutive response supplies elastic strain, irreversible compaction, cohesive separation and motion.

```text
σ = C : (ε - ε_p)
D_compaction = ∫volume ∫ p_c dε_v^p dV
D_fracture = ∫new separation G_c dA
```

Compacted material retains its mass and updated density.
A geometrically separated piece retains its material.
Neither becomes absent merely because a scalar pool is partly spent.

For a round, resolve its swept channel and surrounding crater.
For a blast, resolve the face-loaded crater and rear breakout.
A hole covers only the region its material actually vacates.

Depth refinement must permit a rear layer to separate while the front survives.
Choose refinement from convergence of aperture, residual speed, rear breakout and seam loading.
Baked chunk width is not the minimum hole diameter.

**A hole is a negative volume, drawn only (owner decision).**
A round's channel and a blast's crater dish are stored as shapes in a per-chunk hole list, carried in the record.
The chunk's geometry and collider never change.
The wall's shader skips the surface inside a hole and draws the hole's inner wall, shadow pass included, so sight passes through it.
Rounds, mass, centroid, inertia and the structural calculation ignore holes: a later round meets the whole chunk.
A cap on holes per chunk bounds what is drawn and recorded; a later hole still wears the chunk's pool, and only a spent pool turns a chunk to dust.
A hole the size of a chunk or larger stays a chunk fate (`Dust`, `Fall`), as today.

**Server cost.**
**DERIVED `O(N_c N_t + G)`**, from activated local cells, time steps and generated geometric fragments.
Cache unchanged geometry.

**Tanks.**
Shared projectile response changes residual speed and subsequent dose.
Applying persistent local hole geometry to tank armour is a separate scope decision; do not imply that the wall implementation supplies it automatically.

### Crack-field validity

Lamé cavity stress is limited to an embedded cavity in a connected, approximately homogeneous continuum:

```text
σ_r = -p_cavity a³/r³
σ_θ = p_cavity a³/(2r³)
c ≈ √(E/ρ)
```

`p_cavity` comes from cavity traction.
It is not the deposited energy divided by an arbitrary volume.

Use the approximation only before a relevant boundary’s reflected wave returns and while cavity geometry remains sufficiently embedded.
Stop transmission at gaps and separated faces.
A face blast uses face traction, not an embedded-cavity field.

For **ASSUMED thickness 0.25 m**, **CLAIMED density 1845 kg/m³** and **ASSUMED modulus 8.6 GPa**, transit is **DERIVED 0.116 ms**, from `h/√(E/ρ)`.
Those inputs do not justify ignoring the back face during a round crossing.

**Stand-in and cost.**
Use a restricted cavity adapter only where it agrees with the local transient solution.
Its **DERIVED cost is `O(Q_j)`** for evaluated seam samples.
Boundary-sensitive cases retain the transient patch.

**Tanks.**
Any shared spall source must respect rear surfaces and intervening separations.

## Parts decide what stands

Subject: the game's environment destruction model, game state only. **Owner decision
(2026-11-05):** structural collapse is decided at the part scale only; released parts crumble on
their own, and the physics engine owns touching between separate bodies. Candidate cuts, section
searches, punch rings and a per-chunk force solve have no role in this model.

**The part graph** (`fracture::parts::Graph`, built at load from the construction data). A part
is the standing chunks of one baked element that intact bonds inside it join: one rigid node
with six degrees at its centre of mass. An element's buried chunks are a fixed part of their own,
so its section at grade is an interface rather than the inside of an immovable part. An
interface is every live joint crossing between two parts, or from a part to the ground. Its
stiffness and capacity are its joints': damage inside a part (removal, a cavity crack that splits
it into two nodes) reaches an interface only through the joint area that remains, never through a
factor. MEASURED (`parts::tests`, the shipped data): house_1 has 72 parts (its 55 elements, 17
split at grade; 18 parts fixed in all), 180 interfaces (44 push-only), 815 bonded and 71 push-only
joints crossing them, 324 unknowns and 7,362 factor entries; its factorization costs 351,018
scalar operations and one load column 28,476 (DERIVED loop counts, no timing). fir_1 is one part
on one root-plate interface, 6 unknowns. The shipped map places 74 houses and 635 firs.

**The joint law** (`fracture::bondsolve::Edge`). Each joint is a compliant patch over the actual
convex overlap of its authored chunk faces; the load gate reconstructs that overlap and refuses an
area or centre inconsistent with the bake. Normal and tangential compliances are the sum of the two
sides' representative lengths over their moduli (a ground anchor has one side):

```text
ell_i = cbrt(V_i) / 2
K_n = A / sum(ell_i / E_i)
K_s = A / sum(ell_i / G_i)
Q = integral(r r^T dA) / A
P = identity - n n^T
B = trace(Q) P - Q
M = K_n B omega + K_s trace(Q) n (n dot omega)
```

These are DERIVED stiffnesses, never tuned strengths; the half-side of the equal-volume cube
apportions bulk compliance and is not a flaw size. Triangle fans integrate Q exactly; `AreaShare`
root plates integrate each cell's share over the common equivalent disk. A stated attachment's
fixing carries the bond's whole stiffness, `K = A k_interface` from its own slip slopes (the nail's
lateral slope derives from EN 1995-1-1 section 7.1's `K_ser`, sourced in `attachments.ron`); the
bulk bearing beside it is not a second edge (ASSUMED, the bonded kernel's choice), so the pair
friction on its compression joins its shear capacity. The roof's construction audit is in
`.agents/scratch/bondsolve/roof-s3.md`.

**Load sharing** (`fracture::parts::Assessment`). An owner's interface joints assemble one
`K u = f` over its parts, factored by sparse LU without pivoting on the symmetric pattern, ordered
by exact minimum degree on part blocks (`parts::Factor`, from the bonded-factor experiment), once
per design at load. LU, because a slipping rest's friction makes the system non-symmetric; it does
the Cholesky's updates on both triangles, about twice its operations (DERIVED), and one column
solve costs `4 (entries − n) + n`. Instances
share that factor until their first change; a topology change refactors, which at part scale costs
less than one crack's rank-six downdates would on the chunk graph. Two load columns are solved:
sustained (gravity and presses) and total (plus each blow as its mean force over the tick, capped
by its stated peak). Bonded joints are solved exactly; push-only joints (rests and cracked seams)
join the same linear field with their bulk stiffness until their contact verdict opens or slips
them. A free component is balanced by its rigid acceleration (`accelerating_frame`, spin adding
centrifugal and gyroscopic loading) and gauged at its first part; the gauge is a frame, never a
support. A solve runs only when topology changed or a part's load moved past `LOAD_TOLERANCE`
(Resolution 1e-5 of the owner's weight, and of weight times radius of gyration for a moment: 100 ×
the 1e-7 contact-reading noise MEASURED in `.agents/scratch/bestofboth/bob-split.md`, which defeats
an exact-equality skip). An owner at rest performs zero work.

**Verdicts: contacts settle, then bonds** (`Assessment::settle`). The contacts settle first, in
the tick: each pass solves, opens every push-only face that lifts or tips, starts every face past
its Coulomb capacity slipping, and solves again. A face only goes stick to slip to open, never
back, so the passes end within one per face. The bonds are judged once, on the final field; a
crack changes topology for the next tick.

- A bonded interface cracks whole once one of its joints reaches its strength in the solved
  field: sustained normal traction over tensile strength, or shear over cohesion plus pair
  friction on its compression; or the total field over either strength while the pulse adds at
  least the whole interface's `Σ G_I A` or `Σ G_II A` of elastic work. A force-controlled driver
  pays its own separation work. Its joints then bear as push-only; when the field pulls the
  failed interface apart, it separates in the same tick. Traction extremes lie at polygon
  vertices: normal `N/A + (n cross (B + n n^T)^-1 M_b) dot r / A`, tangential
  `F_s/A + T (n cross r)/(A trace(Q))`; circular footprints use their analytical rim extrema.
- An interface's push-only joints bear as faces (ASSUMED), whatever bonds share the interface: a
  rest never borrows a bond's tension, so an uplift past a rest's weight lifts it and the bond
  beside it carries all the rest. A face is the push-only joints whose normals lean the same way
  (a positive dot), with their area-weighted normal and the hull of their corners. It separates
  when its resultant pulls (lift) or when its pressure centre `r = (M⊥ × n) / N` (DERIVED) leaves
  the hull (tip). A face carrying less than Resolution 1e-9 of the owner's weight is left as it is.
- **Coulomb friction** (owner decisions, 2026-11-06). A face holds a sliding force up to
  `F = Σ μᵢ Nᵢ` and a twist about its pressure centre `p` up to `T = Σ μᵢ Nᵢ ρᵢ`, each patch `i`
  pressing with its own `Nᵢ` at its pair friction `μᵢ`, where `ρᵢ² = trace(Qᵢ) + |cᵢ − p|²` is its
  polar radius of gyration from its geometry, moved to `p` (DERIVED for uniform pressure as the
  root-mean-square reach, which bounds the mean reach by Cauchy–Schwarz). It slips past the
  elliptic limit `(f / F)² + (τ / T)² = 1` (ASSUMED: the elliptic approximation of the limit
  surface), `f` its sliding and `τ` its twisting demand (`parts::Slip`). A slipping patch loses its
  tangential stiffness and carries friction as a term of the system, `μ cos φ t N` at its pressure
  centre and `μ sin φ ρ n N` about its normal, with `N` its normal force, linear in the
  displacement, and `(t, φ)` the demand's direction at the slip's start (`bondsolve::Friction`).
  The term is a non-symmetric slip stiffness, solved directly by the LU factor, so friction
  converges by construction; the excess moves to its parts' other interfaces (a roof board's
  nails). A slip keeps its direction (ASSUMED) and never re-sticks; a rest that joins a slipping
  face slips with it. A slipping face separates only when it lifts or tips, or when its part is
  released. When nothing else holds its part tangentially, the stiffness left is singular (a pivot
  under Resolution 1e-9 of its elastic diagonal, the stiffness without friction: friction's
  non-symmetric term can lower a diagonal to zero or below, where a vanishing pivot would pass a
  test on it): the part is a mechanism, the slipping rests of every
  part it reaches through stiff interfaces separate (`Slide`), and connectivity releases it to
  the physics engine, which owns sliding bodies. A mechanism no slipping rest made (removal or
  cracks leave a part on a joint too slight to hold it) separates the joints its free motion
  moves (`parts::Mechanism`): the mode with the fallen pivot's coordinate moved by one, every
  later coordinate held and the earlier ones in equilibrium (DERIVED from the factor's own
  back-substitution); a joint whose sides move apart by more than Resolution √1e-9 of the most
  any joint does carries it, since the mode moves a stiff joint by about 1e-9 of a free one.
- Separation is permanent: no re-close. What lifts, tips or slides as a mechanism leaves the
  graph; once released, Avian owns its motion. The ledger records `Crack` and `Separate` per
  joint, and replicas derive support from those causes without any equilibrium; a slip is
  authority state and writes no ledger entry.

**Release.** Connectivity (`loose_groups`) runs on every topology change; each group with no
path through live joints to the ground or buried matter is released in the same tick and spawned
as one physics body (ADR-0014: new entities, constructed from data). Every released body,
whether it left a placed construction or split from a moving one, is built by
`breakage::prepare_group`, the hand-off to the layer that makes released parts crumble.

**Inside a standing part** (`fracture::parts::Kernel`, the one chunk-scale kernel). The kernel
is the bonded-factor experiment's solve at element size: a part's chunks are nodes, its intact
bonds edges at their seam stiffness, its cracked seams and rests push-only edges at bulk stiffness
that stick (ASSUMED: once cracks split the part, the part law judges the pieces' sliding with
Coulomb friction), and its joints to the ground or to buried chunks anchors. A chunk set nothing
anchors is balanced by its rigid acceleration and gauged at its first chunk. Joints to chunks
outside the set are no edge: their force is the caller's load. A heavily cracked set can be a
mechanism: the joints its free motion moves part (`Slide`, as at part scale), every event on it
reports them, and the rest is factored again. It is factored once per element
design at load (`Assessment::kernels`) and again only for a part whose chunks changed, and it runs
only on events through one entry, `Kernel::hold(structure, loads)`: per chunk, the loads beside
gravity (the forces its other interfaces hand it, contacts, presses), returning each joint's
carried wrench, the bonds that crack (tension past `f_t`, or shear past cohesion plus friction on
compression) and the push-only joints the field pulls apart. A landing's seam cracks read the
same entry with the landing's contact loads.

**Holes.** When chunks are removed, each part beside them is solved at chunk scale for gravity,
the presses on its chunks and the sustained forces its other interfaces hand it, friction included
(`Assessment::holes`). A joint that cannot
hold, mortar tension included, cracks, a pulled one separates, and the part is solved again on
what is left until it holds. The cracks then split the part, and the part law judges the pieces:
a chunk or cluster whose faces lift, tip or slip as a mechanism separates and connectivity
releases it. Arching is not authored: compression runs across the running bond's overlaps
because the bed joints are where the chunks overlap. No masonry flag or per-material rule exists.

MEASURED (`parts::tests`, a fixture 12-course running-bond wall, observations): a hole of 11
bricks, three courses high and four bricks wide at the base, drops nothing with the shipped brick
seam; the event costs 3,701,506 scalar operations (the part refactor, and the kernel factor and
solves over the wall's 139 remaining chunks). With a no-tension, no-cohesion fixture seam, 43
bricks drop and the wall rests 7 ticks after the event: all 11 of the 45° triangle over the opening, which NCMA TEK 17-01D
CLAIMS arching leaves to a lintel, and 32 above it in a chimney three to seven bricks wide to the
wall's free top. A brick left on half its overlap has its centre of mass over the edge of its
rest (DERIVED), so corbelling is marginal, and a free top gives the arch no abutment load; the chimney is
the model's answer, not a pinned one.

**Hand-checkable gates** (`parts::tests`, fixture geometry): a lintel's two pier shares
`P/2 ± P a s / (2 s² + r²/2)`; a cantilever's interface carrying `W + P` and `W d + P L` and
cracking at `P* = (f_t r A / 4 − W d) / L`, then tipping; a two-joint wall base cracking whole at
its Navier capacity `P* z = (f_t + W/2A) A (2 s² + r²/2) / (s + r)`, then tipping; the same wall
with one block worn away cracking at its one remaining disk's `P* z = (f_t + W₁/A) A r / 4`; a slab
resting on two piers bearing `W/2` on each and slipping past `μ W`, a mechanism nothing else
holds, so it is released. Each holds at 0.98 and changes at 1.02 of its DERIVED limit. A block on a
slope fixed to a buried post sticks below `atan μ`; above it the rest slips, keeps bearing, carries
no tangential force through stiffness and `μ N` as friction, and the fixing carries the excess
`W sin θ − μ N` (DERIVED from the block's equilibrium); once friction settles it costs nothing.
A block at `μ = ½` on a rest of normal stiffness `k`, fixed by an oblique fixing of stiffness
`k [[1, 3], [3, 9.5]]` (where a fixed point on the friction force alternates), settles in the tick
it slips at the DERIVED `N = W/3`, `μ N = W/6`, and costs nothing after; with `k [[1, 2], [2, 5]]`
its fixing cracks only past its settled shear `0.101 W/A`, not the `0.146 W/A` of a fixed point's
first iterate. An uplift past a rest's weight lifts it, and a fixing 1/1000 as stiff beside it
carries all of `P − W`. A disk rest slips past the yaw couple `μ W R/√2`, and a two-patch face of
different pair frictions past `μ₁N₁ + μ₂N₂`, not the mean `μ` times `W`. A hole that leaves a
ligament cracks it past `f_t = 4 W d/(r A)`, and a press beside the hole loads it:
`P* = (f_t r A/4 − W d)/L`; only the chunk-scale solve sees either.

**The unit house under gravity** (MEASURED, `parts::tests`, observations). Its 71 roof-board rests
lie on the gables' 36.87° slopes, where a board needs friction `tan 36.87° = 0.75` against the pair
friction 0.57 (DERIVED from the slope normal and the shipped wood): every rest slips at load, keeps
bearing, and hands the excess to the boards' nails. The house stands: no joint fails, nothing is
released, and its compiled field is already at rest: its first tick solves nothing and does zero
work, as does every tick after. In the settled sustained field the most loaded bonded joint uses
0.198 of its tensile strength and the most loaded in shear 0.245 of its cohesion plus friction
(margins 5.1× and 4.1×). Its 55 element kernels cost 8,553,917 scalar operations to factor at
load; the largest has 66 chunks, 396 unknowns and 13,230 entries, factors in 985,830 operations
and solves one column in 51,732. A fixture 1 MN press on a wall part's top fails parts in
tension, shear, lift, tip and slip (as mechanisms) and comes to rest 7 ticks after, every value
finite (an observation: every house part is held by several interfaces, so no part capacity has
a closed form to bracket).

**Limits.** Contacts settle within a tick; a bond's crack redistributes from the next tick.
Friction keeps its first direction and a slip never re-sticks, so a demand that turns or eases
within a slip is not followed; a rest that joins a slipping face takes the direction and reach of
the rest it joined. Inside a part the kernel's
cracked joints stick, and its event loads come from the rigid part field. A pulse is a tick-duration quasi-static load, not a transient
elastic wave; changing the tick can change a blow's verdict, while sustained limits are
independent of it. The pulse work criterion is recoverable elastic work, not a measured dynamic
fracture law. Pulse fracture work follows the cracked joints into the common launch debit
(`fracture::debit_work`), split once between departing owners.

## Energy and momentum budget

For each source and interval:

```text
E_initial + E_chemical + W_external
  = E_outgoing_projectiles + K_retained + K_ejecta
  + U_elastic + U_gravity
  + D_compaction + D_fracture + D_friction + D_heat
  + E_gas_remaining + E_exported + E_numerical_error
```

Use changes in stored terms when opening an interval.
`E_exported` includes escaping blast energy and unresolved motion explicitly removed from the model.
Numerical error is reported, not hidden as heat.

```text
p_initial + J_external
  = p_remaining_projectiles + p_structure + p_ejecta + p_gas + p_exported

L_initial + ∫ M_external dt
  = L_remaining_projectiles + L_structure + L_ejecta + L_gas + L_exported
```

Ground and foundation reactions are external terms.
A wall can receive impulse with negligible work.
Blast pressure is therefore never converted from intercepted chemical energy into crushing work.

Casing acceleration spends chemical energy once.
The remaining product state drives blast.
A bare-charge pressure curve and a full casing-energy allocation cannot both consume the same source energy.
Do not obtain cased-blast pressure by subtracting casing energy and blindly treating the remainder as TNT mass.

Projectile loss is the boundary transfer into the local response.
Compaction, fracture, elastic storage and ejecta are partitions of that transfer.
They are not additional copies of it.

### Spall as ejecta

Determine ejected mass and velocity from local separation and unloading:

```text
m_e = ∫ejected ρ dV
p_e = ∫ejected ρu dV
E_e = ∫ejected ρ|u|²/2 dV
```

Conservation checks the result; it does not choose the ejected share.
Keep target material and projectile material distinct.
A spall cone is a transport representation of this ejecta, not another allocation of projectile loss.

**Stand-in.**
Replace unresolved ejecta with mass-and-speed bundles preserving energy and vector momentum.
Retain recoil on the remaining material.

**Server cost.**
**DERIVED `O(N_e)`** extraction from ejected cells, followed by fragment-transport cost.

**Tanks — decided (2026-10-31).**
Wall ejecta never dose a tank: they are debris, drawn and simulated, with no potency.
The tanks' own strength-scaled spall cone (tank-design section **5**) stays; budgeted ejecta would need constitutive profiles the model does not hold.

The supplied burst factor is **ASSUMED 210** and the field emits its mathematical half.
Its emission is therefore **DERIVED 105 times** driven casing energy.
That potency fit cannot enter the physical wall budget.

**OWNER DECISION — hard-pool dose accounting.**
Recommend deriving hard-pool dose from locally absorbed, damaging work, excluding outgoing fragment kinetic energy.
Keep the existing single energy-to-HP conversion and receiver susceptibility.
Keep crew line rate as the separate abstract witness response described in tank-design section **6**.
Record this amendment there before migrating tank outcomes.

## Hand-off to moving bodies

A detached group retains its surviving internal seams and constitutive history.

```text
M = Σ m_i
c = Σ m_i x_i / M
I = Σ [R_i I_i R_iᵀ + m_i (|r_i|² I_identity - r_i r_iᵀ)]
v = P/M
ω = I⁻¹ L_about_c
```

Use actual surviving geometry in each term.
A standing structure is rigid to the solver, so no structural response has moved a group before it breaks free.
The group leaves with its source momentum once: the sweep's resulting velocity field, or the field impulse remaining after fracture work (see Bodies in contact).

A rigid carrier is acceptable while internal relative motion is negligible.
Retain unresolved elastic energy and seam history.
Report any energy discarded when replacing internal motion with rigid motion.

A landing crumbles the carrier (**Crumble on landing**).
Children inherit the parent velocity field; splitting conserves mass, momentum and angular momentum and adds no kinetic energy.

**Inputs.**
Use rigid-body mechanics, material profiles and contact geometry.
No separate landing-fragility multiplier is selected.

**Server cost.**
**DERIVED `O(N_b)`** mass-property construction for `N_b` members, plus active seam/contact cost.

**Tanks.**
Loose and resting pieces retain their material in ballistics.
Landing and pushing may fracture masonry without creating contact HP dose to tanks.

### One assessment path for fixed and moving chunk groups

Subject: the game's environment destruction, game state only. A moving body keeps the same
`Breakage` and `Live` construction state as a placed construction: pools, internal bonds, opened
joints and holes. The immutable structure table is shared; each group's state names exactly
its owned chunks. Every field deposit, and every solver contact on a placed construction, enters
`run_laws`: wear, cavity cracks, joint equilibrium and connectivity have one implementation. The
laws read the solver's `Position` and `Rotation`, never an eased display transform.

A solver contact on a placed construction is a transient point resting joint to the other
physics body. Its normal, point, signed impulse and press come from `contact_transfer`; the fixed
side has zero inverse inertia. The contact's transferred impulse has already changed the other
body's motion, so it gets no second plastic exchange or launch. A moving group's own contacts
do not enter the laws: they crumble it (**Crumble on landing**). External fields still apply
their own impulse. There is no landing-fragility factor or switch for a piece's kind.

The part verdict and accelerating-frame reduction are specified once in **Parts decide what
stands**. Every changed owner runs one stage per tick; a topology change or an ended pulse keeps
it active on the following tick, even when no new deposit arrives. Spin changes also wake
assessment. Fixed and moving groups share that path; contacts remain applied loads rather than
invented supports.

A failed seam partitions ownership at the same authoritative tick. `PendingSplit` creates children after the law stage and before publication.
Children inherit `v + omega cross r` at their actual occupied centres of mass and the parent's
angular velocity; their whole occupied hulls partition the parent’s occupied geometry. A chunk occupies its whole hull standing and freed: the game's rounds and the sweep's targets read it, and the parent body owns its occupied mass, inertia and centre. Only contact is separated: a freed chunk's contact hull, also its sweep source, sets each face carrying one of its joints back `LOOSE_GAP_M` (a `Resolution` against Parry's zero-distance normal noise, its basis beside the constant), and keeps every other face. A freed chunk therefore carries two colliders: its occupied hull, a sensor met only by rounds, and its contact hull. This adds no kinetic energy. The existing
fracture-work criterion is a threshold, not a resolved cohesive dissipation/strain model.
The sweep exchanges body momentum before physics; field departures retain their field-impulse launch.

`Piece` carries birth and retirement ticks (protocol revision **60**, a deliberate wire
change shared with the module repair snapshot). A retired parent's final pose remains
replicated. Replicas show it until the shared cursor reaches the split and the replacement
groups have arrived; children wait for their birth tick, and then behind any older piece still
drawing one of their chunks, so each chunk draws once. A moving group owns its chunks alone:
the construction's unbreakable elements stay with the placed construction, and a field's blow on a chunk
it turns to dust moves no group, as on a standing construction. The replacement bodies are server-only, and the client keeps deriving the standing wall's
shape from the ledger. Continued holes remain drawn data and do not change occupied geometry.

The moving group's force solve still takes field loads; its law fixtures use ASSUMED geometry
and seam characterization: free fall opens no seam, a single chunk has none to open, an even blow
along a slab bends no section, and a resting joint carries compression from either side but never
tension. These are invariant tests, not empirical material or handbook validation. More live
bodies and retained carriers remain for the separate pricing lane to measure.

### Crumble on landing

Subject: the game's environment destruction, game state only. A released group cracks where its
landing's force runs; its light fragments leave the simulation for the players' decoration.
Code: `src/debris/crumble.rs` (law and size split), `src/debris/decor.rs` (view).

**Law.** A landing is a physics step in which `contact_transfer` delivers a moving group a blow
past its rest speed (Avian's linear `SleepThreshold`, the split between a blow and a press). The
blows one chunk takes from one other collider are one patch: summed, at their blow-weighted centre
and closing speed, before any force law reads them, so the narrow phase's point count changes
nothing (converting each point alone would multiply the summed peak by `n^(2/5)`, DERIVED). Each
patch's blow peaks as two elastic bodies stopping each other do (Hertz impact; K. L.
Johnson, *Contact Mechanics*, 1985, §11.4): `1/E* = Σ (1 − ν²)/E`, `1/R = Σ 1/R`, approach
`δ = (15 m v² / (16 E* √R))^(2/5)`, peak `P = (4/3) E* √R δ^(3/2)`, with `m = |J| / v` the mass a
plastic stop of the patch moved (DERIVED). The landing chunk's side is its material (`ν` from its
two moduli) curved as the sphere of its volume (StandIn for the patch's own curvature); the other
side is the chunk it meets by that one's material, a whole element flat by its substance's
stiffness (a breakable one's moduli, or the material facts of one that never breaks; a substance
stating neither is refused at the load gate), the ground by
`ground_youngs_modulus_pa` and `ground_poisson_ratio` (`Unknown`, `physical_laws.ron`), or a body
with no stiffness data, such as a hull, as rigid (StandIn). The solver's step enters nowhere: the
contact's stiffness sets how hard it stops. The patch's peak, and each point's press, is a force on
the chunk it touches. The group's chunk kernel (**Inside a standing part**,
`Kernel::hold`) balances those forces by the group's rigid acceleration, so uniform gravity
cancels and the internal forces are the inertial relief, and judges every joint: a bond past its
tensile strength, or past its cohesion plus friction, cracks and bears only push; a joint the
field pulls apart parts. The field is solved again on what is left until it holds (at most 16
passes, `Resolution`); once a joint parts, a touched fragment takes only its share `m_f / M` of the
landing's force, the momentum a plastic stop of that fragment alone needs (DERIVED). Cracks lie
where the force runs, and a short drop cracks nothing.

Energy is conserved, not capped. A crack absorbs its fracture work `G × A`: its seam's fracture
energy in the failing mode (`fracture_energy`, mode I, for tension; `shear.energy`, mode II, for
shear; material data with their own labels) over the joint's area. A parting holds by no cohesion
and costs nothing. All cracks together never absorb more than the kinetic energy the landing
dissipated, `E = ½ Σ |J_i| v_i` over its blows and closing speeds (DERIVED, a plastic impact).
When the other side is a moving group crumbling from the same collision, each side reads that one
collision with its one effective mass and closing speed, so each spends only its own compliance's
share of it, `c_self / (c_self + c_other)` with `c = (1 − ν²)/E`: under one force each side's
approach, and so the work done on it, is in proportion to its compliance (DERIVED from Hertz), and
the two together spend the collision's energy once. Against a side that spends none (the ground, a
hull, a placed construction, whose contact loads carry no energy) the group may spend all of it.
Past that they are taken most overloaded first, and the landing stops at the first crack the
energy left cannot pay; what it does not spend stays in the solver's contact dissipation. What
stays connected over the joints that did not part is a fragment. The crumble moves nothing: every
fragment leaves with the parent's rigid field `v + ω × r`.

This replaces the moving group's contact load (the decided **Collision loads** row's "one load for
every source" now covers fields and placed constructions only); that reading is the owner's to
confirm.

**Size split.** A fragment stays a server body when its size, the cube root of its occupied
volume, reaches `server_rubble_min_size_m` (`crumble::server_body`,
`assets/ballistics/physical_laws.ron`), whatever its mass, so a long splinter counts as small.
It is one game-scale `Taste` for every material (which freed matter the player sees stay solid),
tuned by eye from 0.5 m. The owner decided this on 2026-11-06 and rejected a mass floor, because
it ties the floor to material: fir at about 450 kg/m³ would need more than 1 m³ to reach 500 kg,
and boards would never break free. Weight was allowed if it were cheaper for the same result, and
it is not: server cost follows the number of server bodies and their contacts, not their weight,
and a weight floor scaled by each material's density selects exactly the pieces this volume floor
selects, so weight changes the result only where density differs, the case the owner rejected.
A body serves hulls and rounds at once, so the same number
decides what both meet. Its server cost is an observation, which #306's price measures: a volume
V of collapsed matter leaves at most V / size³ bodies (DERIVED). A smaller fragment merges
through a joint parted in this event into another departing fragment, the largest joint area
first (ties by joint identity); retained absorption joints preserve grouping without restoring
strength. A fragment with no such neighbour becomes decor.
The shared `route` serves hull cuts, structure releases and landings, each fragment with its own
release motion: resolved fragments stay server bodies; decor writes its chunks as dust at the
event's tick and sends one `Shed` (construction, chunks, frame pose, velocity, spin, seed, tick)
to every player on `DebrisChannel`, reliable and unordered. At 0.5 m (DERIVED from the
construction files), the shipped house keeps as bodies its whole piers, gables, wall plates and
six of its seven sills; a whole lintel (0.47–0.48 m) or roof board (0.40–0.42 m) is smaller, as is
every single chunk (0.12–0.42 m). The shipped fir keeps its whole trunk (1.06 m), none of its
single chunks (0.22–0.44 m).
A replica refuses a shed whose chunks its map cannot hold or whose
motion is not finite.

**Decoration.** Each player draws a shed from the event alone at the shared presentation cursor
(ADR-0037): the chunks' own mesh in the constructions' surface material (holes are not drawn),
flying the closed form of gravity, velocity and spin, resting where a hull corner first meets the
terrain height field closing on it, then shrinking away. A piece that starts on the ground (a swept
ground-floor patch) rests there only if its own motion closes on it; moving away or along, it
flies on until it next closes. A shed of a world not yet selected (one that outran its Battle's
identity) waits for that world, and one of a world left behind goes; the world boundary retires
every drawn piece and every shed still due (`decor::reconstruct`). Its lifetime is seeded within
`construction_decor.lifetime_s` and its fade is `fade_s` (both `Taste`,
`assets/vfx/presentation.ron`); at most 256 are drawn at once (`Resolution`, a draw-call bound),
the oldest going first. It has no body and no collider, so no hull, round or server body meets
it, and two players drawing one event at one cursor draw one pose.

**MEASURED** on the shipped house's element with the most internal joints (66 chunks of
0.74–48 kg, 1712 kg; no single chunk reaches the cut-off), landing on the ground. Flat, on its
lowest course, it cracks nothing below a 0.48 m drop (DERIVED from the seams' strengths and the
two sides' stiffness, `a_short_drop_lands_whole`; on a rigid floor, 1.1 cm); from 10 m it cracks 28
bonds, parts 19 joints and stays one server body. Through one corner chunk, a 1 cm drop (28.6 kN
peak) cracks nothing; 10 cm (114 kN) cracks 119 bonds into 23 fragments, 2 kept as server bodies;
10 m, 29 fragments, 1 kept. The cracks absorb 87–122 J of the landing's 1.7–168 kJ. The corner's
peak against this ground is close to the step-based force it replaced, so the corner's breakup
does not come from the solver's step. A landing that cracks nothing costs 0.55 M scalar
operations, its kernel's factorization included (DERIVED); a cracking one 1.2–3.2 M over its
passes.

## Rubble settlement and the cap

Rest requires a supported equilibrium, not merely low velocity.

```text
Σ F = 0
Σ M = 0
normal reactions ≥ 0
|tangential reaction| ≤ μ normal reaction
```

A support chain must terminate in terrain or another stable foundation.
A mutually supporting airborne cluster is not grounded.
A piece resting on a moving tank remains mobile.

Maintain support dependencies.
Wake dependants when a support moves, separates, loses capacity or receives a relevant load.

Before contact resolution, test approaching swept geometry against resting rubble.
Wake the contacted piece and affected support island before solving that contact.
The first tank touch therefore meets finite mass and transfers opposing momentum in the same step.

**Stand-in.**
Rubble rests through Avian's island sleep (`src/debris.rs`).
A piece stays a dynamic body all match; an island sleeps once every body in it has stayed under Avian's sleep thresholds for its `TimeToSleep`, and keeps its geometry, mass and contacts asleep.
An awake body that starts touching a sleeping island wakes it before the solver runs, so the first touch meets finite mass.
Avian wakes nothing when a support with no dynamic body goes, is disabled or moves; the game wakes every sleeping body touching it, a piece or any other (`src/debris.rs`).
A rest timer is debounce, not proof of support.

**Server cost.**
**DERIVED `O(N_s + E_s)`** for an affected support graph, plus approaching-contact queries.

**OWNER DECISION — cap semantics.**
Recommend capping detailed Avian bodies, not declaring excess matter static.
The inherited **ASSUMED cap of 256 detailed bodies** is a resource setting, not a physical constant. It is not implemented: no cap exists in the code.

Reserve slots before activation.
Use bonded compounds where internal motion is negligible.
Use a finite-mass coarse solver for overflow groups, retaining occupied geometry, gravity, momentum and tank contact.
Promote groups near contact or renewed fracture by demoting eligible supported groups.

Never freeze an unsupported group to enforce the cap.
Never delete gameplay-relevant rubble to free a slot.
Never rigidly weld unrelated pieces merely to reduce body count.

A detailed-body cap does not bound total coarse-solver work.
Arbitrary persistent, independently moving rubble cannot have both exact behaviour and a fixed total computation bound.
Measure that worst case and report the resource limit explicitly.

**OWNER DECISION (2026-10-31) — rest by the library's sleep, not a freeze rule.**
Rubble rests through Avian's island sleep and its wake on contact.
No resting piece switches to a static body: a switch rebuilt the piece's contacts and woke its island, and the switching undid Avian's own sleep (`.agents/scratch/destruction-2026-10-30-constructions/debris-cost-survey.md`, §2).
No cap applies until a sleeping piece's per-tick cost is measured.

What limits the piece count (DERIVED from `src/construction/breakage.rs`): every surviving chunk belongs to exactly one body; splitting partitions that ownership, so live construction bodies never exceed the map's breakable chunks. Splits retain historical pose carriers for the delayed presentation cursor and late joiners; DERIVED carrier bound is `2N - 1` for `N > 0` breakable chunks, since every retirement either partitions a group or removes at least one chunk. Nothing else caps live bodies or carriers. The live-body bound is `N`, summed over every admitted map placement, including wood constructions; neither bound forecasts how many chunks a drive releases. The risk this decision brings is an awake pile held by a tank's island (below), which no measurement prices yet; the pricing lane must price it.

Two costs of the library's sleep, MEASURED as counts over 10 s on a fixture lattice of 54 cubes, 0.4 m, 2 cm apart, beside a 3 × 2 × 6 m box with sleeping disabled standing in for a tank (not a hull). The harness that produced the counts is not committed; the numbers rest on it:

- An island sleeps whole. A tank never sleeps (DERIVED: a non-zero force call resets the sleep timer in Avian, and the track code applies forces every tick), so every piece connected to it through touching pieces stays awake. MEASURED against the box: six pieces thrown at it kept 36 of 54 awake; pressed against it, 15 touching kept 18 awake. With a body free to sleep in the box's place, all 54 slept.
- Avian moves only touching pairs to its sleeping set. The fully sleeping pile kept 329 non-touching near pairs active, and the narrow phase recomputes those every tick; 54 touching pairs slept.

## Validation without outcome tuning

Lock geometry, material characterization, source conditions and expected results before running each case.
Resolution and solver tolerances may change for convergence.
Material values and coupling factors may not be fitted to the expected result.

| Case | Expected result and source |
|---|---|
| Contact charge against brick | Through-breach and rear breakout for a published charge/wall pair. **CLAIMED qualitative basis:** military demolition references such as FM 5-250; coupled-product mechanics supplies the response. Exact charge, confinement and aperture bounds require a verified source case before this gate can pass. |
| Standoff charge against brick | No through-breach for a source case explicitly reporting non-breach. Global flexure or cracking may still occur. **CLAIMED source family:** UFC blast-loaded masonry guidance and published wall tests. Moving the contact fixture outward does not by itself establish the expected result. |
| AP through brick | Perforation and a persistent sub-chunk channel for the analytical fixture below; empirical crater dimensions require an independent shot record. **CLAIMED theory:** Poncelet/cavity resistance and rigid-projectile geometry. |
| Sustained push | A free-standing panel topples when sustained overturning moment exceeds its restoring moment, independently of time-step choice. **CLAIMED source:** rigid-body statics and Housner’s rocking-block mechanics. |
| Section landing | A published or independently characterized section fractures again under its landing contacts. Match support geometry, orientation and connection properties. **CLAIMED source:** impact mechanics and cohesive fracture; available fall energy alone does not prove a particular break pattern. |

### Analytical AP fixture

Use **ASSUMED** mass `10.2 kg`, calibre `0.088 m`, speed `773 m/s`, normal chord `0.25 m` and nose factor `7/24`.
Use **CLAIMED** density `1845 kg/m³`.
Hold **ASSUMED** resistance `135.46 MPa` and inertia factor `B = 1` solely to isolate the density correction.

The kinetic equation gives **DERIVED residual speed 686.8 m/s** and **DERIVED loss 642.0 kJ**, from those inputs.
It gives **DERIVED stopping depth 1.895 m**, so this analytical fixture perforates.
The rigid swept-body approximation gives a **DERIVED calibre-sized channel of 88 mm** before surrounding breakout.
Neither prediction is an empirical brick-penetration validation.

The old crush-volume construction gives **DERIVED radius 51.1 mm** from **ASSUMED loss 278002 J**, resistance `135.46 MJ/m³` and thickness `0.25 m`.
That equivalent cylinder is not evidence for crater diameter.

### Analytical rocking fixture

Use **ASSUMED dimensions `4 × 2 × 0.25 m`**, **CLAIMED density `1845 kg/m³`** and **ASSUMED gravity `9.81 m/s²`**.
Mass is **DERIVED `3690 kg`**.
The initial restoring moment is **DERIVED `4524.9 N·m`**, from `Mg t/2`.
The gravitational barrier about a bottom edge is **DERIVED `281.7 J`**, from `Mg(√(H²+t²)-H)/2`.

Apply **ASSUMED sustained horizontal force `6 kN`** at **ASSUMED height `1 m`**, with sufficient base friction and no bonded foundation.
The panel must pass balance if the load persists.
Check **ASSUMED tick rates `64`, `128` and `256 Hz`** against a converged trajectory.
Run a rigid-panel diagnostic first, then the seam model with independently specified strengths.

### Landing energy check

For the same panel and **ASSUMED centre-of-mass descent `1 m`**, available gravitational energy is **DERIVED `36.2 kJ`**, from `MgΔh`.
That energy must appear in rebound, surviving motion, deformation, fracture and dissipation.
The landing case passes only when contact stresses and cohesive work also explain its secondary fracture.

## Prototype sequence

**Source lock.**
Acquire exact breach, non-breach, AP aperture and landing reference cases.
Record their material, geometry, uncertainty and expected intervals.
Missing data remains a named acceptance gap.

**Blast pressure first.**
Show incident, reflected and cleared pressure histories on rigid faces.
Judge peak pressure, impulse, arrival time, resultant force and moment against the selected handbook data.
Show contact and near-contact loads separately from free-air loads.

**Parts next.**
Show gravity equilibrium, force paths, opening, sliding, crushing and redistribution.
Judge the sustained-push and rocking fixtures before coupling explosions.
Repeat with changed time steps, chunk cuts and depth resolution.

**Coupled breach.**
Show crater, channel, rear breakout and all budget terms.
Judge the locked contact and non-breach cases without fitting constants.
Report discrepancies with uncertainty and the proposed missing mechanism.

**Moving groups and rubble.**
Show tensor-correct launch, internal seam survival, landing fracture, support-dependent waking and the first tank shove.
Stress overflow while preserving finite-mass motion and occupied geometry.

Expose a short player view and a separate diagnostic view.
Capture pressure curves, force paths, geometry sections and budget residuals outside the play view.
Measure server cost on deployment hardware before selecting runtime refinement and atlas coverage.

## Review finding disposition

| Finding | Resolution or remaining gate |
|---|---|
| 1 — fragment potency | Physical fragment mass and speed; remove amplified potency from transport. Tank receiver migration requires owner acceptance. |
| 2 — blast interception | Pressure histories, moving-surface work and a separate product-contact response. |
| 3 — connectivity as support | Capacity-bearing structural solution before connectivity; include corner and roof load paths. |
| 4 — partial removal | Shared sub-chunk geometry and depth refinement. |
| 5 — strength-scaled inertia | Independent target density and resistance terms; porous-material validity remains a validation gate. |
| 6 — fracture versus toppling | Cohesive work separates seams; gravity and unilateral contact decide rocking and toppling. |
| 7 — sustained pushing | The strength limit, read against each tick's press from the solver's transferred momentum (see Bodies in contact). |
| 8 — universal Lamé field | Restricted embedded-cavity domain; transient local response near boundaries. |
| 9 — connection mechanisms | Symmetric opening, frictional sliding, crushing and cohesive softening; timber awaits directional data. |
| 10 — ejecta budget | Ejection response determines mass and velocity; complete energy and momentum accounts constrain it. |
| 11 — landing rigidity | Preserve internal seams; a landing's impact energy opens them cheapest first (see Crumble on landing). |
| 12 — settlement | Supported equilibrium, dependency waking, pre-contact activation and finite-mass overflow. |
| 13 — sampling and fuze trigger | Adaptive face integration; fuze approximation remains explicitly restricted below. |

For finding 13, replace whole-projectile-force comparison with an actuator response when extending fuze behaviour:

```text
m_a ÿ + c_a ẏ + k_a y = -m_a a_body(t)
function when the mechanism’s travel/latch condition is reached
```

**Inputs:** actuator mass, preload, stiffness, damping, travel, orientation and load path.
A quoted spring force alone supplies none of the missing acceleration coupling.
**Stand-in:** “substantial solid contact functions this nose fuze” may remain within the first slice’s demonstrated domain.
**DERIVED cost:** constant work per resolved load interval for a lumped actuator.
**Tanks:** preserve selected modes and delays; do not extrapolate the stand-in to base fuzes, grazing contacts or small debris.

## Owner decisions and recommendations

| Decision | Recommendation |
|---|---|
| Small-hole fidelity | **Decided:** a hole is a negative volume, drawn only; rounds, mass and strength ignore it (see Local response and holes). |
| Rubble persistence and mobility | **Provisional (2026-10-31):** retain gameplay-relevant occupied geometry throughout the match; let mass, friction and track contact determine climbing or pushing. Fine presentation debris may fade. |
| Collision loads | **Decided:** occupied body sweeps make holes by momentum and joint capacity before physics; remaining contacts feed the structural check as forces (see Bodies in contact). |
| Contact-damage scope | **Decided (2026-10-31):** wall material never doses a tank, whether it falls, is pushed or is thrown off a hit; walls still fracture on landing and under pushing loads. |
| Accepted strength-scaled spall | **Decided (2026-10-31):** wall ejecta are debris with no dose; the tanks' strength-scaled spall cone stays (see Spall as ejecta). |
| Three-property material model | **Provisional (2026-10-31):** keep the summary, add sourced constitutive and attachment profiles; do not infer missing properties from strength. |
| Hard-pool dose | **Provisional (2026-10-31):** exclude outgoing kinetic energy from absorbed damaging work; retain the shared conversion and abstract crew line rate. |
| Rubble rest | **Decided (2026-10-31):** Avian's island sleep and wake on contact; no body-type switching. The game adds only the wake Avian lacks, when a support goes (see Rubble settlement and the cap). |
| Structural collapse | **Decided (2026-11-05):** at the part scale only; one factored solve per changed owner, interfaces failing whole, one stage per tick; between bodies Avian owns contact (see Parts decide what stands). |
| Debris cap | Cap detailed bodies and use finite-mass overflow; accept that total coarse work still needs a measured limit. No cap applies until a sleeping piece's per-tick cost is measured. |
| Empirical acceptance data | **Provisional (2026-10-31):** require verified source fixtures before declaring handbook validation complete; do not substitute plausible-looking prototype outcomes. |
