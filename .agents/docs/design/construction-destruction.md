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

Every source, a body's contact included, enters as a load (see Bodies in contact).
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

**Owner decision (2026-10-31, [#232](https://github.com/vikng-dev/overmatch/issues/232)): the collision model.**
A body that touches a standing structure loads it through the same laws as a round or a blast.
No collision scenario is written: no "tank rams wall" rule, no marker for which bodies may break things, no per-object threshold.

**One load for every source.**
A round's line, a fragment, a blast ray, any body's contact, the belt's reaction and weight enter the laws as one load on one chunk at one point: a **blow** (N·s), a **press** (N) and an **energy** (J).
Its origin is a **field**, whose impulse its own law fixes, or a **body**, which shares momentum with whatever it frees.

**Contacts are the solver's own transfers.**
A contact's load is the momentum Avian's solver moved through each contact point in the step, normal and friction.
Per substep that is the post-relax accumulated impulse less the share of the previous one warm starting did not reapply; restitution adds its increment after the substeps.
`ContactPoint::normal_impulse` is not that momentum: Avian adds the whole accumulated impulse into it on every solve call, twice per substep.
**MEASURED** 2026-10-26 on the game's 64 Hz, 6-substep solver, in a capture record not committed: it reads 1.90 × F·dt under a tank's sustained push, 1.56 × the hull's momentum change in a 5 km/h ram, 1.01× and 1.00× at 10 and 20 km/h.
The reading (`contact_transfer`) sums Avian's public solver state for every contact between a structure and a dynamic body; the vendored crate is not patched.
Its test holds each step's reading to the body's momentum change; there a 1000 kg cube pressed on a wall by 5 kN reads F·dt to within 10⁻⁵ through the reading and 2.000 × F·dt through `normal_impulse` (**MEASURED**, fixture data).
A contact's transfer splits at the body's rest speed, Avian's own `SleepThreshold` (0.15 m/s by default), below which the library calls a body at rest.
The split is a contact's (one manifold: its points `i` share a normal `n`), not a point's:

```text
v_i     = closing speed at point i along n
blow    = n · min(Σ_i J_i·n, Σ m_i v_i over the points with v_i above the rest speed)
blow_i  = blow × J_i·n / Σ_j J_j·n
press_i = (J_i − blow_i) / dt
```

`m_i` is the solver's effective mass of the body along `n` at point `i`.
The solver's split of one contact's momentum among its points is its own, so a cap point by point reads short: **MEASURED** on a 0.4 m brick cube striking house_1's wall square on at 3 m/s (fixture data), 106, 67, 112 and 67 N·s through four corners whose closing momentum is 88.6 N·s each, which reads 88 % of the cube's 354 N·s capped point by point and 99.5 % capped over the contact.
A ram's first tick is a blow plus that tick's engine press; a parked push is a press.

A body at rest against a structure (a pile on a wall, or a body Avian's `ConstantForce` holds there, which wakes nothing) leaves the solver when it sleeps, and the solver moves nothing through its contact.
The reading keeps the points of the last step it did move them, once, while Avian holds the pair sleeping and touching, and drops them when the pair wakes or ends.
Their press stands on the structure: every run of the laws on it carries the press once, and the press starts no run, so rubble asleep on a wall costs nothing until something else loads the wall.

Contacts reach the laws with their blow and press at each point and one source mass per contact: the body's effective mass along the normal at the blow's centroid, the same on each of its points.
A plastic share is linear in the blow, so the points of one contact then take its share once; read against each point's own effective mass they would each spend the receiver's whole mass (**DERIVED**: a cube's corner holds a quarter of the cube's mass along its face's normal, so a cube striking square on would hand a held group up to four times its exchange).
The structural energy check reads the pushing pair's work (`Origin::work_share`) against the held hinge mass; rest lift and slide read it against the translating group's mass.
A freed group launches against its whole occupied hulls’ mass and inertia, and the source receives the rest at the world contact in the same post-physics law pass, before the next solver step.
A body blow on dust returns whole: dust has no dynamic carrier. A held chunk returns nothing; the failing tick's press stays with the standing structure. A queued contact whose chunk an earlier deposit already freed also returns whole: that chunk launched as its own body, the next physics step's solver meets it as a dynamic body, and keeping the static stop would take the source's momentum into no body.
The strength check reads each press alongside each chunk's gravity force, at their respective points, from pressed chunks and changed supports. Navier tension uses the resultant force and moment; rests require compression, a pressure centre inside the bearing hull, and tangent force within friction. For non-coplanar bearing discs, the pressure-centre calculation uses their area-centroid height (**ASSUMED**, the existing horizontal bearing-hull approximation).
An unchanged set of presses (chunk, point and force) on unchanged topology reuses its held verdict. A changed force or point rechecks, including a decrease or removal (the former loaded chunk remains a check seed): reducing an opposing stabilizing force can expose another load's demand, so a componentwise monotonicity assumption is unsafe. Awake presses join every deposit in the physics step once, including deposits before and after their own contact deposit. Sleeping presses still start no run; they join every run another deposit starts.
**Built: punch rings and the two-asymptote rule** (`fracture::ring`, `ring_failure`, `ring_force_failure`, `Reading`).
- Candidates: the loaded chunks, then those grown by one and two rings of neighbours through intact bonded joints, each bonded component a candidate unless it holds a buried chunk or an intact anchor. Rings compete with the sections on the same ratio, the worst first, in both the energy check (blows) and the strength check (gravity and presses).
- Mechanism: a rigid translation along the summed load, projected off every joint it would press the group into. Each intact seam of the ring opens by `⟨−d·n⟩` and slides by `|d − (d·n) n|`; strength `Σ A (f_t ⟨open⟩ + c ⟨slide⟩)`, work `Σ A (G_I ⟨open⟩ + G_II ⟨slide⟩)`. The friction the group's own weight would clamp a seam with is **ASSUMED** nothing (no static solve; it errs toward failure).
- Energy: each blow along the translation by its pair-work factor against the group's mass, against `√(2 M W)`. Strength: the force along it, the group's weight included, against the ring's strength.
- A body's blow meets both limits on every section, ring and rest: its least mean force `blow / step` (the solver's step, `ContactTransfers::dt`) must beat the strength beside gravity and the presses, and its pair work must pay the energy. A blast's blow keeps `g(r)`; a round's line keeps the energy limit alone.
- A seam whose data states no shear pair (`Shear::OpeningOnly`: every `Stated` attachment kind, the nailed fixing among them) answers by its opening law alone: no ring forms across it, and the section, push and rest laws read it as before. Whether a connection model (nail capacity) supplies one is open.
- **MEASURED baseline, before the pushing-work term; not re-measured under that term:** on the shipped house (unit scale, gravity off in the harness; fixture body: a 0.4 m cube of hull mass, 46.8 t, its face square on a flat patch of the outer wall; `breakage` test harness at 64 Hz): a steady push settled in contact holds at 118.5 kN and first breaks the wall at 118.75 kN, two chunks punched out through ten joints, the breach growing to seven chunks over the next 32 ticks of the same push. A ram holds at 0.37 m/s and breaks it at 0.38 m/s (19 chunks out, the body going on at 0.377 m/s); a ram under the body's rest speed (0.12 and 0.14 m/s) is read wholly as press, `m v / dt`, and breaks four chunks by strength, while 0.16–0.37 m/s is a blow its share cannot pay: the verdict is not monotone in closing speed.
- **Built: pushing work.** The source's continued pushing work joins the freed group's own
  energy on every section, ring and rest. The derivation and fixture evidence live once in
  `.agents/docs/design/construction-destruction-sweep-2.md`, "Pushing work in the energy limit".


**Two limits on every section.**
A force must beat the section's strength: the weight check generalised, gravity one force beside each press and each blow's least mean force `blow / dt`.
A blow must also pay the section's energy, its pair work reaching the section through the modal mass below.
A steady push meets the strength limit alone, so its verdict does not depend on the tick rate.
The structure is rigid-plastic: a press below strength moves nothing and is read again each tick it persists; sub-threshold blows on a resting part are each read alone and never add (accepted).
A blast's blow meets both at once through `g(F_R / F_peak)` ("Standoff blast", Law 3): a far burst whose peak force the section's strength holds fails it by no impulse, however large (**DERIVED** by rerunning the #237 study's disc-wall arithmetic on the sourced rows: an 88 mm HE burst's peak force on house_1's east wall stays over its base section's strength out to the burst's 23.2 m reach, so `g(r)` trims the impulse limit only from 15.0 to 13.8 m; the #237 study's ~11 m cut-off came from the misrecalled far row). A round's line keeps the energy limit alone (**ASSUMED**: its blow is over before a section moves).

The disc-wall arithmetic above retains the prototype's no-gravity section capacity; it is not a
prediction for the current hinged, gravity-loaded wall. The shipped-wall measurement and its
negative study-burst result are recorded in `.agents/scratch/blast-b3-far-topple.md`. That report
also preserves a non-monotone production-sampler comparison; the monotone fixed-face section-law
fixture does not discharge that surface-representation limitation.

**Body contact duration.**
`Origin::Body::step` is the numerical physics step, not a material contact duration.
`Load::mean_force` divides the solver's transfer by that step: a lower bound on the contact's
peak force, not a measured peak. It is sufficient to demonstrate strength exceedance, but
cannot demonstrate that a contact whose mean is below strength never exceeded it. For a fixed
blow and inertia, changing the physics rate can change the strength verdict; steady force
inputs remain rate independent because their per-step transfer scales with the step.
The contact model has no compliance or separation-time input from which to recover a physical
peak. Replacing the bound requires that missing material/construction truth, not a fixed
contact-time constant. The rate-sweep evidence is in
`.agents/scratch/blast-b3-collision-review.md`.

**Structures stay solid to the physics engine (option B).**
Avian stops a body against a standing structure as against any static body, and the laws judge every joint from the contact's real transfer.
A section's energy check includes the pushing work (`construction-destruction-sweep-2.md`, "Pushing work in the energy limit"), using the group's effective mass about its hinge; a field's blow reaches it whole.
A group that breaks free leaves with the plastic share against its free effective mass, `J_launch = blow × m_f / (m_b + m_f)`.
The body gets back `blow − J_launch` at the contact point, so a body that breaks through slows by the momentum the freed group took, no more.
No energy is created (**DERIVED**): the pair would keep `J²/2(m_b + m_f)`, and before the launch each body blow on the freed group is scaled by `√(1 − W/E)` for every energy opening about it (`fracture::debit_work`; the opening read `E = D²/2M` and spent `W = C²/2M`), so the pair leaves with what the work left, the plastic loss `½ m_red v_n²` beside it; the momentum the blow gives up stays with the structure the seams held to.
The launch reads the free mass, never the hinge mass, which grows without bound near the hinge and would create energy.
**DERIVED limit check:** at fixed blow, a source mass tending to zero gives the field's whole blow in both demand and launch; an infinite source mass gives a vanishing share.

Simultaneous departures connected by shared source bodies in one contact deposit share a kinetic budget across constructions; disjoint contacts remain independent.
Each point first takes its free-mass share. Those impulses keep their relative sizes, then one scale caps their combined exchange at the common velocity along that combined impulse.
**DERIVED:** with scale `s`, total kinetic energy changes by `-s W + s² Q/2`, so the plastic minimum is `s = W/Q`, clamped to the interval from zero to one.
`W` is the impulses' work against the sources' closing velocities, less the freed groups' field-driven motion; `Q` sums every freed group's linear and angular response and each source body's response to its **combined** impulse and torque.
The source response uses the solver's recorded inertia, including locked axes. A source is counted once even when it frees several groups; several sources freeing one group count that carrier once.
Fields keep their whole impulse. The cap can only reduce body shares, and the complement still returns at each contact point.
This prevents independent shares from creating energy when simultaneous contacts share bodies. It preserves the isolated-contact formula; it does not re-solve the relative contact distribution chosen by the static solver.

**Rejected: option A**, every piece a finite-mass body joined by breakable springs inside the solver: the compliant-seam reference model below, run in Avian with custom joints.
Its visible result is the same, since masonry does not visibly flex, and no study could bound its cost: **DERIVED** 34,890 bodies and 97,980 joints and anchors with 30 houses in play, from house_1's 1,163 chunks, 3,185 joints and 81 anchors (**MEASURED**, `house_1.construction.ron`).
Reopen it only for a structure whose bending is visible: a swaying timber barn, a sagging bridge.

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

**Walls fail locally by punch rings.**
A punch ring is a patch pushed out through its surrounding joints: the chunks a tick's loads touch, grown by rings through intact bonded joints, each group with no anchor a candidate whose cut set is every live joint to the rest.
Its capacity reads sourced masonry shear strength `c` and mode-II fracture energy `G_II`, refused at load when absent.
Planar sections cannot see a local failure: house_1's running bond leaves no single joint, and no coplanar set short of a whole course, that isolates a chunk under a tank's nose.

**Tanks.**
A contact never doses a tank (decided 2026-10-31): the body takes the opposing momentum, through the solver and the give-back, once.

**Lanes**, in order, each under 90 minutes.
Walls ([#235](https://github.com/vikng-dev/overmatch/issues/235)): 1, the contact transfer reading; 2, one load type in place of the fracture laws' separate sources, pushes and energies, with no change in behaviour; 3, contacts into constructions, split at the rest speed; 4, the reduced mass, launch and give-back; 5, strength under gravity and presses; 6, sourced masonry shear data; 7, punch rings and the two-limit rule, where walls first break under tanks.
Trees ([#236](https://github.com/vikng-dev/overmatch/issues/236)): 8, wood grain and root-plate data; 9, trees as structures; 10, the dynamic trunk.
The blast's crush term is #237's Law 1 ("Standoff blast"); pricing follows the walls.

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
joints crossing them, 324 unknowns and 7,362 factor entries; its factorization costs 179,190
scalar operations and one load column 28,800 (DERIVED loop counts, no timing). fir_1 is one part
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
`K u = f` over its parts, factored by sparse Cholesky ordered by exact minimum degree on part
blocks (`parts::Factor`, from the bonded-factor experiment), once per design at load. Instances
share that factor until their first change; a topology change refactors, which at part scale costs
less than one crack's rank-six downdates would on the chunk graph. Two load columns are solved:
sustained (gravity and presses) and total (plus each blow as its mean force over the tick, capped
by its stated peak). Bonded joints are solved exactly; push-only joints (rests and cracked seams)
join the same linear field with their bulk stiffness, bilaterally, and are re-checked once per
tick. A free component is balanced by its rigid acceleration (`accelerating_frame`, spin adding
centrifugal and gyroscopic loading) and gauged at its first part; the gauge is a frame, never a
support. A solve runs only when topology changed or a part's load moved past `LOAD_TOLERANCE`
(Resolution 1e-5 of the owner's weight, and of weight times radius of gyration for a moment: 100 ×
the 1e-7 contact-reading noise MEASURED in `.agents/scratch/bestofboth/bob-split.md`, which defeats
an exact-equality skip). An owner at rest performs zero work.

**Verdicts, one stage per tick.** No failure starts another solve inside its own tick.

- A bonded interface cracks whole once one of its joints reaches its strength in the solved
  field: sustained normal traction over tensile strength, or shear over cohesion plus pair
  friction on its compression; or the total field over either strength while the pulse adds at
  least the whole interface's `Σ G_I A` or `Σ G_II A` of elastic work. A force-controlled driver
  pays its own separation work. Its joints then bear as push-only; when the field pulls the
  failed interface apart, it separates in the same tick. Traction extremes lie at polygon
  vertices: normal `N/A + (n cross (B + n n^T)^-1 M_b) dot r / A`, tangential
  `F_s/A + T (n cross r)/(A trace(Q))`; circular footprints use their analytical rim extrema.
- A push-only interface (no bond left) bears as one face (ASSUMED): the area-weighted normal and
  the hull of its corners. It separates when its resultant pulls (lift) or when its pressure
  centre `r = (M⊥ × n) / N` (DERIVED) leaves the hull (tip). A face carrying less than Resolution
  1e-9 of the owner's weight is left as it is. While an interface keeps a bond, its bonds decide it
  and its rests bear beside them.
- **Coulomb friction** (owner decision, 2026-11-06). A push-only face whose tangential demand
  passes `μ N` slips (`parts::Slip`). It stays in contact while compressed and carries `μ N`
  against its slip: the field is solved again in the same tick with the face's tangential stiffness
  removed and its saturated friction as a pair of loads at its centre, so the excess moves to its
  parts' other interfaces (a roof board's nails). Friction is read again from each solve and
  re-solved while it moves past `LOAD_TOLERANCE`, within Resolution 16 solves a tick. A slipping
  face separates only when it lifts or tips, or when its part is released. When nothing else holds
  its part tangentially, the stiffness left is singular (a pivot under Resolution 1e-9 of its
  diagonal): the part is a mechanism, the slipping rests of every part it reaches through stiff
  interfaces separate (`Slide`), and connectivity releases it to the physics engine, which owns
  sliding bodies. A slip keeps its direction (ASSUMED) and never re-sticks.
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
outside the set are no edge: their force is the caller's load. It is factored once per element
design at load (`Assessment::kernels`) and again only for a part whose chunks changed, and it runs
only on events through one entry, `Kernel::hold(structure, loads)`: per chunk, the loads beside
gravity (the forces its other interfaces hand it, contacts, presses), returning each joint's
carried wrench, the bonds that crack (tension past `f_t`, or shear past cohesion plus friction on
compression) and the push-only joints the field pulls apart. A landing's seam cracks read the
same entry with the landing's contact loads.

**Holes.** When chunks are removed, each part beside them is solved at chunk scale for gravity
and the sustained forces its other interfaces hand it (`Assessment::holes`). A joint that cannot
hold, mortar tension included, cracks, a pulled one separates, and the part is solved again on
what is left until it holds. The cracks then split the part, and the part law judges the pieces:
a chunk or cluster whose faces lift, tip or slip as a mechanism separates and connectivity
releases it. Arching is not authored: compression runs across the running bond's overlaps
because the bed joints are where the chunks overlap. No masonry flag or per-material rule exists.

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

**The unit house under gravity** (MEASURED, `parts::tests`, observations). Its 71 roof-board rests
lie on the gables' 36.87° slopes, where a board needs friction `tan 36.87° = 0.75` against the pair
friction 0.57 (DERIVED from the slope normal and the shipped wood): every rest slips at load, keeps
bearing, and hands the excess to the boards' nails. The house stands: no joint fails, nothing is
released, and it settles in 7 ticks to zero work. In the settled sustained field the most loaded
bonded joint uses 0.198 of its tensile strength and the most loaded in shear 0.245 of its
cohesion plus friction (margins 5.1× and 4.1×). Its 55 element kernels cost 4,347,583 scalar
operations to factor at load; the largest has 66 chunks, 396 unknowns and 13,230 entries, factors
in 499,530 operations and solves one column in 52,128. A fixture 1 MN press on a wall part's top
fails parts in tension, shear, lift, tip and slip (as mechanisms) and settles in 22 ticks, every
value finite.

**Limits.** The trial is linear and bilateral for one tick, so a lift or tip lags its
redistribution by a tick; a slip is re-solved within its tick. Friction acts at each slipping
joint's centre and keeps its first direction; a slip never re-sticks. Inside a part the kernel's
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
The group leaves with the momentum the load that freed it hands it, once, at carrier creation: a body's plastic share, a field's whole (see Bodies in contact).

A rigid carrier is acceptable while internal relative motion is negligible.
Retain unresolved elastic energy and seam history.
Report any energy discarded when replacing internal motion with rigid motion.

Landing contacts return to the same connection calculation.
An overloaded internal seam splits the carrier.
Children inherit the parent velocity field plus any resolved internal motion.
Splitting conserves mass, momentum and angular momentum; newly released energy comes from stored strain or contact work.

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
its owned chunks. Every deposit, including solver contacts, enters `run_laws`: wear, cavity
cracks, joint equilibrium and connectivity have one implementation. The laws read
the solver's `Position` and `Rotation`, never an eased display transform.

A solver contact is a transient point resting joint to the other physics body. Its normal,
point, signed impulse and press come from `contact_transfer`; each structural side receives
its own opposite reading. A fixed side has zero inverse inertia; a moving side keeps its
finite inertia. The contact's transferred impulse has already changed their motion, so it
gets no second plastic exchange or launch. External fields still apply their own impulse.
There is no landing-fragility factor or switch for a piece's kind.

The part verdict and accelerating-frame reduction are specified once in **Parts decide what
stands**. Every changed owner runs one stage per tick; a topology change or an ended pulse keeps
it active on the following tick, even when no new deposit arrives. Spin changes also wake
assessment. Fixed and moving groups share that path; contacts remain applied loads rather than
invented supports.

A failed seam partitions ownership at the same authoritative tick. `PendingSplit` waits for
that tick's plastic returns to reach their sources, then creates the children before publication.
Children inherit `v + omega cross r` at their actual occupied centres of mass and the parent's
angular velocity; their whole occupied hulls partition the parent’s occupied geometry. Contact hulls are inset independently and do not define mass, inertia or ballistic thickness. This adds no kinetic energy. The existing
fracture-work criterion is a threshold, not a resolved cohesive dissipation/strain model.
The initial departure's plastic exchange remains the shared launch path.

`Piece` carries birth and retirement ticks (protocol revision **60**, a deliberate wire
change shared with the module repair snapshot). A retired parent's final pose remains
replicated. Replicas show it until the shared cursor reaches the split and the replacement
groups have arrived; children wait for their birth tick, and then behind any older piece still
drawing one of their chunks, so each chunk draws once. A moving group owns its chunks alone:
the construction's unbreakable elements stay with the placed construction, and a field's blow on a chunk
it turns to dust moves no group, as on a standing construction. The replacement bodies are server-only, and the client keeps deriving the standing wall's
shape from the ledger. Continued holes remain drawn data and do not change occupied geometry.

The required tests use ASSUMED fixture geometry and seam characterization: a strong landing
opens only the weaker seam, gentle support keeps the slab whole, free fall opens no seam, a
single chunk has none to open, and a resting joint carries compression from either side but
never tension. Real Avian contact and split tests complement the law fixtures. These are
invariant tests, not empirical material or handbook validation. More live bodies and retained
carriers remain for the separate pricing lane to measure.

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
| 11 — landing rigidity | Preserve internal seams and feed landing contacts back into structural response. |
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
| Collision loads | **Decided (2026-10-31, #232):** one load for every source; contacts read from the solver's transfers; two limits on every section; structures solid to the physics engine, a freed group taking the plastic share of the blow (option B; option A rejected); trees as structures; punch rings (see Bodies in contact). |
| Contact-damage scope | **Decided (2026-10-31):** wall material never doses a tank, whether it falls, is pushed or is thrown off a hit; walls still fracture on landing and under pushing loads. |
| Accepted strength-scaled spall | **Decided (2026-10-31):** wall ejecta are debris with no dose; the tanks' strength-scaled spall cone stays (see Spall as ejecta). |
| Three-property material model | **Provisional (2026-10-31):** keep the summary, add sourced constitutive and attachment profiles; do not infer missing properties from strength. |
| Hard-pool dose | **Provisional (2026-10-31):** exclude outgoing kinetic energy from absorbed damaging work; retain the shared conversion and abstract crew line rate. |
| Rubble rest | **Decided (2026-10-31):** Avian's island sleep and wake on contact; no body-type switching. The game adds only the wake Avian lacks, when a support goes (see Rubble settlement and the cap). |
| Structural collapse | **Decided (2026-11-05):** at the part scale only; one factored solve per changed owner, interfaces failing whole, one stage per tick; between bodies Avian owns contact (see Parts decide what stands). |
| Debris cap | Cap detailed bodies and use finite-mass overflow; accept that total coarse work still needs a measured limit. No cap applies until a sleeping piece's per-tick cost is measured. |
| Empirical acceptance data | **Provisional (2026-10-31):** require verified source fixtures before declaring handbook validation complete; do not substitute plausible-looking prototype outcomes. |