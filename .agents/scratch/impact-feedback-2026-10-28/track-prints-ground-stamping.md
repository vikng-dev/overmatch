Yes: derive each print from the shoe mesh, preserving its grouser pattern, handedness and actual contact pose.  
Bake a reusable underside height-and-coverage stamp; drive placement from simulation contacts and belt identity.  
Surface response needs separate controls for depth, wall collapse, displaced material and slip smearing; “hardness” alone is insufficient.  
Bekker gives useful broad-rut estimates, but published soil fits require explicit assumptions about moisture, contact length and layer depth.  
Start with visual stamping; make effects on sinkage, resistance and grip a separate, optional simulation project.

## 1. Link-derived stamps

**Decision:** derive the pattern from the authored shoe geometry. No hand-painted print is needed for each tank.

Numerical labels throughout this report mean:

- **MEASURED:** inspected source or asset bytes, or an actual measurement explicitly identified as such.
- **CLAIMED:** reported by a publication or declared in asset data; not independently physically validated.
- **ASSUMED:** a proposed setting, approximation or calculation input.
- **DERIVED:** calculated from stated inputs.

Versions, source identifiers, section numbers and file line numbers are identifiers rather than measurements. This was read-only research against checkout `0b0840647a7e63f26e1f19703ebdf967fb74a424`. No builds, Cargo commands, Blender or GUI tools were run.

### What to bake

Project the shoe from its ground-facing side into its canonical link frame. Record:

- The **lowest solid surface** at each location across the shoe.
- **Coverage**, distinguishing an actual opening from a recessed surface.
- Physical bounds, depth scale and the transform between mesh space and the pin frame.

This is a lower-envelope heightfield, not a photograph of the tread. A grouser protruding farther toward the ground cuts deeper; a recessed part touches only after enough penetration. A real opening remains uncovered.

Bake from the highest-detail shoe geometry, independently of whichever visual LOD happens to be displayed. Cache by source geometry and bake settings. The left shoe can reuse a mirrored stamp when its geometry is genuinely mirrored.

A load-time GPU bake is reasonable. A cached import-time bake is preferable eventually: it avoids repeating the work and makes inspection easier. Either remains presentation data. It must not create rollback state after an asynchronously loaded mesh arrives.

**Limit:** a geometry bake reproduces geometry. Detail represented only in a normal map does not become trustworthy physical relief. Do not turn worn-metal texture noise into soil excavation.

An underside heightfield also loses undercuts and multiple vertical intersections. That is acceptable for ordinary ground-facing shoes. If extreme articulation exposes a visible failure, retain a small contact mesh as the alternative.

### “At its real pose” requires more than rotating a square

The stamp must carry the shoe’s lateral position, yaw, pitch and roll. A yaw-only projection would make every print equally deep across a cross-slope and would misrepresent a shoe crossing an edge.

Reconstruct the baked surface in the shoe frame, transform it by the shoe pose, then intersect/project it against the receiver’s terrain surface. Coverage must be clipped by actual contact. A shoe partly supported on a rock must not print its full underside into nearby soil.

The physics estimate sets the broad indentation target; the underside geometry supplies its local shape. Do not multiply the complete tread silhouette by rut depth: that would make shallow surfaces lose the distinction between protruding grousers and recessed plate.

### What already exists in this checkout

The requested `assets/tanks/` directory is absent. The inspected fixture lives under `assets/tiger_1/`. That path is an asset observation, never a proposed branch in a law.

| Existing input | Source and significance |
|---|---|
| Authored shoe mesh and material | `LinkTemplate` binds the shoe, its mirrored counterpart and its LOD chain in [link_view.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:290). |
| Pin frame and pitch | `Pin_Start`, `Pin_End` and the canonical-frame correction provide placement geometry. |
| Shoe width and radial faces | `DerivedModel` reads these from `Link_Box`; they are not trustworthy RON geometry overrides. See [marker_model.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/marker_model.rs:83). |
| Belt speed and unbounded travel | `TrackDriveSide.speed` and `.phase` already distinguish rolling, wheelspin and a stopped belt. |
| Material-link identity | `phase_decompose` and the rotating station-to-link mapping already preserve which physical shoe occupies a station. |
| Per-contact position and load | `BeltContact.point`, `.load` and `.load_elastic` expose the resolved support result. |
| Longitudinal and lateral slip | `BeltContact.slip` and `.slip_lat` provide the essential inputs for smearing and pivot scuffing. |
| Per-element shear memory | `TrackGripElements` already carries world-space shear and departure state per material link and lateral column. |
| Terrain query | `TrackField` combines the height grid and authored blocks through `TerrainOracle`. |

Python inspection of the GLB gives **MEASURED asset-space** shoe width **0.724714 m**, pin pitch **0.130426 m**, and **1,520 triangles** in the authored shoe primitive. These are fixture observations, not constants for implementation.

There are important differences between what the code exposes and what a stamp needs:

