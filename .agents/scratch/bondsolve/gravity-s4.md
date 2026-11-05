# Scale and convergence diagnosis, session 4

Subject: the video game's environment destruction model, game state only.

## Measurement boundary

MEASURED by the ignored `gravity_placed_scales` instrument: every shipped placement starts
from its admitted intact equilibrium and advances the production law, without external loads.
The before and after runs use the same materials, attachments, roof, map and 32-round budget.
These are law-only observations, not captures, prices, or an oracle that the asset must stand.
Departed chunks are removed from the standing owner; subsequent rigid-body landings belong to
the dedicated-server proof. Raw logs are `target/bondsolve-s4-diagnosis.log` and
`target/bondsolve-s4-fixed-census.log` in the lane's bay.

## Scale and first cracks

The [before](scale-s4-before.csv) and [after](scale-s4-after.csv) tables list all 74 houses,
their scales, independently predicted homogeneous first-crack counts, measured counts,
judgement residuals, first eight numerical ticks, and state after 128 law ticks. A row stops
when its owner is equilibrated and unchanged; a single zero in the stage column records that
idle state, which spends no later sparse rounds.

| Scale interval | Houses | Initial cracks per house | Total initial cracks |
|---|---:|---:|---:|
| 0.5492 to 0.75 | 20 | 0 | 0 |
| 0.75 to 1 | 19 | 0 | 0 |
| 1 to 1.25 | 19 | 2–7 | 84 |
| 1.25 to 1.4636 | 16 | 8–15 | 173 |

MEASURED: largest clean placed scale 0.999503016; smallest cracking scale 1.024615049.
The separately admitted unit scale is clean. All 257 initial failures are shear at `nailed`
board-to-board edges on the western roof slope. No initial failure is mortar, intrinsic timber,
a resting gable bearing, or a ground attachment. The following table gives the complete set
of first-stage failed source addresses; use is demand/capacity. Coordinates are unit-scale
joint centroids in metres; all shear capacities are the attachment's **Unknown** 3,240 Pa
lateral-strength analogue, not a measured connection capacity.

| Joint | Board edges | Unit shear use | Homogeneous critical scale 1/u | Shear at largest scale, Pa | Unit centroid (x,y,z), m |
|---|---|---:|---:|---:|---|
| 3084 | west/board_2 / west/board_3 | 0.960847 | 1.040748 | 5021.72 | (-0.494797, 3.734729, 1.050276) |
| 3085 | west/board_2 / west/board_3 | 0.850667 | 1.175548 | 4220.46 | (-0.495027, 3.735036, 2.510926) |
| 3090 | west/board_3 / west/board_4 | 0.986678 | 1.013502 | 5274.23 | (-0.654983, 3.614977, -2.830628) |
| 3092 | west/board_3 / west/board_4 | 0.992994 | 1.007055 | 5249.72 | (-0.654972, 3.614963, 0.115239) |
| 3094 | west/board_3 / west/board_4 | 0.843601 | 1.185394 | 4444.70 | (-0.654992, 3.614990, 1.268653) |
| 3103 | west/board_4 / west/board_5 | 0.837778 | 1.193634 | 4157.68 | (-0.815143, 3.495191, -1.894512) |
| 3104 | west/board_4 / west/board_5 | 0.823551 | 1.214254 | 4134.71 | (-0.814526, 3.494367, -1.386826) |
| 3105 | west/board_4 / west/board_5 | 0.714726 | 1.399137 | 3804.83 | (-0.815177, 3.495236, -0.855947) |
| 3122 | west/board_6 / west/board_7 | 0.607136 | 1.647078 | 3513.59 | (-1.134923, 3.254897, -0.928059) |
| 3130 | west/board_7 / west/board_8 | 0.590692 | 1.692930 | 3364.62 | (-1.294974, 3.134966, -0.384339) |
| 3134 | west/board_8 / west/board_9 | 0.628855 | 1.590192 | 3423.55 | (-1.454694, 3.014593, -2.269850) |
| 3138 | west/board_8 / west/board_9 | 0.679232 | 1.472252 | 3778.78 | (-1.455057, 3.015076, 0.032557) |
| 3142 | west/board_9 / west/board_10 | 0.785209 | 1.273546 | 3843.35 | (-1.615137, 2.895183, -3.250664) |
| 3149 | west/board_9 / west/board_10 | 0.666052 | 1.501384 | 3739.38 | (-1.614973, 2.894963, 1.004529) |
| 3161 | west/board_10 / west/board_11 | 0.638185 | 1.566945 | 3358.16 | (-1.775006, 2.775008, 2.374000) |

