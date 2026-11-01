# Blast material input source audit

Subject: the game's environment destruction model, game state only. These rows describe how a blast sphere loads construction materials. They do not change any tank or crew HP pool.

## Data contract

`BulkFacts::blast` holds a complete `BlastFacts` block for every breakable bulk row. Bulk-only unbreakable matter may omit it. Every authored scalar carries provenance; no numeric default is supplied. The loader refuses a missing block, a missing scalar, non-finite/non-positive values, a DIF below one, a fully dense density no greater than bulk density, or an overflowing dynamic-strength product. Refusals name the actual source file, material row, field and repair.

The DIF is a **reference-rate datum**, stored with its strain rate, not a universal factor. The material law must name how it uses that source domain; these data do not license extrapolation or imply that wood and brick were tested at the same rate. Law 1 now reads the reference-rate strength as an ASSUMED effective plateau, with no per-pulse strain-rate law. The wall design owns that model and its validity limits. This audit records the source domain, not an intrinsic rate law or permission to extrapolate it.

## Brick

| Input | Authored value | Provenance and scope |
|---|---:|---|
| Quasi-static compression | MEASURED published mean 36.5 MPa | Molded solid clay brick units, BIA TN 3A Table 1, ASTM C67. Unit strength, not the assembled masonry prism strength. |
| Dynamic increase factor | DERIVED 2.357106867962561 | Pereira et al. eq. 1 evaluated at ASSUMED reference rate 100 /s: `0.3224 ln(100) + 0.8724`. The fit is to handmade Galveias brick, with published domain `4 < rate < 200 /s`. Its use for the scalar molded-brick row is an ASSUMED rate analogue. The resulting dynamic compression is DERIVED 86.0344 MPa. |
| Fully dense grain density | MEASURED published parameter 2250 kg/m³ | Meyer Table 1, grade SW brick. The report attributes it to ERDC tests; the underlying ERDC report was not independently inspected. Its application to the existing London-stock bulk-density row is an ASSUMED analogue. |
| Young's modulus | DERIVED 8.6 GPa | BIA Table 5 interpolated for 36.5 MPa units with Type N mortar gives prism strength 12.265 MPa; eq. 1 gives `700 f'm = 8.586 GPa`, rounded. An effective assemblage modulus, not an intact unit modulus. |

Sources inspected:

- [Brick Industry Association, Technical Note 3A, June 2024, Tables 1 and 5 and eq. 1](https://www.gobrick.com/media/file/TN_3A_Brick_Masonry_Material_Properties.pdf). The current table was read, rather than inheriting the prototype's rounded prism-strength statement. The generic lowercase `3a-brick-masonry-material-properties.pdf` URL serves an older edition; the capitalized filename above is the inspected current note.
- [Pereira, Dias and Lourenco, *Dynamic Properties of Clay Brick at Different Strain Rates*, 12th Canadian Masonry Symposium (2013), eq. 1](https://www.canadamasonrydesigncentre.com/wp-content/uploads/12th_symposium/621.pdf). Its compression fit and stated rate domain were read. Its discussion of a separate tensile experiment is not a newly sourced dynamic tensile row.
- [Meyer, *Development of Geomaterial Parameters for Numerical Simulations Using the Holmquist-Johnson-Cook Constitutive Model for Concrete*, ARL-TR-5556 (2011), Table 1](https://www.govinfo.gov/content/pkg/GOVPUB-D101-PURL-gpo10967/pdf/GOVPUB-D101-PURL-gpo10967.pdf). The fully dense grain-density parameter was read from the table. The HJC volumetric-strain parameter is not copied into an engineering-strain field: the definitions differ.

The study's DERIVED `1 - 1845/2250 = 0.18` can be computed from the bulk and fully dense densities. It is an ideal pore-volume fraction, not an independently measured lock-up strain for the shipped row. The data retain the underlying density rather than silently declaring that fraction to be a measured compaction curve.

## Wood

| Input | Authored value | Provenance and scope |
|---|---:|---|
| Quasi-static compression | MEASURED 5.60 MPa | Zulkifli et al. Table 3, tangential spruce mean yield stress. Tested specimen moisture 8–10%; ASSUMED spruce analogue for the existing scalar forest timber. |
| Dynamic increase factor | DERIVED 5.960714285714286 at MEASURED 2400 /s | Tangential specimen ST-52 yield stress 33.38 MPa in Table 7 divided by the same study's static mean 5.60 MPa. Both measurements are read, not a recalled wood rate factor. The published tangential test range is MEASURED approximately 591–2530 /s. |
| Fully dense cell-wall density | DERIVED 1500 kg/m³ | Kamke's adopted theoretical limit 1.5 g/cm³, converted to SI. An ASSUMED cell-wall analogue, not a measured spruce lock-up strain. |
| Young's modulus | MEASURED 118.23 MPa | Zulkifli et al. Table 3, tangential quasi-static compression mean for the same specimen set as the static strength. |

Sources inspected:

- [Zulkifli, Kusumaningrum and Rahmi, *Experimental Study and Numerical Model of Spruce and Teak Wood Strength Properties Under Compressive High Strain Rate Loading*, J. Eng. Technol. Sci. 53(1), 210103 (2021), Tables 3 and 7](https://journals.itb.ac.id/index.php/jets/article/download/13345/pdf_111/44406), [DOI](https://doi.org/10.5614/j.eng.technol.sci.2021.53.1.3). The publisher PDF was downloaded and its relevant tables inspected visually. Static strength, transverse stiffness and the dynamic/static ratio therefore come from one experimental campaign. Applying that campaign to the row's existing density and its other spruce seam analogues is explicit, not an assertion that all rows describe one tested specimen.
- [Kamke, *Densified Radiata Pine for Structural Composites*, Maderas Ciencia y Tecnologia 8(2):83–92 (2006), discussion before Conclusions](https://doi.org/10.4067/S0718-221X2006000200002). The paper adopts approximately 1.5 g/cm³ as the theoretical fully dense cell-wall limit; that statement was read. Its thermally processed pine measurements are not relabelled as spruce blast measurements.
- [Senalik and Farber, USDA Wood Handbook FPL-GTR-282 (2021), Chapter 5 Tables 5-1 and 5-3a](https://research.fs.usda.gov/download/treesearch/62244.pdf). The indexed primary chapter confirms red-spruce transverse proportional-limit strength 3.8 MPa and longitudinal bending modulus 11.4 GPa, but the `E_T/E_L = 0.043` ratio belongs to **Sitka spruce**, not red spruce. It also recommends increasing bending modulus to estimate axial modulus before using the ratios. The study's recalled DERIVED 0.48 GPa transverse modulus is not copied. The new block uses the directly measured transverse compression modulus from Zulkifli instead. The USDA file was readable through the primary-source web index; direct downloads from this host returned access refusals.

The published wood DIF is a specimen-response observation, not a separated cell-wall viscoelastic coefficient. The compaction-front model's use of this ratio as an effective plateau remains an ASSUMED approximation: an independently sourced decomposition is required to determine how much of the measured enhancement its front inertia already represents. This audit does not assert a measured intrinsic DIF after removing inertial effects, nor does it replace that missing decomposition with a factor of one.

**Decision:** use the inspected tangential spruce campaign for the new compression and stiffness block. Do not fill a universal wood DIF of one or the recalled Gibson–Ashby factor 1.4. The latter was not verified from its original source and is absent. The fully dense density is sufficient input for a later explicitly stated compaction idealization; the data do not claim to supply a measured densification-onset strain. Dynamic tensile strength remains outside this lane's brief and is not fabricated from the compressive DIF.

## Validation

The material changes prepare data only. No crush-volume pool, scab calculation, contact-duration law, view asset, geometry, collider or body spawn was added.

MEASURED gate results: all 22 material tests pass, including independent omission, invalid-value and provenance refusals for every blast scalar. The combined building, wall, material, shipped-spec and doc-citation selection passes all 139 tests. Formatting and locked workspace/all-target clippy with `dev_ui,bitprobe` and warnings denied pass. The raw parser's bulk record is boxed to keep the authored enum compact; parsed runtime facts remain plain values. All runs used the build queue with unattended/headless settings, and no capture or timing measurement was taken.