- **The view pose is presented, not raw simulation truth.** `presented_phase` carries and heals phase between simulation samples. `place_links` consumes the conformed view joints. Read [view.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:93) and [link_view.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs:1065). Permanent stamping should use a stable simulation contact stream, with compatible presentation interpolation.
- **The reported contact normal is the belt segment’s inward normal.** In `collect_column_contacts`, `normal = -out`; it is not a terrain hit normal. `TerrainOracle` currently returns directional penetration only. Terrain-aware stamp orientation needs additional information or a terrain sampling adapter.
- **Contact telemetry omits stable identity and footprint area.** The internal `ColumnContact` has element identity, but `BeltContact` drops it. Its production output also lacks the segment endpoints, contact length and lateral patch bounds needed to reconstruct a load-bearing patch robustly.
- **Telemetry is transient.** `TrackContacts` is overwritten each tick and is not rollback state. It is not already a persistent stamping event log.
- **Timing matters.** Contact evaluation uses the pre-update belt phase and speed; `belt_tick` advances phase afterward. Pairing a report with the next phase would shift its shoe association.

These findings follow the implementation in [forces.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/forces.rs:631) and [sim.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs:545).

`gear_phase.rs` already locks rotating running gear to belt travel and link pitch. Stamping should reuse that shared phase convention. It should not introduce another distance counter.

### Missing inputs

The practical additions are:

- A cached underside stamp and its canonical-frame metadata.
- A production contact identifier: entity, side, material-link index and contact episode.
- Contact footprint geometry or effective area, including partial support.
- A surface record and receiver identity.
- A terrain normal suitable for projection.
- A world-space relative-slip vector with explicit sign conventions.
- A small presentation history for swept stamping, deduplication and discontinuity handling.
- A policy for remote vehicles and rollback replay.

The existing lateral slip scalar uses a different sign convention from longitudinal relative slip. Export a correctly constructed vector; do not blindly combine the scalar fields.

## 2. Hardness response

### The physics worth borrowing

**Ground pressure.** Begin with weight divided by supported track area:

\[
p=\frac{mg}{2bL}.
\]

This is an average, not the pressure under every grouser. Uneven support, acceleration, slopes and road-wheel loading redistribute pressure.

**Bekker pressure–sinkage.**

\[
p=\left(\frac{k_c}{b}+k_\phi\right)z^n,
\qquad
z=\left[\frac{p}{k_c/b+k_\phi}\right]^{1/n}.
\]

Here \(z\) is sinkage and \(b\) is the shorter dimension of the equivalent rectangular loading footprint. With pressure in kPa and lengths in metres, \(k_c\) has units kN/m\(^{n+1}\), and \(k_\phi\) has units kN/m\(^{n+2}\).

The useful result is **broad track-rut depth**. It is neither grouser relief nor an amount to subtract every time another shoe crosses the same ground.

**Wong–Reece shear.** Wong and Reece’s wheel-performance treatment combines normal stresses with a shear-displacement model. The familiar monotonic relation is specifically **Janosi–Hanamoto**:

\[
\tau=(c+p\tan\phi)\left(1-e^{-|j|/K}\right).
\]