DERIVED: uniform bulk elasticity gives `K ~ s`, weight `F ~ s^3`, displacement `u ~ s^2`,
and stress `sigma ~ F/A ~ s`. Thus the unit's largest use, 0.992994036 at joint 3092,
predicts a critical scale 1.007055; joint 3090 predicts 1.013502. Both are first-stage
failures in all 35 larger placements, including the smallest cracking placement.

The full construction is not homogeneous under this scaling. Fixing stiffness is authored
per area, so `K_fixing ~ s^2`, while bulk bearing remains `K_bulk ~ s`; their relative
stiffness changes with scale. MEASURED joint 3092 uses are 0.763796 at scale 0.810858,
1.338210 at 1.263034, and 1.620285 at 1.463569. These are respectively 5.14% below,
6.70% above and 11.49% above `s * unit_use`. The homogeneous prediction gives 200 initial
failures across the placements, compared with the measured 257. The larger house's highest
use is joint 3090 at 1.627850. The trend is self-weight and changed load sharing, not a
historical-output obligation or permission to increase an attachment strength.

## Failure judgement and the false cascade

MEASURED admitted-house residuals range from 3.784849155e-9 to 9.938599732e-9.
Every first-stage verdict passes the 1e-8 projected equilibrium tolerance.
The [before judgement table](judgement-s4-before.csv) records every nonempty crack stage.
It contains 35 converged initial stages and **523 nonempty Iterating stages**. Residuals
at those unfinished stages range from 0.134912 to 2.154933402e142. Their force guesses
are not equilibria; the law was incorrectly changing topology from them, repeatedly restarting
the next solve before it could converge. Each affected house eventually recorded 2,799
cracked joints; 35 were still unfinished at the census limit. None of that establishes
real progressive failure.

MEASURED example, `house_0008`, scale 1.463569:

| Numerical tick | Before cracks | Before relative residual | After cracks | After relative residual |
|---|---:|---:|---:|---:|
| 0 | 15 | 5.500967145e-9 | 15 | 5.500967145e-9 |
| 1 | 14 | 0.742104 | 0 | 0.742104 |
| 2 | 7 | 1.104198 | 0 | 0.383201 |
| 4 | 44 | 135.364787 | 0 | 0.399320 |
| 8 | 800 | 71971.15578 | 0 | 0.398397 |
| 16 | 0 | 1.282793e+52 | 0 | 0.398298 |
| 32 | 0 | 4.606602e+55 | 0 | 0.398296 |
| 64 | 0 | 2.020820e+58 | 0 | 0.069182 |
| 127 | 0 | 1.178705e+62 | 0 | 0.004290 |

The top initially failed joints 3084, 3085, 3090, 3092, 3094, 3103, 3104 and 3142 remain
cracked but unseparated in the trace: they retain compression/friction contact in the next
graph. Their closed seams have not silently lost connectivity. Later crack stages were
judged from unfinished force redistribution, so their causes cannot be classified as physical
contact escape or genuine overload. The [contact trace](contact-trace-s4.csv) additionally records actual projected
contact resultants and closed/sliding site counts, rather than applying the retired cohesive
strength to a contact's unprojected trial field.

The fix admits cohesive failure only from `Progress::Equilibrated`; a certified contact
mechanism can still separate contact faces. `Progress::Iterating` retains its warm displacement
and Krylov history for the next tick and changes no cohesion. This is a convergence gate,
not a cascade cap, strength margin or pre-settled state. The analytical hanging-chain witness
was red before the fix: a stale overstrength guess committed cracks even though total actual
weight/area was below the material's tensile limit. It passes with the gate and eventually
reaches equilibrium without cracks.

