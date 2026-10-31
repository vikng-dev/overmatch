# Collision c6 shear source audit

Subject: the game's environment destruction and physics, game state only; contacts never dose a tank.

## Implemented material data

| Row | Cohesion c | Cohesive mode-II energy G_II | Source and scope |
|---|---:|---:|---|
| Brick | MEASURED published intercept 0.165 MPa | DERIVED 28.4 J/m² from the published zero-normal-stress regression 0.0284 N/mm | Segura et al., cement-mortar/solid clay brick triplets (TC), section 3.3 and Table 5; section 3.3 and Figure 10c respectively. ASSUMED mortar-interface analogue for the registry's effective masonry. |
| Wood | MEASURED published mean 8.9 MPa | DERIVED 740 J/m² from MEASURED mean 0.74 N/mm | Red spruce at 12% moisture, clear straight-grained material, USDA Wood Handbook Table 5-3a; Fruehmann et al., clear Norway spruce in RL mode II. ASSUMED spruce analogue for the scalar row's fracture energy. |
| mortared attachment | Same Brick row | Same Brick row | The declared `Seam("Brick")` has one source of material facts. |
| nailed attachment | Absent | Absent | No verified shear pair for the authored reference fixing; see below. |
| resting attachment | Not applicable | Not applicable | Unbonded bearing and friction, with no cohesive seam to shear. |

Only the shear facts were added. Existing strengths, opening energies, density, stiffness and blast data were left as authored. No law reads the new values: they are parsed and held by `BulkFacts` for collision lane 7. No directional timber model or punch ring was implemented.

The registry refuses a material that states any seam fact without all opening and shear facts, naming its actual file, row, missing field and repair. The regression removes each shear fact independently, rejects non-finite/non-positive or unsourced facts, and checks all shipped breakable material rows. The house spec documents which material and attachment owns each seam.

**Incomplete:** the separate `Stated` attachment schema still holds opening facts only. A strict shear refusal cannot be enabled for every shipped fixing while keeping this house loadable: `nailed` has no sourced shear pair. The attachment schema, fixing refusal gate and all-fixings completeness test remain to be completed together with that source. This report does not claim the lane's full acceptance gate is green. The material gate is green; the fixing gate is not implemented.

## Sources inspected

[Segura et al. (2021), *Experimental comparison of two testing setups for characterizing the shear mechanical properties of masonry*, Journal of Building Engineering 44, 103277](https://doi.org/10.1016/j.jobe.2021.103277), [author manuscript](https://upcommons.upc.edu/bitstream/2117/356908/1/Segura_et_al_JBE_2021_final.pdf). The authors distinguish cohesive fracture work above residual friction from the separate integral including friction to a prescribed displacement. The data uses their cohesive zero-precompression extrapolation, not a compressed specimen's total work. The paper reports substantial specimen/setup scatter; the triplet/couplet comparison is not evidence of a universal mortar value.

[Senalik and Farber (2021), *Wood Handbook*, Chapter 5, FPL-GTR-282, Table 5-3a](https://research.fs.usda.gov/download/treesearch/62244.pdf). The red-spruce dry row's shear column is 8900 kPa. This is clear-wood shear parallel to grain, not nail capacity or a structural grade's characteristic strength. Its application as a scalar seam fact retains the current model's lack of a grain frame; lane 8 owns that extension.

[Fruehmann, Tschegg and Stanzl-Tschegg (2001), *A New Experimental Technique for Determining the Fracture Energy of Wood under Mode II Loading Conditions*, ICF100694OR](https://www.icfweb.org/Proceedings/ICF10A/69/), [Results](https://www.icfweb.org/Proceedings/ICF10A/72/), [Conclusions](https://www.icfweb.org/Proceedings/ICF10A/74/). The primary proceedings text reports the mean and describes removing the elastic bending contribution. The study also reports a specimen-size effect. This is a timber material measurement, not a nailed-joint measurement.

[Schoenmakers (2010), *Fracture and failure mechanisms in timber loaded perpendicular to the grain by mechanical connections*, Table 2-4](https://pure.tue.nl/ws/files/3236576/201010274.pdf) was inspected visually after its PDF's text encoding failed extraction. It restates Stefansson's spruce measurements; that restatement was not substituted for an inspected original. The linked original Stefansson thesis returned access errors or a non-PDF response; those recalled numbers were not authored.

## Nailed fixing: why no number was filled

The attachment is a homogenized joint, not a piece of solid timber. Its effective cohesion needs a lateral force per nail times the authored nailing density; its effective G_II needs lateral separation work per nail over the same area. Solid wood's c or G_II, axial withdrawal work, and cyclic hysteresis work are different properties.

[Korin (2000), *Lateral Shear Capacity of Nailed Connectors*, CIB-W18/33-7-2](https://www.irbnet.de/daten/iconda/CIB_DC34247.pdf) is a primary timber-to-timber capacity study. It reports plain wire nails and thick members, but uses peak force or force at a limited slip as its capacity criterion. It does not establish complete lateral separation work for the authored fixing. The tested nail geometry also differs from the attachment's assumed reference. No energy was inferred from that capacity alone.

[Kho, Li and Lee (2019), *Behaviour of nailed timber-plywood connections under monotonic and cyclic loading*](https://www.timberdesign.org.nz/_files/ugd/7f6880_e03e3088c57049e8a4b3eae15d6134af.pdf) provides curves and capacity/ductility summaries for plywood-to-timber connections. Its ultimate displacement criterion is a reduction to 80% of peak or a displacement limit. That is not the complete-separation work of the house's timber-to-timber nailing.

[Shameli Derakhshan, Zhou and Ni (2022), *Influence of the Loading Protocol and Loading Rate on the Characteristics of Timber Nail Joints*](https://www.techscience.com/jrm/v10n10/48182/html), section 3.2, likewise defines its tabulated energy at a drop to 80% of peak. Its specimens join OSB to framing and the paper distinguishes loading-protocol effects. These energies were not relabelled as G_II.

[Loo, Quenneville and Chouw (2012), *A numerical approach for simulating the behaviour of timber shear walls*](https://doi.org/10.12989/sem.2012.42.3.383), [publisher PDF](https://www.techno-press.org/download.php?journal=sem&num=3&ordernum=6&volume=42), supplies a Foschi envelope and a declining tail from sheathing-to-framing test data. Transferring that envelope to this thick timber joint and extending its tail to separation would require an explicit connection-model assumption, not just a material-property lookup. This lane does not silently make that substitution to fill the missing field.

The remaining source requirement is a primary monotonic lateral force-slip/separation record, or an accepted engineering connection model with its applicable construction and material inputs, for the effective nailed fixing. A capacity-only design table cannot settle its fracture work. Decision: keep the values absent and report the uncompleted fixing work; do not invent a value or disable the shipped roof's breakability.

## Validation

MEASURED gate results: 21 material tests, 79 building tests, the shipped-spec parse/validation test, and 3 doc-citation checks pass. CI clippy passes for the locked workspace and all targets with `dev_ui,bitprobe` and warnings denied; formatting passes. The library test link emits the existing oversized unwind-table warning. No capture, game window or timing measurement was taken. The prerequisite review and its reproduced wall matrix are in `collision-c6-review.md`.