Here \(c\) is cohesion, \(\phi\) internal friction angle, \(j\) accumulated relative tangential displacement, and \(K\) its characteristic displacement scale. The limiting shear strength comes from Mohr–Coulomb. Wong–Reece is not the name of the exponential relation’s original invention. See their [original wheel-performance paper](https://doi.org/10.1016/0022-4898(67)90105-X).

For prints, borrow **accumulated vector slip**. It explains a crisp rolling impression becoming smeared under wheelspin or lateral scrub. A longitudinal slip ratio alone misses pivot-turn scuffing and behaves poorly around a stationary vehicle.

**DERIVED:** at displacement \(j=K\), the exponential multiplier is **0.632**. This is a characteristic scale, not the displacement at which the maximum is reached.

**Compaction versus displacement.** A depression does not imply an equal-volume berm. Compaction reduces pore volume; displacement moves material elsewhere. Keep a separate displaced-material fraction. USDA’s [soil-compaction guidance](https://www.nrcs.usda.gov/state-offices/illinois/soil-tech-note-16a-compacted-zone-in-soil) supports that distinction. Chrono’s [vehicle-terrain material](https://www.projectchrono.org/assets/slides_3_0_0/5_Vehicle/1_ChronoVehicle.pdf) provides a practical implementation precedent for heuristic boundary build-up.

**Angle of repose.** Loose granular berms should slump when their slope is too steep. USACE gives a **CLAIMED representative range of 30–42°** for cohesionless sediment, depending on size and angularity. This is not automatically the soil’s shear friction angle, and it does not describe temporarily steep cohesive mud or bonded snow walls. See [USACE’s guidance](https://www.hec.usace.army.mil/confluence/rasdocs/rasum/latest/hydraulic-design-hd-calculators/stable-channel-design-functions/stable-channel-design/selecting-an-angle-of-repose-in-cohesionless-sediment).

### Published soil parameters

The following table is **CLAIMED throughout**: published coefficients reproduced in NASA’s *An Overview of Tire-Ground Contact Modeling Approaches for Surface Mobility Applications*, Table 1, printed page 8. NASA attributes the table to J. Y. Wong, *Theory of Ground Vehicles*, Wiley, 2001. These are specimen fits, not universal terrain-class constants. The complete book was not independently inspected for this report. [NASA report](https://ntrs.nasa.gov/api/citations/20250006958/downloads/TM-20250006958.pdf?attachment=true)

| Published specimen | \(k_c\) | \(k_\phi\) | \(n\) | \(c\), kPa | \(\phi\), degrees |
|---|---:|---:|---:|---:|---:|
| Dry sand | 0.99 | 1528.43 | 1.10 | 1.04 | 28 |
| Sandy loam, moisture 11% | 52.53 | 11127.97 | 0.90 | 4.83 | 20 |
| Clayey soil, moisture 38% | 13.19 | 692.15 | 0.50 | 4.14 | 13 |
| Heavy clay, moisture 40% | 1.84 | 103.27 | 0.11 | 20.69 | 6 |
| US snow specimen | 4.37 | 196.72 | 1.60 | 1.03 | 19.7 |

The sensitivity is substantial: even the label “clay” spans very different fitted responses. Moisture, preparation, density and the fitted pressure range matter.

### Sinkage calculation

Use these **ASSUMED calculation inputs**:

- Mass \(m=\) **57,000 kg**.
- Track width \(b=\) **0.70 m**.
- Supported contact length \(L=\) **3.60 m** per track.
- Gravitational acceleration \(g=\) **9.81 m/s²**.
- Equal static loading on level, initially undisturbed ground.

The mass agrees with the inspected asset declaration, but this calculation is a generic example.

**DERIVED arithmetic:**

\[
A=2(0.70)(3.60)=5.04\ {\rm m^2}
\]

\[
W=57{,}000(9.81)=559{,}170\ {\rm N}
\]

\[
p=559{,}170/5.04=110{,}946\ {\rm Pa}
=110.946\ {\rm kPa}.
\]

Every sinkage result below is **DERIVED**, conditional on its stated soil or authored proxy.

| Ground type | Sinkage arithmetic and result | Print character | What earns a visible difference |
|---|---|---|---|
| **Soft mud** | Heavy-clay proxy: \([110.946/(1.84/0.70+103.27)]^{1/0.11}=\) **1.527 m**. Reject this deep extrapolation as an ordinary rut prediction. With an **ASSUMED visual cap of 0.15 m**, the capped target is **0.15 m**. A different published clayey specimen gives \([110.946/(13.19/0.70+692.15)]^{1/0.50}=\) **0.02435 m**. | Cohesive edges can start steep; slip produces broad smearing and pushed ridges. Wet material may ooze back. | Broad depth, glossy/dark disturbed floor, rounded ridges and pivot churn. Fine tread becomes less legible under sustained slip. |
| **Snow** | US specimen: \([110.946/(4.37/0.70+196.72)]^{1/1.60}=\) **0.6856 m** in a sufficiently deep homogeneous layer. An **ASSUMED visual cap of 0.20 m** gives a capped target of **0.20 m**. | Strong compaction, bright broken edges, a compressed floor; fresh impressions can be crisp. | Deep paired troughs, edge shadows and compressed-surface contrast. Snow depth and substrate must limit the result. |
| **Sand** | Dry-sand specimen: \([110.946/(0.99/0.70+1528.43)]^{1/1.10}=\) **0.09206 m**. | Granular walls slump; grousers leave a softened pattern. Slip pushes material into fans and berms. | Rounded rut shoulders, broad displaced material and steering fans. Individual grains earn little. |
| **Grass** | **ASSUMED firm sandy-loam substrate:** \([110.946/(52.53/0.70+11127.97)]^{1/0.90}=\) **0.005930 m**. Root-mat strength is unmodelled. | Mostly flattened vegetation and exposed/darkened soil; a wet substrate can instead rut deeply. | Vegetation flattening and directional strips dominate. A shallow height stamp alone will contribute little. |
| **Firm dirt** | Sandy-loam proxy: the same arithmetic gives **0.005930 m**. | Shallow, relatively crisp grouser marks; modest powder/dust disturbance. | Close-range pattern and roughness changes. Large berms would contradict this selected response. |
| **Gravel** | No universal published fit follows from the name. For an **ASSUMED game curve** supporting **150 kPa at 0.01 m**, with exponent **1.10**: \(0.01(110.946/150)^{1/1.10}=\) **0.007602 m**. | Broken, discontinuous impression; aggregate rearranges instead of accepting a clean cast. | Broad scuffing and displaced stones. A crisp repeated shoe silhouette should be suppressed. |
| **Road** | Bekker is inapplicable to intact paving. Under an **ASSUMED rigid, non-yielding receiver**, permanent indentation is **0 m**. | Surface scuff, dust displacement or transferred mud; no soil berm. | Material disturbance only. Rubber deposition depends on actual pad material; steel shoes should not automatically paint black tire marks. |

The mud and snow caps are **art assumptions**, not validated finite-layer solutions. A physical layered model must account for the supporting substrate and load transfer. Simply clamping a homogeneous-soil result does not establish that physics.

Likewise, grass and gravel are surface descriptions, not sufficient soil specifications. The grass calculation assumes a substrate; the gravel calculation deliberately uses an authored curve.

Do not substitute shoe pitch for track width in the broad-strip equation and call it more accurate. An isolated shoe or grouser has a different footprint, local pressure and calibration scale.

### What the player can see

Use screen-space size to decide which properties survive.

For an **ASSUMED illustrative view** of **1,080 pixels** vertically and **45° vertical field of view**, focal length is **DERIVED 1,304 pixels**. The table gives **DERIVED projected sizes** for features perpendicular to the view direction:

| Camera distance, ASSUMED | Track width 0.70 m | Shoe pitch 0.1304 m | Fine feature 0.01 m |
|---:|---:|---:|---:|
| 10 m | 91 px | 17 px | 1.30 px |
| 25 m | 37 px | 6.8 px | 0.52 px |
| 50 m | 18 px | 3.4 px | 0.26 px |
| 100 m | 9.1 px | 1.7 px | 0.13 px |

Ground-plane foreshortening reduces longitudinal detail further. These are visibility estimates, not measured recognition thresholds. The real camera and optic profile should drive the implementation’s thresholds.

The worthwhile hierarchy is:

- **Far:** paired trail width, route, contrast and broad turning scars.
- **Middle distance:** broad rut shoulders, displaced berms and whether the pattern remains crisp or smeared.
- **Near:** actual shoe/grouser geometry, broken edges and small relief.

Keep depth, edge softness, berm fraction, slip response and disturbance contrast because each changes that hierarchy. Omit independent grain simulation, detailed pore-pressure evolution and separate wall-material parameters unless a capture demonstrates a visible gain.

Recovery earns its cost when the player can watch a wet edge slump or snowfall soften an old trail. A scientifically elaborate recovery model that looks unchanged during play earns nothing.

## 3. What shipped games do

These sources establish different levels of evidence: developer descriptions establish intended behavior; public engine source establishes a mechanism in that source snapshot. Neither automatically proves the complete configuration of every shipping game.

| Game | Public evidence about surface variation | Mechanism and limits |
|---|---|---|
| **SnowRunner / MudRunner** | Saber’s documentation authors mud depth, wetness/hidden mud and snow depth separately. Snow is a layer over a substrate, and underlying hidden mud changes how deeply a vehicle can sink. | The official FAQ says MudRunner’s terrain simulation was ported into SnowRunner’s engine. Current constitutive equations, recovery rules and tread-generation details are not fully public. |
| **Forza Horizon** | A developer’s technical portfolio explicitly lists deformable mud/sand, special snow/sand effects, terrain material blending and dynamic wetness. An environment artist describes painting deep-snow versus frosty areas. | Confirms surface-specific presentation and authored coverage. It does not disclose a physical hardness law, underside-derived tire stamps, berm conservation or recovery equations. |
| **Red Dead Redemption 2** | Rockstar explicitly advertises improved snow trails. A Take-Two patent describes different trail treatment for shallow mud, deep snow, viscous mud and loose gravel. | The patent is a primary description of a related technical approach, **not proof that every embodiment shipped in RDR2**. The complete shipping mechanism is not public. |
| **War Thunder / Enlisted** | Enlisted explicitly announces deformation under vehicles on sand, dirt, snow and similar soft surfaces, while preventing tanks from burying themselves by spinning in place. War Thunder separately documents terrain shaping. | Dagor source provides a concrete material-dependent depth-and-cap mechanism. Shipping game settings and their full relation to collision remain configuration-dependent. |
| **BeamNG.drive** | Official ground models expose friction, resistance, soft-ground depth, fluid-like response and skid-mark behavior by material. | Strong precedent for linking material data to handling and visual effects. This documentation alone does not establish persistent, volume-conserving terrain ruts. |
| **Batman: Arkham Origins** | Its rendering programmer describes generating snow deformation from objects intersecting the snow zone, including sliding and falling. | Direct precedent for geometry-derived marks beyond pre-painted footprints; not a terramechanics simulation. |
| **God of War Ragnarök** | Its rendering programmer describes carving shapes attached to characters and objects, a hard ground beneath the soft layer, and the same framework representing deep snow and deep sand through data changes. | Useful precedent for shared laws plus surface-specific data. The primary testimony does not claim physically calibrated soil behavior. |

### SnowRunner and MudRunner

The strongest documented distinction is **layer depth and substrate**, rather than a cosmetic “mud” texture. Saber’s [mud brush documentation](https://expeditions-guides.saber.games/map_modding/creating_a_map/terrain/geometry_brushes_for_terrain/mud/) distinguishes visible surface treatment from hidden mud affecting passability. Its [snow documentation](https://expeditions-guides.saber.games/map_modding/creating_a_map/terrain/geometry_brushes_for_terrain/snow/) distinguishes snow over solid ground from snow over hidden mud. The [official FAQ](https://community.focus-entmt.com/focus-entertainment/snowrunner/blogs/275-snowrunner-faq) establishes the shared terrain-simulation lineage.

There is also a useful historical primary account: Pavel Zagrebelnyy’s [original SpinTires technical article](https://www.gamedeveloper.com/programming/rendering-and-simulation-in-offroad-driving-game) describes separate depression, pushed-up material and tire-track masks. It explicitly says that deformation texture was GPU-resident and not used for physical simulation. This is evidence about that implementation, not a claim that current SnowRunner works identically.

### Forza Horizon

Developer Aidan Vangrysperre’s [technical portfolio](https://aidanv.com/forza-horizon-5/) explicitly separates deformable mud/sand, other surface effects and wetness. Simon Hughes’s [terrain portfolio](https://simonhughes3d.artstation.com/projects/0XEwB8) confirms authored winter material differences.

The transferable lesson is that deformation shape, surface appearance and weather response can be coordinated without all being outputs of a soil solver. The public sources do not justify inventing a specific pressure–sinkage implementation for Forza.

### Red Dead Redemption 2

Rockstar confirms snow trails in its [PC feature announcement](https://www.rockstargames.com/newswire/article/398a45525ka319/Red-Dead-Redemption-2-For-PC-Now-Available-to-Pre-Purchase-via-the-Roc).

Take-Two’s [graphics-rendering patent](https://patents.google.com/patent/US11534688B2/en) describes shallow, detailed mud trails; deeper snow deformation; repeated filtering to soften trails; and stronger ooze-like treatment for viscous mud or loose scree. It supports the design distinction between crisp impressions and relaxing material. It does **not** name RDR2 in the inspected text, so attributing its exact settings to the shipped game would be an inference.

### Dagor source inspection

The inspected source snapshot is `2baf19472d60b96b437b2f9e549a3a6c8958cfd8`.

In [`deform_hmap.dshl`](https://github.com/GaijinEntertainment/DagorEngine/blob/2baf19472d60b96b437b2f9e549a3a6c8958cfd8/prog/daNetGame/shaders/deform_hmap.dshl):

- `getPhysMat(worldPos.xz)` selects a deformation record.
- That record supplies base vehicle deformation, maximum vehicle deformation and human deformation.
- Vehicle pressure scales the base deformation.
- A height test rejects deformers too far above the surface.
- A maximum limits deformation.
- Separate edge-processing and blur passes shape the result.

In [`render/tireTracks.cpp`](https://github.com/GaijinEntertainment/DagorEngine/blob/2baf19472d60b96b437b2f9e549a3a6c8958cfd8/prog/gameLibs/render/tireTracks.cpp), tracks carry width, texture/material selection, opacity, wetness and a deformation-pressure multiplier.

This establishes a useful generic architecture. It does not establish mesh-derived shoe baking or a full Bekker implementation.

Enlisted’s [first-party announcement](https://www.enlisted.net/en/news/show/636-new-technologies-and-improved-graphics-en/) explicitly names soft-surface variation and the no-self-burial limit. War Thunder’s [terrain-shaping announcement](https://warthunder.com/en/news/7412/current/) establishes broader terrain modification, which should not be conflated with the detailed print mechanism.

### Other useful precedents

BeamNG’s [ground-model documentation](https://docs.beamng.com/modding/levels/level_formats/groundmodels/) demonstrates that grip, sinking, resistance, particle type and skid-mark permission can be data associated with a terrain material. Its soft-ground response should not be assumed to imply persistent geometric excavation.

For geometry-derived interaction, see Colin Barré-Brisebois’s [Batman presentation](https://colinbarrebrisebois.com/wp-content/uploads/2022/06/gdc2014-deformable_snow_rendering.pdf). For shared snow/sand behavior and separation from ordinary gameplay collision, see Paolo Surricchio’s [first-person technical interview](https://80.lv/articles/santa-monica-s-senior-programmer-on-how-god-of-war-ragnar-k-s-snow-system-was-made).

## 4. The proposed design

**Decision:** implement a visual contact consumer with geometry-derived stamps and material-driven response. Keep terrain collision and handling unchanged for the initial slice.

### Stamp format

Use:

- **R16_UNORM:** underside height relative to an asset-defined datum, with physical scale in metadata.
- **R8_UNORM:** coverage.
- Metadata: physical bounds, canonical-frame transform, source hash and bake version.

The format names specify storage precision, not measured performance.

Do not bake a shear channel: slip is runtime state. Add a runtime disturbance channel only if it visibly controls smearing, roughness or vegetation flattening. Persistent shear direction is optional; the emitter can usually rasterize the swept impression without storing a world-sized vector field.

For an **ASSUMED stamp resolution of 128 × 32 texels** over the inspected fixture’s **MEASURED bounds of approximately 0.725 × 0.179 m**, spacing is **DERIVED approximately 5.7 × 5.6 mm**. Height plus coverage costs **DERIVED 12 KiB** before mips, or approximately **16 KiB** with a full mip chain. Mirroring can reuse those bytes.

This is a starting quality choice. The receiver’s world-space resolution can erase detail regardless of stamp resolution.

### When to lay a print

**Use contact-time updates, initialized at touchdown and finalized at departure.**

Touchdown-only stamping misses later loading and slip. Lift-off-only stamping delays the print and fails to represent ongoing churn.

The contact consumer should:

- Begin an episode when a material shoe becomes plausibly supported.
- Reuse contact-membership hysteresis where appropriate.
- Track the strongest supported indentation target during that episode.
- Sweep the shoe footprint through actual relative movement.
- Finalize when departure is definitive.
- Reset history on teleport, respawn or a discontinuous reseed.

A rolling shoe approximately stays still against the ground while the hull passes over it. Therefore, **do not sweep its print along hull travel**. Sweep the shoe’s world contact motion and its relative sliding motion. This is essential to distinguish rolling, wheelspin and a braked skid.

For pivot turns, evaluate each contact location. Opposing lateral motion across the footprint produces broad curved scuffs and material pushed toward the turn’s shoulders. Averaging slip across the entire track can cancel the very motion the player should see.

### Separate broad indentation from local pattern

Use a broad pressure estimate to choose the indentation target. Apply the underside envelope within that target to reproduce shoe relief.

For a first implementation:

- Estimate average pressure from total supported load and effective track footprint.
- Use local contact load to modulate it conservatively.
- Do not divide a column’s load by an arbitrary tiny area.
- Export effective contact length and lateral weights before attempting detailed local pressure.

For stationary repeated contact, approach or retain a target depth. **Never add the same depth every tick.** Repeated identical load should settle, not excavate indefinitely.

Slip may increase disturbance and move material. Any additional slip-induced sinkage needs its own bounded, authored rule; Bekker alone does not supply it.

### A small surface-response table

The following are **ASSUMED prototype settings**, including every numerical entry. They are chosen to produce distinguishable appearances and must be judged in captures. They are not physical material measurements.

“Displaced fraction” is the share of newly removed geometric volume routed to visible berms; the remainder represents compaction or unresolved transport.

| Surface | Depth response / cap | Walls | Displaced fraction | Slip response | Recovery |
|---|---|---|---:|---|---|
| Mud | Selected cohesive response; cap 0.15 m | Initially cohesive, then rounded | 0.50 | Strong smear and pushed ridges | Fine relief relaxes; broad rut persists |
| Snow | Snow response; cap 0.20 m | Crisp compressed edge | 0.15 | Compacted smear with broken shoulders | Persistent without snowfall; snowfall fills |
| Sand | Dry-sand response; cap 0.12 m | Granular slope relaxation | 0.60 | Broad granular fans | Edges slump; broad rut persists |
| Grass | Substrate response; cap 0.02 m | Mostly concealed by vegetation | 0.05 | Flattening; soil exposure under strong scrub | Flattening persists during encounter |
| Firm dirt | Firm-loam response; cap 0.015 m | Shallow and crisp | 0.10 | Modest abrasive scuff | Persistent |
| Gravel | Authored gravel curve; cap 0.03 m | Broken/discontinuous | 0.40 | Aggregate scatter and broad scuff | Local settling; persistent disturbance |
| Road | Rigid response; cap 0 m | No soil walls | 0 | Surface scuff or transferred material | No terrain-height recovery |

Use the underlying material’s appearance for the floor and berm. Wet mud can darken and gloss; compacted snow can change roughness; grass needs vegetation flattening. Arbitrary darkening of every surface would make these settings less distinguishable.

For loose granular slopes, an **ASSUMED initial repose angle of 35°** lies within the cited representative range. Cohesive materials use a separate relaxation policy.

A berm should depend on **newly removed volume**, not cumulative stamp count:

\[
V_{\rm berm}=f_{\rm displaced}\,\Delta V_{\rm newly\ removed}.
\]

Route that material toward the footprint boundary and, under slip, preferentially in the transport direction. Subsequent passes can flatten existing berms.

Recovery should soften fine relief before erasing the broad rut. Weather-driven filling is distinct from mechanical edge relaxation. Avoid universal rapid healing; it can visibly contradict persistent heavy-vehicle trails.

### Where hardness comes from in this repo

The current blend has **MEASURED four material slots**:

- Base: `coast_sand_rocks_02`.
- Mask layers: `dirt_aerial_03`, `coast_sand_05`, `aerial_mud_1`.

These are declared in [terrain_grid.rs](/Users/Yan/github/vikng-dev/overmatch/src/terrain_grid.rs:203) and [terrain_blend.rs](/Users/Yan/github/vikng-dev/overmatch/src/terrain_blend.rs:67). Their names identify visual packs; they do not establish physical soil properties.

**Proposed data:** associate each terrain pack with a `SoilVisualResponse` record containing its indentation curve or preset, depth cap, displaced fraction, wall behavior, slip response and disturbance appearance. Put this in declarative terrain data, including the base pack.

The renderer’s final weights are not simply the painted RGB values. [terrain_blend.wgsl](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl:161) applies AO-dependent bias, a blending window and normalization.

**Decision:** use normalized painted weights for the initial visual response. Treat this as an **ASSUMED blending rule**. It is stable and avoids making soil mechanics depend on scan-texture AO.

Evaluate each response curve at the same pressure, then blend the resulting depths or presentation outputs. Do not arithmetic-average Bekker coefficients when exponents differ: their units depend on the exponent. A weighted curve blend remains an authored visual approximation, not a soil-mixture theorem.

The current pack roster also cannot distinguish every requested surface. Snow, road and grass need appropriate records and receiver assignments when those surfaces exist. Material on authored blocks or roads must override the heightmap’s underlying soil.

### Optional simulation coupling and its cost

Hardness could also affect:

- Support sinkage.
- Compaction resistance and rolling resistance.
- Available shear strength and grip.
- Steering resistance.
- Slip-induced sinkage.

That is optional and materially larger than visual stamping.

The current support law is an envelope/suspension penalty model. Its penetration is **not soil sinkage**. Increasing that penetration or reducing spring stiffness to “make mud” would mix suspension travel with ground yielding.

A simulation extension needs deterministic surface sampling available to the server, a stable physical material map, calibrated layered response, and rollback-compatible state. If deformation changes future contact geometry, that terrain state must also be represented consistently across the simulation. Loaded view meshes and GPU textures cannot initialize it late.

A lower-cost intermediate option is static material-dependent grip and resistance without evolving terrain geometry. Even that requires deliberate integration with the existing per-element grip law; it should not be presented as automatically supplied by the print system.

### Cost and boundedness

The bake is small. Ongoing cost is dominated by contact history, swept footprint writes, touched terrain storage and relaxation passes.

**DERIVED example:** at **ASSUMED belt speed 5 m/s** and **ASSUMED link pitch 0.13 m**, newly arriving shoes occur at approximately

\[
2(5/0.13)=76.9
\]

shoe arrivals per second across both tracks. Sliding contact can require more footprint updates; arrivals alone are not a complete budget.

Process and batch touched regions. Skip unchanged stationary contacts after their target is reached. Use swept footprints or distance-based resampling so fixed-step spacing does not leave gaps.

For illustration, an **ASSUMED active region of 64 × 64 m** at **ASSUMED spacing 0.02 m** contains **DERIVED 10.24 million cells**. A signed height field using **2 bytes per cell** plus disturbance using **1 byte per cell** costs **DERIVED 29.3 MiB**, before ping-pong buffers, mips or history. World coverage and texel spacing matter much more than the shoe atlas.

No runtime timings were measured. A frame-time claim would be unsupported.

Keep cosmetic history outside rollback state, but give it stable tick/episode identities so replay cannot repeatedly deepen or duplicate trails. For remote tanks, contact reconstruction or transmitted contact evidence is a separate integration task: the local telemetry vector alone does not solve it.

The sister rendering lane should choose how the resulting field is displayed. The producer described here supplies signed height, disturbance and material context without requiring a particular display technique.

## 5. Open questions

These are validation items with provisional decisions, not blockers.

- **Does the authored underside contain the desired grouser shape?** Geometry exists and can be baked, but this report did not render and visually certify the resulting impression. Decision: inspect a generated stamp before changing assets.
- **How much depth remains credible with unchanged collision?** Deep visual troughs can make shoes appear to bridge or float. Decision: treat caps as provisional and judge them from the actual camera before expanding depth.
- **Which surface differences survive ordinary play?** Decision: capture straight travel, wheelspin, a braked skid and a pivot on contrasting surface presets, using identical lighting and camera conditions.
- **Does grass flattening exist as a receiver effect?** Decision: keep grass response modest until vegetation can consume disturbance; a height-only print will not sell flattened grass.
- **How should contact data reach remote-vehicle presentation?** Decision: design the production contact record and replay deduplication before promising persistent multiplayer trails.
- **Should trail history survive travel or reload?** Decision: bound the initial presentation cache spatially; persistence is a separate product choice with storage cost.
- **Are physical soil fits worth carrying into runtime?** Decision: retain their provenance in research and data, but allow a compact authored response curve where it produces the same visible result.
- **Does any extra property earn a visible difference?** Decision: remove a parameter when matched captures cannot distinguish its effect. No separate brittle-shear, grain-scale or hydrology model is justified for the first slice.

## 6. Sources

**Terramechanics and soil response**

- J. Y. Wong, *Theory of Ground Vehicles*. [Publisher’s current book description](https://www.wiley-vch.de/en?isbn=9781119719700&option=com_eshop&view=product). Numerical coefficients here come from the explicitly attributed older-edition table reproduced by NASA, not from an independently inspected complete book.
- Schepelmann et al., [*An Overview of Tire-Ground Contact Modeling Approaches for Surface Mobility Applications*](https://ntrs.nasa.gov/api/citations/20250006958/downloads/TM-20250006958.pdf?attachment=true). Table 1, printed page 8; Wong attribution in reference 37.
- Wong and Reece, [*Prediction of rigid wheel performance based on the analysis of soil-wheel stresses, Part I*](https://doi.org/10.1016/0022-4898(67)90105-X).
- Project Chrono, [terrain-model documentation](https://api.projectchrono.org/vehicle_terrain.html) and [vehicle-terrain presentation](https://www.projectchrono.org/assets/slides_3_0_0/5_Vehicle/1_ChronoVehicle.pdf).
- USDA NRCS, [*Compacted Zone in Soil*](https://www.nrcs.usda.gov/state-offices/illinois/soil-tech-note-16a-compacted-zone-in-soil).
- USACE, [*Selecting an Angle of Repose in Cohesionless Sediment*](https://www.hec.usace.army.mil/confluence/rasdocs/rasum/latest/hydraulic-design-hd-calculators/stable-channel-design-functions/stable-channel-design/selecting-an-angle-of-repose-in-cohesionless-sediment).

**Shipped-game evidence**

- Saber/Focus, [SnowRunner FAQ](https://community.focus-entmt.com/focus-entertainment/snowrunner/blogs/275-snowrunner-faq), [mud authoring](https://expeditions-guides.saber.games/map_modding/creating_a_map/terrain/geometry_brushes_for_terrain/mud/) and [snow authoring](https://expeditions-guides.saber.games/map_modding/creating_a_map/terrain/geometry_brushes_for_terrain/snow/).
- Pavel Zagrebelnyy, [*Rendering and simulation in offroad driving game*](https://www.gamedeveloper.com/programming/rendering-and-simulation-in-offroad-driving-game).
- Aidan Vangrysperre, [Forza Horizon technical work](https://aidanv.com/forza-horizon-5/); Simon Hughes, [Forza Horizon terrain work](https://simonhughes3d.artstation.com/projects/0XEwB8).
- Rockstar Games, [RDR2 PC graphical enhancements](https://www.rockstargames.com/newswire/article/398a45525ka319/Red-Dead-Redemption-2-For-PC-Now-Available-to-Pre-Purchase-via-the-Roc).
- Take-Two, [*Method and apparatus for enhanced graphics rendering in a video game environment*](https://patents.google.com/patent/US11534688B2/en). Technical disclosure; exact shipping-game attribution remains unestablished.
- Gaijin/Darkflow, [Enlisted vehicle-track announcement](https://www.enlisted.net/en/news/show/636-new-technologies-and-improved-graphics-en/); Gaijin, [War Thunder terrain shaping](https://warthunder.com/en/news/7412/current/).
- Dagor source: [`deform_hmap.dshl`](https://github.com/GaijinEntertainment/DagorEngine/blob/2baf19472d60b96b437b2f9e549a3a6c8958cfd8/prog/daNetGame/shaders/deform_hmap.dshl), [`dynamic_render_deform_hmap_inc.dshl`](https://github.com/GaijinEntertainment/DagorEngine/blob/2baf19472d60b96b437b2f9e549a3a6c8958cfd8/prog/daNetGame/shaders/dynamic_render_deform_hmap_inc.dshl), and [`render/tireTracks.cpp`](https://github.com/GaijinEntertainment/DagorEngine/blob/2baf19472d60b96b437b2f9e549a3a6c8958cfd8/prog/gameLibs/render/tireTracks.cpp).
- BeamNG, [ground-model documentation](https://docs.beamng.com/modding/levels/level_formats/groundmodels/).
- Colin Barré-Brisebois, [*Deformable Snow Rendering in Batman: Arkham Origins*](https://colinbarrebrisebois.com/wp-content/uploads/2022/06/gdc2014-deformable_snow_rendering.pdf).
- Paolo Surricchio, [first-person account of Ragnarök’s snow system](https://80.lv/articles/santa-monica-s-senior-programmer-on-how-god-of-war-ragnar-k-s-snow-system-was-made).

**Repository evidence**

- Track simulation and telemetry: [forces.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/forces.rs), [sim.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs), [oracle.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/oracle.rs), [terrain.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/terrain.rs).
- Track presentation and geometry: [link_view.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/link_view.rs), [view.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs), [gear_phase.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/gear_phase.rs), [marker_model.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/marker_model.rs), [rig_geom.rs](/Users/Yan/github/vikng-dev/overmatch/src/track/rig_geom.rs).
- Terrain materials: [terrain_blend.rs](/Users/Yan/github/vikng-dev/overmatch/src/terrain_blend.rs), [terrain_blend.wgsl](/Users/Yan/github/vikng-dev/overmatch/assets/shaders/terrain_blend.wgsl), [terrain_grid.rs](/Users/Yan/github/vikng-dev/overmatch/src/terrain_grid.rs).
- Asset bytes inspected using Python: [shoe GLB](/Users/Yan/github/vikng-dev/overmatch/assets/tiger_1/tiger_1.glb); declared vehicle data: [tank RON](/Users/Yan/github/vikng-dev/overmatch/assets/tiger_1/tiger_1.tank.ron).