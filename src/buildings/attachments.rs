//! `assets/materials/attachments.ron`: the shared list of attachment kinds, how two parts of a
//! structure hold together where they touch, or a part to the ground (the game's environment
//! destruction, game state only). A building names a kind for each joint between two of its
//! components ([`super::schema::Bond::Attached`]) and each foundation anchor; the list says what
//! that kind holds by, as the
//! fracture laws read it ([`Bond`]): the seam of a substance (`materials.ron` owns those numbers),
//! a seam the list states with its sources, or a rest. A tree's species names the kind its trunk
//! holds to the ground by ([`RootPlate`], `crate::trees`). The file's comments carry each kind's
//! meaning.
//!
//! Embedded in the build as `materials.ron` is, and parsed once ([`Attachments::shipped_static`]).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::fracture::{Bond, Masonry, Seam, Shear};
use super::schema::BuildingAsset;
use crate::keyed_map::KeyedMap;
use crate::physical_laws::gravity_mps2;
use crate::provenance::Sourced;
use crate::substances::SubstanceRegistry;

/// The list's path under the repository root: the file its refusals name.
pub const ATTACHMENTS_PATH: &str = "assets/materials/attachments.ron";

const ATTACHMENTS_RON: &str = include_str!("../../assets/materials/attachments.ron");

/// What fixes one kind, as the file states it (the file's header).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum Row {
    Seam(String),
    Stated {
        tensile_pa: Sourced<f64>,
        fracture_energy_j_per_m2: Sourced<f64>,
    },
    Resting,
    RootPlate {
        overturning_nm_per_kg: Sourced<f64>,
    },
}

/// A trunk's anchorage to the ground by its root plate: the critical overturning moment it holds
/// per kilogram of the fresh stem it carries, N·m/kg (a moment per mass, no `g`). It states the
/// plate's strength alone, no separation work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RootPlate {
    pub(crate) overturning_nm_per_kg: f32,
}

impl RootPlate {
    /// The game's environment destruction model: equivalent circular base face. DERIVED:
    /// S = pi r^3 / 4 and tensile = C m / S - m g / A, so the base opens at moment C m.
    /// Positive hold requires r < 4 C / g.
    /// StandIn: rigid-plastic release has no separation work; the source supplies no work fact.
    pub(crate) fn seam(self, area: f32, mass: f32) -> Result<Seam, String> {
        let radius = (area / std::f32::consts::PI).sqrt();
        let modulus = std::f32::consts::PI * radius * radius * radius / 4.0;
        let tensile = self.overturning_nm_per_kg * mass / modulus - mass * gravity_mps2() / area;
        if !(area.is_finite()
            && area > 0.0
            && mass.is_finite()
            && mass > 0.0
            && tensile.is_finite()
            && tensile > 0.0)
        {
            return Err("root-plate area, mass and converted tensile hold must be positive finite simulation values".into());
        }
        Ok(Seam {
            tensile,
            fracture_energy: 0.0,
            shear: Shear::OpeningOnly,
        })
    }
}

/// A base face retains its footprint share so scaled instances use their actual mass and area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GroundAttachment {
    Joint(Bond),
    RootPlate {
        plate: RootPlate,
        area: f32,
        mass: f32,
    },
}

impl GroundAttachment {
    pub(crate) fn bond(self, scale: f32) -> Result<Bond, String> {
        match self {
            Self::Joint(bond) => Ok(bond),
            Self::RootPlate { plate, area, mass } => plate
                .seam(area * scale * scale, mass * scale * scale * scale)
                .map(Bond::Bonded),
        }
    }
}

/// What one kind fixes: a joint between two parts, or a trunk to the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Joint(Bond),
    RootPlate(RootPlate),
}

/// The attachment kinds by name, each resolved to what the fracture laws hold its joints by.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachments {
    kinds: BTreeMap<String, Kind>,
}

