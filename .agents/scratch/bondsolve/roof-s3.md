# Roof facts and owner options

Subject: the video game's environment destruction model, game state only.

## What is authored

`tools/constructions/house_1.house.ron` is the construction input; the admitted baked
`house_1.construction.ron` agrees with it. DERIVED from the input and element hulls:
there are 13 boards on each slope, running along Z, each 6.6 m long. Ordinary boards
are 0.2 m wide and 0.05 m thick; the ridge board has a mitred profile. Their grain is
explicitly `Grain((0, 0, 1))`. The two wall plates also run along Z. There are no rafters,
purlins, ties, battens or discrete board clips in this construction.

The board undersides contact the two brick gable rakes. Those interfaces are `resting`.
Boards reaching the eaves contact their wall plate through `nailed` interfaces. Every
touching pair of board edges, including the ridge pair, is also `nailed`. Within each
board the baked chunks use the timber's intrinsic seam.

The attachment's density was DERIVED for **two skew nails per rafter at a wall plate**,
not a connection along the thin edges of two parallel boards. Applying that density to
board-to-board edges is not evidence of how those boards are assembled. An edge nail
would also need edge distances, penetration and lateral/withdrawal orientation that the
current construction never states.

## Comparison with timber construction

[American Wood Council's rafter description](https://awc.org/resources/wood-products/roof/lumber-rafters/)
places the roof load on rafters connected to wall plates and the ridge.
[APA's roof installation guidance](https://www.apawood.org/buildertips/pdfs/N335.pdf)
fastens sheathing to framing at supported edges and intermediate supports. Its panel
edge clips are not a continuous nailed seam between neighbouring sheet edges. This
panel guidance establishes the distinction between a fastening into supporting framing
and a free edge; it is not a nail schedule for the game's solid boards.
[PNNL's gable-overhang guide](https://basc.pnnl.gov/resource-guides/framing-gable-roof-overhangs)
shows ladder or outrigger framing carrying the deck and verge. A gable rake can bear a
local board reaction; it does not supply the missing intermediate framing along a roof.
The modern overhang guidance is a load-path comparison, not a historical identification
of this house.

**Decision:** neighbouring plain board edges should not inherit `nailed` merely because
they touch. A deliberately engineered edge-fastened deck would need its own construction
and connection facts. The lane leaves the house input and bake unchanged for the owner.

## Options for the owner

| Option | What it needs | Appearance and implementation cost |
|---|---|---|
| Retain the long boards; add supporting rafter frames | Rafters across the slope at authored spacings, their plate/ridge connections and a way to carry or tie their horizontal reactions; board-to-rafter fastenings, plain board edges | Keeps the external planked silhouette; visible framing inside a broken roof. Adds elements, joints, authoring and a rebake. Framing spacing/section needs an engineering choice. |
| Use boards down the slope on purlins | Rotate the deck's board layout and grain; add purlins spanning between declared supports, with their gable/rafter bearings and connections | Changes the visible seam direction and debris shapes. More geometry and a broader rebake; supports and member sections still need sizing. |
| Keep an intentionally self-spanning plank deck | A documented deck/edge-connection system, its actual fastening arrangement, slip, directional timber response and edge distances; an adequate eaves restraint | Least external visual change; greatest connection-data burden. Calling all contacts `nailed` does not establish this system. |

No option is implemented by raising a strength, gluing a resting rake, or seeding a settled
topology. Removing edge nails alone would leave the downslope load path unresolved:
DERIVED `tan(theta)=3/4`, exceeding the authored pair friction
`sqrt(0.5 * 0.65)`. A correct replacement must explain where that reaction goes.

## Fixing compliance and grain

[Swedish Wood, Design of Timber Structures, volume 1 (2022), section 4.16, equation 4.35](https://www.swedishwood.com/siteassets/5-publikationer/pdfer/sw-design-of-timber-structures-vol1-2022.pdf)
transcribes EN 1995-1-1 section 7.1: for non-predrilled nails,
`K_ser = rho_m^1.5 * d^0.8 / 30` N/mm per shear plane per fastener, with density in
kg/m³ and diameter in mm. This is a **lateral** slip modulus. It does not measure axial
withdrawal stiffness, a strength, or fracture work. The same section distinguishes the
ultimate-state approximation `K_u = 2 K_ser / 3`; the solver uses the service slip slope
as an elastic compliance and tests strength separately. DERIVED at the declared reference
448 kg/m³, 3.1 mm and `2 / 0.6 / 0.3125` nails/m²: 781.421 N/mm per nail and
8,335,158.159 N/m³ of interface area. Axial withdrawal uses an explicit Unknown lateral
analogue; no axial stiffness measurement is claimed.

[USDA FPL Wood Handbook (2021), chapter 5, table 5-1](https://www.fpl.fs.usda.gov/documnts/fplgtr/fplgtr282/fpl_gtr282.pdf)
distinguishes longitudinal, radial and tangential moduli and shear moduli. For Sitka
spruce it gives `E_T/E_L=0.043`, `E_R/E_L=0.078`, `G_LR/E_L=0.064`,
`G_LT/E_L=0.061`, `G_RT/E_L=0.003` (MEASURED tabulated elastic ratios at approximately
12% moisture). These are not interchangeable with a tangential compression modulus.
The game's `Wood` row currently uses a European-spruce tangential compression modulus
as a scalar for every seam direction. A transverse tensile strength is likewise not an
along-grain tensile strength; a bending modulus of rupture is not an axial tensile test.
Directional constitutive data must keep those distinctions and species/moisture domains.
The authored surface frame supplies a longitudinal axis, but not growth-ring orientation.
Every house board and both plates declare Z grain; the shipped tree trunk declares Y grain.
Directional timber remains open unless it is implemented and verified in the session report.

For a consistent graded-timber comparison, [Swedish Wood's transcription of SS-EN
338:2016, table 1](https://www.traguiden.se/konstruktion/dimensionering/hallfasthet-och-barformaga/karakteristisk-hallfasthet/konstruktionsvirke2/)
states C24 mean moduli of 11,000 MPa along and 370 MPa across grain, and characteristic
tensile strengths of 14.5 MPa along and 0.4 MPa across grain (MEASURED transcription of
standardized class values, not measurements of the game's boards). The game's roof has
not been declared C24. These class strengths cannot replace its clear-wood averages merely
to obtain a different verdict, and the 24 MPa bending class is not axial tensile strength.