MEASURED after the fix: the [judgements](judgement-s4-after.csv) contain only the same
35 converged initial stages, totalling 257 cracks, with no later cracks or departures in
128 law ticks. The [residual envelope](residual-s4-after.csv) reports every numerical tick.
There are still 35 unfinished owners at that observation limit. This is explicitly unfinished
redistribution, not proof of either later stability or later collapse. All 635 placed trees,
scales 0.50205–1.499289, remain intact and finish; the unit house and unit tree are intact.

## Extended redistribution trace

MEASURED continuation of the largest house, `house_0008`, without fresh input:

| Numerical tick | New cracks | Departures | Progress | Relative residual |
|---|---:|---:|---|---:|
| 128 | 0 | 0 | Iterating | 4.291436514e-3 |
| 256 | 0 | 0 | Iterating | 2.287709371e-5 |
| 342 | 13 | 0 | Equilibrated | 9.179614952e-9 |
| 512 | 0 | 0 | Iterating | 0.158444658 |
| 912 | 8 | 0 | Equilibrated | 9.825398794e-9 |
| 1024 | 0 | 0 | Iterating | 5.334803242 |
| 2047 | 0 | 0 | Iterating | 5.253294025e-9 |

The two additional stages are converged redistributions and therefore genuine verdicts of the
current authored law: 36 total cracks, zero departures. The earlier one-stage-per-tick explosion
was an artifact. The trace shows no `Mechanism` verdict; closed cracked contacts remain in the
graph while the active-set/elastic solve continues. A low projected residual does not necessarily
mean the frozen inner solve has reached its stricter tolerance; no failure is admitted early.
The numerical continuation is still unfinished at its observation limit. This measures neither
wall time nor a real-time promise, and does not certify that the remaining roof will settle.
The later raw trace is in `target/bondsolve-s4-step1-gates.log`.

## Placement and roof options for the owner

DERIVED from `house_1.house.ron` and the placed scale range:

| Quantity | Smallest placement | Unit | Largest placement |
|---|---:|---:|---:|
| Footprint, m | 2.197 x 3.295 | 4 x 6 | 5.854 x 8.781 |
| Wall thickness, mm | 137.3 | 250 | 365.9 |
| Brick-plus-joint course, mm | 41.2 | 75 | 109.8 |
| Roof board length, m | 3.625 | 6.6 | 9.660 |
| Roof board width/thickness, mm | 109.8 / 27.5 | 200 / 50 | 292.7 / 73.2 |
| Total mass relative to unit | 0.166 | 1 | 3.135 |

Uniform scaling enlarges every brick, board and support span; it does not construct a larger
house from the same building stock. The present solver therefore cannot honestly certify
all existing placements as standing. The roof's automatically nailed free edges and missing
framing remain the owner's construction decision, as recorded in [roof-s3.md](roof-s3.md).
The strength analogue and directional timber omissions also limit any physical-soundness claim.

| Owner option | Evidence and appearance | Work |
|---|---|---|
| Place the current construction at unit scale | Unit gravity has no crack; nominal 4 x 6 m footprint and repeated house size | Placement edit and map review; no new construction bake. DERIVED: one admitted house equilibrium instead of the current 75 distinct scales, with shared immutable reuse |
| Restrict scales to the observed standing range, 0.549203–1.0 | Existing 39 smaller placements and unit fixture stand; footprint variation up to 4 x 6 m | Placement edit and review of 35 larger houses; this range is evidence, not a universal law or a proposed strength adjustment |
| Author size variants with correct framing and stock dimensions | Retains larger footprints without giant bricks or longer unsupported deck members; construction appears in interior views and debris | New authored dimensions, member counts/supports and joins, with a bake and gravity verification for each variant; runtime graph cost remains unmeasured |

**Decision for this lane:** leave roof, map, strengths and placement scales untouched. Supply
solver correctness and measured options; do not manufacture a spawn pass from the Unknown
connection data. The owner must choose the construction/placement change before the unchanged
spawn bar can be met.