impl Attachments {
    /// The shipped list, parsed once. Panics on a list that does not load: a broken authored
    /// contract is a build defect, as the embedded `materials.ron` is
    /// ([`SubstanceRegistry::shipped`]).
    pub fn shipped_static() -> &'static Self {
        static LIST: LazyLock<Attachments> = LazyLock::new(|| {
            crate::provenance::shipped(ATTACHMENTS_PATH, || {
                Attachments::from_ron(ATTACHMENTS_RON, SubstanceRegistry::shipped_static())
            })
            .unwrap_or_else(|err| panic!("attachments: {ATTACHMENTS_PATH}: {err}"))
        });
        &LIST
    }

    /// Parse a list's text and resolve each kind against `registry`. Refuses a text that does not
    /// parse (a kind missing one of its numbers or its source, a name stated twice, a field the
    /// format does not hold), a stated number that is not finite and positive, and the seam of a
    /// substance that states none, each naming the kind, the field and the fix.
    fn from_ron(text: &str, registry: &SubstanceRegistry) -> Result<Self, String> {
        let KeyedMap(rows) = ron::from_str::<KeyedMap<Row>>(text).map_err(|err| {
            format!(
                "{err} — a kind is `Seam(\"<substance>\")`, `Stated(tensile_pa: (value, \
                 Kind(\"source\")), fracture_energy_j_per_m2: (value, Kind(\"source\")))`, \
                 `Resting` or `RootPlate(overturning_nm_per_kg: (value, Kind(\"source\")))`"
            )
        })?;
        let kinds = rows
            .into_iter()
            .map(|(name, row)| {
                let positive = |field: &str, value: f64| {
                    let converted = value as f32;
                    if converted.is_finite() && converted > 0.0 {
                        Ok(converted)
                    } else {
                        Err(format!(
                            "kind {name:?} in {ATTACHMENTS_PATH}: {field} {value} holds nothing in simulation precision: author a positive finite representable value"
                        ))
                    }
                };
                let kind = match row {
                    Row::Seam(substance) => Kind::Joint(Bond::Bonded(
                        Masonry::of(registry, &substance)
                            .map_err(|err| format!("kind {name:?}: {err}"))?
                            .ok_or_else(|| {
                                format!(
                                    "kind {name:?} takes the seam of {substance:?}, whose \
                                     materials.ron row states no bulk facts: name a substance \
                                     that states a seam, or state the kind's two numbers"
                                )
                            })?
                            .seam,
                    )),
                    Row::Stated {
                        tensile_pa: Sourced(tensile),
                        fracture_energy_j_per_m2: Sourced(energy),
                    } => {
                        let tensile = positive("tensile_pa", tensile)?;
                        let energy = positive("fracture_energy_j_per_m2", energy)?;
                        // The kind states its opening alone, so its seam answers by the opening
                        // law alone: no punch ring forms across it.
                        Kind::Joint(Bond::Bonded(Seam {
                            tensile,
                            fracture_energy: energy,
                            shear: Shear::OpeningOnly,
                        }))
                    }
                    Row::Resting => Kind::Joint(Bond::Resting),
                    Row::RootPlate {
                        overturning_nm_per_kg: Sourced(moment),
                    } => {
                        let simulation_moment = positive("overturning_nm_per_kg", moment)?;
                        Kind::RootPlate(RootPlate {
                            overturning_nm_per_kg: simulation_moment,
                        })
                    }
                };
                Ok((name, kind))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { kinds })
    }

    /// The kind `name`. Refuses a name the list does not hold, naming the kinds it does and the
    /// fix.
    fn kind(&self, name: &str) -> Result<Kind, String> {
        self.kinds.get(name).copied().ok_or_else(|| {
            format!(
                "kind {name:?} is not in {ATTACHMENTS_PATH}, which holds [{}]: name one of those, \
                 or add the kind there with its numbers and their sources",
                self.kinds
                    .keys()
                    .map(|kind| format!("{kind:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    /// What kind `name` holds a joint between two parts by. Refuses a name the list does not
    /// hold, and a root plate, which holds a trunk to the ground and no two parts together.
    pub(crate) fn bond(&self, name: &str) -> Result<Bond, String> {
        match self.kind(name)? {
            Kind::Joint(bond) => Ok(bond),
            Kind::RootPlate(_) => Err(format!(
                "kind {name:?} in {ATTACHMENTS_PATH} is a root plate, a trunk's anchorage to the \
                 ground: name a kind that joins two parts"
            )),
        }
    }

    /// The root plate `name` holds a trunk to the ground by. Refuses a name the list does not
    /// hold, and a kind that joins two parts.
    pub(crate) fn root_plate(&self, name: &str) -> Result<RootPlate, String> {
        match self.kind(name)? {
            Kind::RootPlate(plate) => Ok(plate),
            Kind::Joint(_) => Err(format!(
                "kind {name:?} in {ATTACHMENTS_PATH} joins two parts: a trunk's anchorage names a \
                 `RootPlate` kind"
            )),
        }
    }

    /// Admission for foundation attachments; both joint kinds and root plates can hold ground.
    pub fn holds_anchor(&self, name: &str) -> Result<(), String> {
        self.kind(name).map(|_| ())
    }

    /// The game's environment destruction model: distribute one component's root moment by
    /// base-face area. Each face receives that share of the carried mass, never the whole mass.
    pub(crate) fn anchors(&self, asset: &BuildingAsset) -> Result<Vec<GroundAttachment>, String> {
        let mut footprints = BTreeMap::<u16, (f32, f32)>::new();
        let component = |chunk: u32| {
            asset.elements[usize::from(asset.chunks[chunk as usize].element)].component
        };
        for anchor in &asset.anchors {
            footprints.entry(component(anchor.chunk)).or_default().0 += anchor.area_m2;
        }
        for chunk in &asset.chunks {
            if let Some((_, mass)) =
                footprints.get_mut(&asset.elements[usize::from(chunk.element)].component)
            {
                *mass += chunk.mass_kg;
            }
        }
        asset.anchors.iter().map(|anchor| {
            let name = &asset.attachments[usize::from(anchor.attachment)];
            let (area, mass) = footprints[&component(anchor.chunk)];
            let hold = match self.kind(name)? {
                Kind::Joint(bond) => GroundAttachment::Joint(bond),
                Kind::RootPlate(plate) => GroundAttachment::RootPlate {
                    plate,
                    area: anchor.area_m2,
                    mass: mass * (anchor.area_m2 / area),
                },
            };
            hold.bond(1.0).map_err(|err| {
                format!("anchor on chunk {}, attachment {name:?}: {err}; rebake a representable footprint", anchor.chunk)
            })?;
            Ok(hold)
        }).collect()
    }

    /// Whether the list holds kind `name`, refusing as [`Self::bond`] does: the bake's check of a
    /// spec, which never reads what a kind holds by.
    pub fn holds(&self, name: &str) -> Result<(), String> {
        self.bond(name).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> &'static SubstanceRegistry {
        SubstanceRegistry::shipped_static()
    }

    /// Every declared kind resolves to its own source, whatever names or number of kinds the
    /// list carries. Fracture reads these bonds: copying a seam or treating rest as zero-strength
    /// glue would substitute a different construction for the authored one.
    #[test]
    fn every_declared_kind_resolves_to_its_source() {
        let list = Attachments::shipped_static();
        let KeyedMap(rows) =
            crate::provenance::from_shipped_str::<KeyedMap<Row>>(ATTACHMENTS_PATH, ATTACHMENTS_RON)
                .unwrap();
        assert_eq!(list.kinds.len(), rows.len());
        for (name, row) in rows {
            let expected = match row {
                Row::Seam(substance) => Kind::Joint(Bond::Bonded(
                    Masonry::of(registry(), &substance)
                        .unwrap()
                        .expect("a declared seam has bulk facts")
                        .seam,
                )),
                Row::Stated {
                    tensile_pa: Sourced(tensile),
                    fracture_energy_j_per_m2: Sourced(energy),
                } => Kind::Joint(Bond::Bonded(Seam {
                    tensile: tensile as f32,
                    fracture_energy: energy as f32,
                    shear: Shear::OpeningOnly,
                })),
                Row::Resting => Kind::Joint(Bond::Resting),
                Row::RootPlate {
                    overturning_nm_per_kg: Sourced(moment),
                } => Kind::RootPlate(RootPlate {
                    overturning_nm_per_kg: moment as f32,
                }),
            };
            // A joint kind answers a building's joint and never a trunk's anchorage, and a root
            // plate the reverse: a building joined by a root plate, or a trunk anchored by mortar,
            // would read one construction as another.
            match expected {
                Kind::Joint(bond) => {
                    assert_eq!(list.bond(&name), Ok(bond), "{name}");
                    assert!(list.root_plate(&name).is_err(), "{name}");
                }
                Kind::RootPlate(plate) => {
                    assert_eq!(list.root_plate(&name), Ok(plate), "{name}");
                    assert!(list.bond(&name).is_err(), "{name}");
                }
            }
        }
    }

    /// An unknown kind is refused with the available kinds and fix. FIXTURE DATA: this small
    /// list makes absence independent of additions to the shipped attachment table.
    #[test]
    fn a_kind_the_list_does_not_hold_is_refused_naming_the_fix() {
        let list = Attachments::from_ron(r#"{"fixture-kind": Resting}"#, registry()).unwrap();
        let err = list.bond("unknown-kind").unwrap_err();
        for part in [
            "\"unknown-kind\"",
            ATTACHMENTS_PATH,
            "\"fixture-kind\"",
            "add the kind there",
        ] {
            assert!(err.contains(part), "{part}: {err}");
        }
        assert!(list.holds("unknown-kind").is_err());
    }

    /// A KIND WITHOUT BOTH NUMBERS, OR WITHOUT THEIR SOURCES, DOES NOT LOAD: each of the labelled fixture
    /// kind's two numbers dropped, its source dropped, a number of no strength, a name
    /// stated twice, a field the format does not hold, and a seam of a substance that states none
    /// are each refused, naming the field or the kind; so is a root plate without its moment, its
    /// source or a positive value. A list that let a kind load with a number missing would hold
    /// its joints, or a trunk to the ground, by a code default.
    #[test]
    fn a_kind_missing_a_number_or_its_source_is_refused() {
        let nailed = |fields: &str| format!("{{\"nailed\": Stated({fields})}}");
        let tensile = "tensile_pa: (3240.0, Fixture(\"fixture\"))";
        let energy = "fracture_energy_j_per_m2: (65.0, Fixture(\"fixture\"))";
        Attachments::from_ron(&nailed(&format!("{tensile}, {energy}")), registry())
            .expect("both numbers, each sourced");
        for (text, names) in [
            (nailed(tensile), "fracture_energy_j_per_m2"),
            (nailed(energy), "tensile_pa"),
            (
                nailed(&format!("tensile_pa: 3240.0, {energy}")),
                "provenance",
            ),
            (
                nailed(&format!(
                    "tensile_pa: (0.0, Fixture(\"fixture\")), {energy}"
                )),
                "tensile_pa 0",
            ),
            (
                nailed(&format!("{tensile}, {energy}, spacing_m: 0.6")),
                "spacing_m",
            ),
            (
                "{\"nailed\": Resting, \"nailed\": Resting}".to_owned(),
                "twice",
            ),
            ("{\"mortared\": Seam(\"RHA\")}".to_owned(), "\"mortared\""),
            ("{\"mortared\": Seam(\"Adobe\")}".to_owned(), "Adobe"),
            (
                "{\"plate\": RootPlate()}".to_owned(),
                "overturning_nm_per_kg",
            ),
            (
                "{\"plate\": RootPlate(overturning_nm_per_kg: 150.0)}".to_owned(),
                "provenance",
            ),
            (
                "{\"plate\": RootPlate(overturning_nm_per_kg: (0.0, Fixture(\"fixture\")))}"
                    .to_owned(),
                "overturning_nm_per_kg 0",
            ),
        ] {
            let err = Attachments::from_ron(&text, registry()).unwrap_err();
            assert!(err.contains(names), "{text}: {err}");
        }
    }

    /// Load laws rely on every sourced coefficient surviving conversion to simulation precision.
    /// Fixture magnitudes straddle f32's range; checking f64 alone admits zero or infinity.
    #[test]
    fn attachment_values_must_survive_simulation_precision_at_load() {
        for value in ["1.0e100", "1.0e-100"] {
            for (row, field) in [
                (
                    format!(
                        "Stated(tensile_pa: ({value}, Fixture(\"fixture\")), fracture_energy_j_per_m2: (5.0, Fixture(\"fixture\")))"
                    ),
                    "tensile_pa",
                ),
                (
                    format!(
                        "Stated(tensile_pa: (10.0, Fixture(\"fixture\")), fracture_energy_j_per_m2: ({value}, Fixture(\"fixture\")))"
                    ),
                    "fracture_energy_j_per_m2",
                ),
                (
                    format!("RootPlate(overturning_nm_per_kg: ({value}, Fixture(\"fixture\")))"),
                    "overturning_nm_per_kg",
                ),
            ] {
                let error = Attachments::from_ron(&format!("{{\"fixture\": {row}}}"), registry())
                    .unwrap_err();
                for part in ["fixture", ATTACHMENTS_PATH, field, "author"] {
                    assert!(error.contains(part), "{error}");
                }
            }
        }
    }

    /// Fixture construction: two unequal base cells and one upper cell in one component.
    /// Inline fixture geometry has total volume 4 m³ and density 100 kg/m³, hence mass 400 kg.
    fn footprint() -> BuildingAsset {
        use super::super::schema::*;
        let cuboid = |x: [f32; 2], y: [f32; 2]| -> Vec<[f32; 3]> {
            (0..8)
                .map(|i| [x[i & 1], y[(i >> 1) & 1], [0.0, 1.0][i >> 2]])
                .collect()
        };
        let element = Element {
            name: "fixture".into(),
            kind: ElementKind::Pier,
            component: 0,
            material: 0,
            surface_frame: SurfaceFrame::Upright,
            hull: cuboid([0.0, 2.0], [0.0, 2.0]),
            chunks: [0, 3],
        };
        let mut chunks = Vec::new();
        let mut anchors = Vec::new();
        for (k, (low, high, bottom, top)) in [
            (0.0, 0.5, 0.0, 1.0),
            (0.5, 2.0, 0.0, 1.0),
            (0.0, 2.0, 1.0, 2.0),
        ]
        .into_iter()
        .enumerate()
        {
            chunks.push(Chunk {
                element: 0,
                material: 0,
                hull: cuboid([low, high], [bottom, top]),
                volume_m3: high - low,
                centroid: [(low + high) / 2.0, (bottom + top) / 2.0, 0.5],
                mass_kg: (high - low) * 100.0,
            });
            if bottom == 0.0 {
                anchors.push(Anchor {
                    chunk: k as u32,
                    attachment: 0,
                    area_m2: high - low,
                    centroid: [(low + high) / 2.0, 0.0, 0.5],
                });
            }
        }
        BuildingAsset {
            id: "fixture".into(),
            format: FORMAT,
            foundation_y_m: 0.0,
            materials: vec!["Brick".into()],
            attachments: vec!["fixture".into()],
            components: vec!["fixture".into()],
            elements: vec![element],
            chunks,
            joints: vec![],
            anchors,
        }
    }

    /// The game's environment destruction load laws must read the named foundation kind,
    /// including rest. Substituting the chunk's seam fails these analytical fixture strengths.
    #[test]
    fn layout_reads_the_named_anchor_kind() {
        let asset = footprint();
        asset.check().unwrap();
        let masonry = [Masonry::of(registry(), "Brick").unwrap()];
        for (row, expected) in [
            ("Resting", Bond::Resting),
            (
                "Stated(tensile_pa: (10.0, Fixture(\"fixture\")), fracture_energy_j_per_m2: (5.0, Fixture(\"fixture\")))",
                Bond::Bonded(Seam {
                    tensile: 10.0,
                    fracture_energy: 5.0,
                    shear: Shear::OpeningOnly,
                }),
            ),
        ] {
            let list =
                Attachments::from_ron(&format!("{{\"fixture\": {row}}}"), registry()).unwrap();
            let layout = super::super::breakage::Layout::of(&asset, &masonry, &list).unwrap();
            let structure = layout.structure(&asset, 1.0);
            assert_eq!(structure.joints.len(), asset.anchors.len());
            for joint in structure.joints {
                assert_eq!(joint.bond, expected);
            }
        }
    }

    /// The game's environment destruction model allocates C*m once per footprint. Load laws
    /// rely on the sum of each base face's critical moments equalling that declared budget.
    #[test]
    fn split_base_faces_share_one_root_moment_at_each_scale() {
        let asset = footprint();
        let masonry = [Masonry::of(registry(), "Brick").unwrap()];
        let list = Attachments::from_ron(
            r#"{"fixture": RootPlate(overturning_nm_per_kg: (100.0, Fixture("fixture")))}"#,
            registry(),
        )
        .unwrap();
        let layout = super::super::breakage::Layout::of(&asset, &masonry, &list).unwrap();
        for scale in [0.5_f32, 1.0, 2.0] {
            let structure = layout.structure(&asset, scale);
            let mut moment = 0.0;
            // Resolution: 32 f32 epsilons cover the products, square root and sum against
            // this independently evaluated f64 section calculation.
            for (joint, fraction) in structure.joints.iter().zip([0.25, 0.75]) {
                let Bond::Bonded(seam) = joint.bond else {
                    panic!("a root plate is bonded")
                };
                let area = f64::from(joint.area);
                let radius = (area / std::f64::consts::PI).sqrt();
                let modulus = std::f64::consts::PI * radius.powi(3) / 4.0;
                let mass = 400.0 * f64::from(scale).powi(3) * fraction;
                let critical =
                    (f64::from(seam.tensile) + mass * f64::from(gravity_mps2()) / area) * modulus;
                let expected = 100.0 * mass;
                assert!(
                    (critical - expected).abs() <= expected * 32.0 * f64::from(f32::EPSILON),
                    "{critical} != {expected}"
                );
                assert_eq!(seam.fracture_energy, 0.0);
                assert_eq!(seam.shear, Shear::OpeningOnly);
                moment += critical;
            }
            let expected = 40_000.0 * f64::from(scale).powi(3);
            assert!((moment - expected).abs() <= expected * 32.0 * f64::from(f32::EPSILON));
        }
    }

    /// Bakers and load laws refuse malformed anchor identities and root conversions before play.
    #[test]
    fn invalid_anchor_identity_or_root_hold_is_refused() {
        let mut asset = footprint();
        asset.anchors[0].attachment = u8::MAX;
        assert!(asset.check().unwrap_err().contains("past the table"));
        asset.anchors[0].attachment = 0;
        asset.attachments.push("other".into());
        asset.anchors[1].attachment = 1;
        assert!(
            asset
                .check()
                .unwrap_err()
                .contains("one kind for its footprint")
        );
        let list = Attachments::from_ron(
            r#"{"fixture": RootPlate(overturning_nm_per_kg: (1.0e38, Fixture("fixture")))}"#,
            registry(),
        )
        .unwrap();
        let error = list.anchors(&footprint()).unwrap_err();
        for part in ["anchor on chunk", "fixture", "converted tensile", "rebake"] {
            assert!(error.contains(part), "{error}");
        }
        let text = ron::ser::to_string(&footprint()).unwrap();
        assert!(
            BuildingAsset::from_ron(&text.replace("attachment:0,", ""))
                .unwrap_err()
                .contains("attachment")
        );
    }
}
