# Wall destruction: model v2

Subject: Overmatch’s environment destruction and tank damage model, game state only.
Crew and modules are HP pools; dose can leave them reduced or their HP pool emptied and knocked out.

Design for [#189](https://github.com/vikng-dev/overmatch/issues/189), building on [#188](https://github.com/vikng-dev/overmatch/issues/188).
This document owns the environment model.
[The tank damage design](armor-penetration-and-damage.md) owns the shared penetration, spall and burst narratives; amendments below state only what changes.

## Evidence and status

**PROPOSED** means the implementation choice recommended here.
**OWNER DECISION** marks a change to an accepted decision or an unresolved product choice.
Proceed with the recommendations when prototyping; they are not recorded as owner acceptance.

Every physical value below is **CLAIMED**, **DERIVED** or **ASSUMED**.
CLAIMED handbook values and source attributions are recalled knowledge or claims present in the supplied code, without external verification.
No new MEASURED results are asserted.
Equation coefficients, unit conversions, indices, issue numbers and version numbers are mathematical identities or identifiers.

The supplied inputs do not contain a verified contact-charge breach example or an AP crater-size dataset.
Those empirical acceptance cases remain evidence gates; analytical predictions must not be presented as measurements.

## Principles

**Use the cheapest model whose differences cannot affect play.**
Bake convex blocks instead of simulating individual bricks.
Resolve actual holes, support capacity and finite-mass rubble because rounds, sight and tanks expose those differences.
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
advance(layout, state, timestamped_loads, contacts, interval)
    -> state_delta, geometry_edits, body_transfers, receiver_deposits, audit
```

The interface requires SI units, stable identities, event ordering and an explicit source budget.
Its implementation owns local response, structural response, support assessment and representation changes.
Callers do not decide which chunks fail.

Build authoritative state synchronously from baked data at spawn.
Lazy allocation may expand that state; loaded visual assets never initialize it.

An intact element remains one collision proxy until its first relevant load.
Resolve that load against the underlying geometry before atomically replacing the proxy.
A subsequent event must see the replacement geometry, including holes.

Extend #188’s `Swap`, `Dust`, `Crack` and `Fall` record with:

- Local geometry edits, identified within a baked chunk.
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

### Rams and other contact

Use transferred contact force over time:

```text
J_contact = ∫ F_contact dt
Δp_tank = -Δp_structure
```

Advance structural displacement and rotation throughout sustained loading.
A below-threshold tick is not forgotten motion.

**Inputs.**
Mass, inertia, contact geometry, friction and the connection profiles determine response.
Use the solver’s net transferred impulse, not a sum of retained iteration accumulators.

**Stand-in.**
Rigid-block contact is adequate where local indentation cannot change passage or failure.
Read or instrument the actual momentum transfer before adapting Avian’s contact output.

**Server cost.**
**DERIVED `O(C)`** transfer extraction for `C` contacts, plus structural solution cost.

**Tanks.**
Tanks receive the opposing momentum once.
Contact creates no tank HP dose under the recommended interpretation of the owner’s exclusion.
Trees eventually use this same force-history response.

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
Subtract only material that actually vacates the original occupied region.

Depth refinement must permit a rear layer to separate while the front survives.
Choose refinement from convergence of aperture, residual speed, rear breakout and seam loading.
Baked chunk width is not the minimum hole diameter.

The visual surface, ballistic query and physical contact geometry consume the same surviving geometry.
Repeated shots through an existing channel encounter only remaining matter.
Recompute affected mass, centroid, inertia and connection polygons after removal.

**Stand-in.**
Use clipped convex remnants and a local refinement hierarchy.
Keep ordinary baked blocks outside the affected patch.
Accept simplification only when it preserves admitting projectile size, sight and load-bearing sections.

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

## Structural calculation

Use rigid blocks with distributed, compliant seams and finite mass.
Face quadrature transmits force and bending moment.

```text
M q̈ + C q̇ + f_internal(q, history) = f_applied + f_gravity + f_contact
```

Build seam stiffness from bulk compliance and representative lengths:

```text
k_n ∼ E'/ℓ
k_s ∼ G/ℓ
```

Verify the assembled stiffness with uniform-strain and bending patch checks.
The representative length apportions bulk compliance; it is not a physical flaw size.

Use **ASSUMED zero additional viscous damping** initially.
Friction, crushing and cohesive softening already dissipate energy.
Add sourced damping only if an omitted dissipation mechanism is demonstrated.

### Connection law

Take opening as positive normal separation.
Each connection retains irreversible history.

```text
opening initiation:       t_n⁺ = f_t
sliding limit:            |t_s| ≤ (1-d)c + μ p
compression:              p ≥ 0, governed by bearing/crushing response
fully separated opening:  t_n = 0
```

Compression remains possible after cohesive failure.
Friction acts only while faces are in contact.
Bearing-only attachments begin without tensile cohesion.

Use bilinear cohesive softening.
For pure opening:

```text
δ_0 = f_t/k_n
δ_f = 2 G_I/f_t
∫ t_n dδ_n = G_I
```

Use the corresponding shear envelope with `c`, `k_s` and `G_II`.
For mixed loading, adopt an **ASSUMED linear fracture-work interaction**:

```text
W_open/G_I + W_slide/G_II = 1
```

`W_open` and `W_slide` are cohesive work, excluding frictional dissipation.
Use an energy-consistent damage update under changing mode mix.
Reject a discretization whose elastic onset lies beyond its specified softening endpoint.

Crushing uses the assembly’s compressive response and permanent shortening.
Local patches replace the corresponding coarse material response; they do not charge its compaction work again.

**Inputs.**
Use the material and attachment table.
No physical failure threshold depends on baked chunk size.

**Stand-in.**
Distributed seam compliance replaces unresolved deformation inside blocks.
It is acceptable only after stiffness, failure load and dissipated fracture work converge across cuts.

**Server cost.**
**DERIVED `O(IN_j)` per active substep**, from `I` iterative sweeps over `N_j` connection samples.
Iteration count and active-region size must be measured.

**Tanks.**
Only contact response changes.
This calculation does not replace tank module HP pools.

### Failure, redistribution and support

Establish gravity equilibrium before applying an event.
Solve all connections in the active load path, including corners, roofs and foundations.
Advance failure, redistribute load and continue until equilibrium or dynamic motion is resolved.

A path to ground identifies connectivity.
It does not establish sufficient bearing, sliding resistance or bending capacity.

Connectivity identifies detached groups after connection assessment.
A failed base can leave a wall rocking on unilateral contact.
Toppling follows gravity and motion, not a fracture-energy threshold with a tipping surcharge.

Activate the connected load path until the omitted region’s reaction error is below the declared tolerance.
Do not restrict failure to individual seams or coplanar cut sets.

### Force versus impulse

For a relevant structural mode:

```text
T = 2π √(m_eff/k_eff)
```

Use an impulse approximation only when load duration is short relative to every mode needed for the requested outcome.
A provisional screen is **ASSUMED `τ/T ≤ 0.05`**.
Confirm it by comparing resolved loading with an impulse applied at the load’s time centroid.

Local cracking still resolves short stress pulses.
A globally impulsive event need not be locally quasistatic.
Longer loads retain their force history and evolving displacement.

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

**Tanks — OWNER DECISION.**
Recommend replacing the accepted strength-scaled spall rule in tank-design section **5**.
Strength influences fracture and ejection through the response; it is not an independent energy multiplier.

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
Transfer the velocities already produced by structural response.
Do not add the initiating impulse again at carrier creation.

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
Supported equilibria may leave detailed dynamics while retaining geometry, material, mass and wake dependencies.
A rest timer is debounce, not proof of support.

**Server cost.**
**DERIVED `O(N_s + E_s)`** for an affected support graph, plus approaching-contact queries.

**OWNER DECISION — cap semantics.**
Recommend capping detailed Avian bodies, not declaring excess matter static.
The inherited **ASSUMED cap of 256 detailed bodies** is a resource setting, not a physical constant.

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

**Structural calculation next.**
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
| 7 — sustained pushing | Force-history integration and net transferred contact impulse. |
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
| Small-hole fidelity | Let sub-chunk perforations pass sight and later rounds through shared geometry. |
| Rubble persistence and mobility | Retain gameplay-relevant occupied geometry throughout the match; let mass, friction and track contact determine climbing or pushing. Fine presentation debris may fade. |
| Contact-damage scope | Exclude tank HP dose; allow structural fracture on landing and under pushing loads. |
| Accepted strength-scaled spall | Replace it with budgeted ejecta for walls and tanks; validate the changed tank outcomes explicitly. |
| Three-property material model | Keep the summary, add sourced constitutive and attachment profiles; do not infer missing properties from strength. |
| Hard-pool dose | Exclude outgoing kinetic energy from absorbed damaging work; retain the shared conversion and abstract crew line rate. |
| Debris cap | Cap detailed bodies and use finite-mass overflow; accept that total coarse work still needs a measured limit. |
| Empirical acceptance data | Require verified source fixtures before declaring handbook validation complete; do not substitute plausible-looking prototype outcomes. |