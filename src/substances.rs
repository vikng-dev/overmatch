//! The global substance registry — the numeric half of the material library
//! (`assets/materials/materials.ron`).
//!
//! The Blender side is `assets/materials/materials.blend`: one material datablock per entry,
//! LINKED into each tank's `.blend` and assigned to its ballistic meshes. The datablock NAME is
//! the join key; this module owns the parsed scalars. Design doc §12 ("no numbers in the model")
//! and §13.7 (substance is authored per plate, paint is a runtime livery). The target structure
//! (material facts, declared effective behaviour, mechanisms reading them) is
//! `.agents/scratch/damage-cam-2026-10-08/research/recommendation-solid-shot.md` Part 1.
//!
//! THIS IS THE CLASSIFIER, not just a table. [`crate::bake`] asks it about every glTF primitive's
//! material name: `Ok` means the primitive IS a ballistic volume made of that substance, `Err`
//! means it is ordinary art. Membership and resistance therefore come from one lookup and cannot
//! disagree.
//!
//! # Three kinds of substance
//!
//! A [`Substance`] is [`Substance::Hard`], [`Substance::Witness`] or [`Substance::Unworn`], and the
//! kind decides which law reads it (ADR-0040). Hard matter is a layer of the §13 walk: it resists
//! the shell, is charged the shell's spend and shadows the spall field. A witness has no resistance
//! — not a zero one, none — so nothing can hand it to the walk; it takes dose from what the hard
//! world sent through it (`ballistics::wound`). Crew are the witnesses. An unworn substance is no
//! volume's: it states only facts a round reads, [`SubstanceRegistry::hard`] refuses it and
//! [`Substance::matter`] has no answer for it, so the walk fails closed on a volume that names it.
//!
//! # Burst facts
//!
//! A burst core is any substance whose row states [`BurstFacts`] (`burst: Some(..)`), the facts a
//! round's burst block reads ([`crate::rounds`]). They are an optional field of a row, beside what
//! its kind states: an unworn row states them alone (and states nothing else, so it must state
//! them), and a hard row may state them too ([`Substance::burst`]).
//!
//! # What a hard entry states
//!
//! - [`MaterialFacts`]: density, flow strength, Young's modulus and Poisson's ratio, for a substance
//!   that is a structural metal with a sourced row. `None` where no fact is known — never a guess.
//! - [`BulkFacts`]: density and fracture toughness, for a non-metal with a sourced row (brick,
//!   timber). A row states either these or [`MaterialFacts`], never both, so its density is stated
//!   once ([`HardSubstance::density`]).
//! - [`Resistance`]: how a round's force law is resisted. Exactly one entry is the
//!   [`Resistance::Reference`] — the reference steel, whose facts ARE the force law's reference
//!   constants and whose millimetre is the reference-mm. Every other entry DECLARES its effective
//!   behaviour ([`Resistance::Effective`]): it resists as a fixed fraction of the reference force at
//!   every speed. Declared, never inferred from a fill fraction or from facts the law does not yet
//!   read.
//! - The debris half-angle: the rim of the spall cone a body exit through it throws, authored in
//!   degrees and carried as the field law's ratio ([`HardSubstance::debris_ratio`]).
//!
//! FAIL-LOUD, NO DEFAULTS (ADR-0011, and §13.1's defect class): an unknown substance name is a hard
//! error naming the name, never a fallback; a malformed entry is refused at parse, naming the entry.

use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use std::collections::BTreeMap;

use crate::provenance::Sourced;

/// The shipped registry, embedded so the walk never depends on asset-server timing (the same reason
/// [`crate::bake`] embeds the Tiger spec).
pub(crate) const MATERIALS_RON: &str = include_str!("../assets/materials/materials.ron");

/// Reference-mm per metre of the reference substance: the unit's definition (a millimetre of the
/// reference steel is one reference-mm), not a measured number.
pub const REFERENCE_FACTOR: f32 = 1000.0;

/// The facts a structural metal is stated by. SI units throughout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialFacts {
    /// kg/m³.
    pub density: f64,
    /// Representative flow stress at large strain (Pa) — the cavity-expansion law's `Y`.
    pub flow_strength: f64,
    /// Pa.
    pub youngs_modulus: f64,
    /// Dimensionless, in `(0, ½)`.
    pub poisson_ratio: f64,
}

impl MaterialFacts {
    /// Cavity-expansion strength term `a = A_CE · Y` (Pa), in the compressible
    /// elastic–perfectly-plastic form `A_CE = (2/3) · (1 + ln(E / (3 (1 − ν) Y)))`. At `ν = ½` it is
    /// the incompressible form `(2/3) · (1 + ln(2E / (3Y)))`.
    ///
    /// A DERIVED prior from material facts, not validated against thick-steel depth data; the
    /// compressible form was read in restatement, not in its primary (Hill 1950; Forrestal & Luk
    /// 1988). The logarithm is `libm`'s, so the term is the same bits on every shipped target.
    pub fn cavity_strength(&self) -> f64 {
        let ratio = self.youngs_modulus / (3.0 * (1.0 - self.poisson_ratio) * self.flow_strength);
        let a_ce = 2.0 / 3.0 * (1.0 + libm::log(ratio));
        a_ce * self.flow_strength
    }

    /// Every fact finite and in its physical domain, and the strength term they give finite and
    /// positive. Names the first violation.
    fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("density", self.density),
            ("flow_strength", self.flow_strength),
            ("youngs_modulus", self.youngs_modulus),
            ("poisson_ratio", self.poisson_ratio),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{field} must be finite and positive (got {value})"));
            }
        }
        if self.poisson_ratio >= 0.5 {
            return Err(format!(
                "poisson_ratio must be below ½ (got {}) — the cavity law divides by 1 − ν and a \
                 solid with ν ≥ ½ has no finite bulk modulus",
                self.poisson_ratio
            ));
        }
        let strength = self.cavity_strength();
        if !strength.is_finite() || strength <= 0.0 {
            return Err(format!(
                "the facts give a cavity strength of {strength} Pa — E / (3 (1 − ν) Y) is too small \
                 for the elastic–plastic cavity law to state a resistance"
            ));
        }
        Ok(())
    }
}

/// The bulk facts of a non-metal: what a building's bake reads (a chunk's mass) and what the
/// buildings' fracture laws read (`buildings::fracture`). SI units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BulkFacts {
    /// kg/m³.
    pub density: f64,
    /// Mode I fracture toughness `K_IC`, Pa·√m. No law reads it yet.
    pub toughness: f64,
    /// The tensile strength across a seam between pre-cut chunks of this substance, Pa: what the
    /// push rule compares the load at a joint with. `None` where none is sourced: an element of
    /// such a substance never breaks.
    pub tensile: Option<f64>,
    /// The seam's mode-I fracture energy `G_f`, J/m²: the energy that opens a square metre of
    /// seam, what the structural check charges a section. Stated with [`Self::tensile`] or not at
    /// all (checked at parse).
    pub fracture_energy: Option<f64>,
}

/// What a round's burst block reads of the substance its burst core is made of. SI units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BurstFacts {
    /// kg/m³.
    pub density: f64,
    /// The speed constant `K` (m/s): the sideways speed scale of the burst's `v_s`.
    pub speed_constant: f64,
    /// The chemical energy one kilogram of it releases, J/kg: what a burst's blast field carries
    /// once the casing has taken its share (`buildings::fracture`). `None` where none is sourced:
    /// such a core drives a casing and no blast.
    pub chemical_energy: Option<f64>,
}

/// How a substance resists a round's force law.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resistance {
    /// The reference steel: its facts are the force law's reference constants, and one metre of it
    /// is [`REFERENCE_FACTOR`] reference-mm by definition.
    Reference,
    /// A declared effective behaviour: reference-mm per metre of chord, i.e. the substance resists
    /// as `factor / REFERENCE_FACTOR` of the reference steel's force at every speed. The value is
    /// authored, not derived.
    Effective(f32),
}

/// A substance's identity: its position in the registry's name order. Stable for one registry;
/// never a wire or save identity (the registry's own text is).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SubstanceId(u64);

impl SubstanceId {
    /// A fixture substance for tests that build volumes without a registry: one id per factor, so
    /// fixture plates of one factor are one substance. Disjoint from every registry id.
    #[cfg(test)]
    pub(crate) fn fixture(factor: f32) -> Self {
        Self(1 << 32 | factor.to_bits() as u64)
    }

    /// The §13.6 gate's probe substance: the stronger copy its monotonicity check adds beside a
    /// real volume. Disjoint from every registry and fixture id.
    #[cfg(any(feature = "dev_tools", test))]
    pub(crate) fn probe() -> Self {
        Self(2 << 32)
    }
}

/// One substance's entry: matter the walk resists with, a witness the walk never sees, or a
/// substance no volume wears.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Substance {
    /// A layer of the walk.
    Hard(HardSubstance),
    /// Takes dose from what passes through it. No resistance, no facts, no livery.
    Witness,
    /// Worn by no volume: it states only facts a round reads, nothing the walk reads. Its burst
    /// facts are always stated (checked at parse): a row that states nothing is no substance.
    Unworn { burst: Option<BurstFacts> },
}

/// A hard substance's entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HardSubstance {
    pub resistance: Resistance,
    /// `λ = 1/sin α` of the authored debris half-angle `α`: the field law's ratio for the spall
    /// cone a body exit through this substance throws (design doc §5), at least 1 and in the law's
    /// domain (checked at parse).
    pub debris_ratio: f32,
    /// The substance's material facts, or `None` where none are sourced.
    pub facts: Option<MaterialFacts>,
    /// A non-metal's bulk facts, or `None` where none are sourced. Never stated beside
    /// [`Self::facts`] (checked at parse).
    pub bulk: Option<BulkFacts>,
    /// The dynamic friction coefficient this substance brings to a contact, dimensionless, or
    /// `None` where none is stated. Avian combines the two sides of a pair under its rule, so this
    /// is one side's value, not the pair's. Read by every body that rests on the ground by
    /// contact: a loose building piece, a launched turret.
    pub friction: Option<f32>,
    /// The facts a round's burst block reads where the substance is a burst core too, or `None`.
    /// The walk never reads them.
    pub burst: Option<BurstFacts>,
}

impl HardSubstance {
    /// Reference-mm per metre of chord: [`REFERENCE_FACTOR`] for the reference, the declared value
    /// otherwise.
    pub fn factor(&self) -> f32 {
        match self.resistance {
            Resistance::Reference => REFERENCE_FACTOR,
            Resistance::Effective(factor) => factor,
        }
    }

    /// [`Self::friction`], or an error naming `name` where the row states none: for a body whose
    /// contact must not fall back to Avian's default.
    pub fn friction_of(&self, name: &str) -> Result<f32, String> {
        self.friction.ok_or_else(|| {
            format!("substance {name:?} states no friction, and this body rests on it by contact")
        })
    }

    /// kg/m³, from whichever facts the row states, or `None` where it states neither.
    pub fn density(&self) -> Option<f64> {
        self.facts
            .map(|facts| facts.density)
            .or(self.bulk.map(|bulk| bulk.density))
    }
}

impl Substance {
    /// The hard entry, or `None` for a witness or an unworn substance.
    pub fn hard(&self) -> Option<&HardSubstance> {
        match self {
            Self::Hard(hard) => Some(hard),
            Self::Witness | Self::Unworn { .. } => None,
        }
    }

    /// The kind as a ballistic primitive carries it, or `None` for an unworn substance, which no
    /// primitive wears.
    pub fn matter(&self) -> Option<Matter> {
        match self {
            Self::Hard(hard) => Some(Matter::Hard {
                factor: hard.factor(),
                debris_ratio: hard.debris_ratio,
            }),
            Self::Witness => Some(Matter::Witness),
            Self::Unworn { .. } => None,
        }
    }

    /// The burst facts its row states, of a hard or an unworn substance; `None` where it states
    /// none, and for a witness, which states nothing.
    pub fn burst(&self) -> Option<BurstFacts> {
        match self {
            Self::Hard(hard) => hard.burst,
            Self::Unworn { burst } => *burst,
            Self::Witness => None,
        }
    }
}

/// A substance's kind as a ballistic primitive carries it, resolved once at the bake: what the
/// walk reads of hard matter, nothing for a witness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Matter {
    Hard {
        /// The §13.2 field value: reference-mm per metre of chord.
        factor: f32,
        /// [`HardSubstance::debris_ratio`].
        debris_ratio: f32,
    },
    Witness,
}

/// One entry as authored: `Hard(..)` or `Unworn(..)` with every number carrying its provenance
/// ([`crate::provenance`]), or a bare `Witness`. `visual` is the key of the material's look in the
/// view's visual library (`assets/materials/visuals.ron`), not a number, and carries none; the
/// reference's resistance is the unit's definition and carries none either.
/// A hard row's `burst` is optional, absent read as `None`; an unworn row authors it, as it states
/// nothing else.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum SubstanceFile {
    Hard {
        resistance: ResistanceFile,
        visual: String,
        /// Degrees, converted once at parse ([`debris_ratio`]).
        debris_half_angle_deg: Sourced<f64>,
        facts: Option<FactsFile>,
        #[serde(default)]
        bulk: Option<BulkFile>,
        #[serde(default)]
        friction: Option<Sourced<f32>>,
        #[serde(default)]
        burst: Option<BurstFile>,
    },
    Witness,
    Unworn {
        burst: Option<BurstFile>,
    },
}

/// A burst core's facts as authored: density (kg/m³) and the speed constant `K` (m/s).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BurstFile {
    density: Sourced<f64>,
    speed_constant_mps: Sourced<f64>,
    #[serde(default)]
    chemical_energy_j_per_kg: Option<Sourced<f64>>,
}

impl BurstFile {
    fn facts(self) -> BurstFacts {
        BurstFacts {
            density: self.density.0,
            speed_constant: self.speed_constant_mps.0,
            chemical_energy: self.chemical_energy_j_per_kg.map(|energy| energy.0),
        }
    }
}

/// A non-metal's bulk facts as authored: density (kg/m³), fracture toughness (Pa·√m) and,
/// optionally, the seam's tensile strength (Pa).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BulkFile {
    density: Sourced<f64>,
    toughness: Sourced<f64>,
    #[serde(default)]
    tensile: Option<Sourced<f64>>,
    #[serde(default)]
    fracture_energy: Option<Sourced<f64>>,
}

#[derive(Deserialize)]
enum ResistanceFile {
    Reference,
    Effective(Sourced<f32>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FactsFile {
    density: Sourced<f64>,
    flow_strength: Sourced<f64>,
    youngs_modulus: Sourced<f64>,
    poisson_ratio: Sourced<f64>,
}

/// The field law's ratio `λ = 1/sin α` of the debris half-angle `degrees` authored for substance
/// `name`, converted once here with `libm`'s `sin` (the same bits on every shipped target).
/// Refuses an angle outside `(0°, 90°]` and one the field law cannot resolve
/// ([`crate::ballistics::spall_cone::FieldDescriptor::check`]), naming the substance.
fn debris_ratio(name: &str, degrees: f64) -> Result<f32, String> {
    if !(degrees > 0.0 && degrees <= 90.0) {
        return Err(format!(
            "substance {name:?} has a debris half-angle of {degrees}° — a plate exit's spall \
             cone opens to an angle in (0°, 90°]"
        ));
    }
    let ratio = (1.0 / libm::sin(degrees * std::f64::consts::PI / 180.0)) as f32;
    crate::ballistics::spall_cone::FieldDescriptor::ball(ratio)
        .check()
        .map_err(|why| {
            format!("substance {name:?} has a debris half-angle of {degrees}°: {why}")
        })?;
    Ok(ratio)
}

/// Substance `name` as authored, converted: the debris half-angle to its ratio
/// ([`debris_ratio`]), refused there by name, and a hard row's visual key beside it, refused blank.
fn converted(name: &str, file: SubstanceFile) -> Result<(Substance, Option<String>), String> {
    Ok(match file {
        SubstanceFile::Hard {
            resistance,
            visual,
            debris_half_angle_deg,
            facts,
            bulk,
            friction,
            burst,
        } => {
            if visual.trim().is_empty() {
                return Err(format!(
                    "substance {name:?} names a blank visual — every hard substance is worn, and a \
                     worn surface draws as the visual it names"
                ));
            }
            let hard = HardSubstance {
                resistance: match resistance {
                    ResistanceFile::Reference => Resistance::Reference,
                    ResistanceFile::Effective(Sourced(factor)) => Resistance::Effective(factor),
                },
                debris_ratio: debris_ratio(name, debris_half_angle_deg.0)?,
                facts: facts.map(|facts| MaterialFacts {
                    density: facts.density.0,
                    flow_strength: facts.flow_strength.0,
                    youngs_modulus: facts.youngs_modulus.0,
                    poisson_ratio: facts.poisson_ratio.0,
                }),
                bulk: bulk.map(|bulk| BulkFacts {
                    density: bulk.density.0,
                    toughness: bulk.toughness.0,
                    tensile: bulk.tensile.map(|tensile| tensile.0),
                    fracture_energy: bulk.fracture_energy.map(|energy| energy.0),
                }),
                friction: friction.map(|friction| friction.0),
                burst: burst.map(BurstFile::facts),
            };
            (Substance::Hard(hard), Some(visual))
        }
        SubstanceFile::Witness => (Substance::Witness, None),
        SubstanceFile::Unworn { burst } => (
            Substance::Unworn {
                burst: burst.map(BurstFile::facts),
            },
            None,
        ),
    })
}

/// The parsed `materials.ron` — every substance in the game, keyed by material datablock name.
#[derive(Clone, Debug)]
pub struct SubstanceRegistry {
    substances: BTreeMap<String, Substance>,
    /// Each hard substance's visual key (`assets/materials/visuals.ron`), by substance name. View
    /// data the walk never reads, held apart from [`HardSubstance`] so the walk's entry stays `Copy`.
    visuals: BTreeMap<String, String>,
    /// The one [`Resistance::Reference`] entry's name.
    reference: String,
}

/// The file's shape before the whole-registry checks ([`SubstanceRegistry::from_ron`]).
#[derive(Deserialize)]
#[serde(rename = "SubstanceRegistry", deny_unknown_fields)]
struct RegistryFile {
    substances: SubstanceMap,
}

/// The substance table, with each entry's validity enforced AT PARSE.
///
/// A plain `HashMap` cannot carry the contract: serde fills one by repeated insertion, so a
/// duplicated key silently overwrites and the file's second `"RHA"` wins with nobody told. Nor does
/// a derived map reject a value that cannot be a resistance. Both are the §13.1 defect class
/// dressed as data — a registry that answers `Ok` for a file it should have refused hands the walk
/// a wrong number instead of an error.
///
/// Validity is checked where the entries arrive, not later at [`crate::ballistics`]'s
/// `VolumeTable`: that one guards the WALK, this one guards the FILE, and a bad file must fail at
/// the bake rather than at the first shot that happens to cross the offending plate.
#[derive(Clone, Debug, Default)]
struct SubstanceMap {
    substances: BTreeMap<String, Substance>,
    visuals: BTreeMap<String, String>,
}

impl<'de> Deserialize<'de> for SubstanceMap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor;

        impl<'de> Visitor<'de> for MapVisitor {
            type Value = SubstanceMap;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of material datablock name to substance")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                // BTreeMap, not HashMap: the diagnostic vocabulary and any future iteration are then
                // ordered by construction rather than by hash seed.
                let mut out = SubstanceMap::default();
                while let Some((name, substance)) = access.next_entry::<String, SubstanceFile>()? {
                    let (substance, visual) = converted(&name, substance)
                        .and_then(|(substance, visual)| Ok((checked(&name, substance)?, visual)))
                        .map_err(de::Error::custom)?;
                    if let Some(visual) = visual {
                        out.visuals.insert(name.clone(), visual);
                    }
                    if out.substances.insert(name.clone(), substance).is_some() {
                        return Err(de::Error::custom(format!(
                            "substance {name:?} is declared twice — the datablock name is the join \
                             key between materials.blend and this file, so a duplicate means two \
                             different answers for one key and the file cannot say which is meant"
                        )));
                    }
                }
                Ok(out)
            }
        }

        deserializer.deserialize_map(MapVisitor)
    }
}

/// One entry's own validity, with `-0.0` canonicalized. A witness states nothing to check; burst
/// facts, wherever stated, are each finite and positive; an unworn row states them, or it states
/// nothing at all.
fn checked(name: &str, substance: Substance) -> Result<Substance, String> {
    if let Some(facts) = substance.burst() {
        checked_burst(name, facts)?;
    }
    match substance {
        Substance::Hard(hard) => checked_hard(name, hard).map(Substance::Hard),
        Substance::Witness => Ok(Substance::Witness),
        Substance::Unworn { burst: None } => Err(format!(
            "substance {name:?} is worn by no volume and states no burst facts — a row no volume \
             wears and no round reads is no substance"
        )),
        unworn @ Substance::Unworn { .. } => Ok(unworn),
    }
}

/// A burst core's two facts, each finite and positive.
fn checked_burst(name: &str, facts: BurstFacts) -> Result<(), String> {
    let stated = [
        ("density", Some(facts.density)),
        ("speed_constant_mps", Some(facts.speed_constant)),
        ("chemical_energy_j_per_kg", facts.chemical_energy),
    ];
    for (field, value) in stated
        .into_iter()
        .filter_map(|(field, value)| value.map(|value| (field, value)))
    {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!(
                "substance {name:?}: burst {field} must be finite and positive (got {value})"
            ));
        }
    }
    Ok(())
}

fn checked_hard(name: &str, substance: HardSubstance) -> Result<HardSubstance, String> {
    if let Some(friction) = substance.friction
        && !(friction.is_finite() && friction >= 0.0)
    {
        return Err(format!(
            "substance {name:?}: friction must be finite and not negative (got {friction})"
        ));
    }
    if let Some(bulk) = &substance.bulk {
        if substance.facts.is_some() {
            return Err(format!(
                "substance {name:?} states both material facts and bulk facts — each states a \
                 density, and a row states its density once"
            ));
        }
        let stated = [
            ("density", Some(bulk.density)),
            ("toughness", Some(bulk.toughness)),
            ("tensile", bulk.tensile),
            ("fracture_energy", bulk.fracture_energy),
        ];
        if bulk.tensile.is_some() != bulk.fracture_energy.is_some() {
            return Err(format!(
                "substance {name:?}: a seam is stated by its tensile strength and its fracture \
                 energy together, or not at all"
            ));
        }
        for (field, value) in stated
            .into_iter()
            .filter_map(|(field, value)| value.map(|value| (field, value)))
        {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "substance {name:?}: bulk {field} must be finite and positive (got {value})"
                ));
            }
        }
    }
    if let Some(facts) = &substance.facts {
        facts.validate().map_err(|reason| {
            format!("substance {name:?} has unusable material facts: {reason}")
        })?;
    }
    let resistance = match substance.resistance {
        Resistance::Reference => {
            if substance.facts.is_none() {
                return Err(format!(
                    "substance {name:?} is the reference but states no material facts — the \
                     reference's facts ARE the force law's reference constants"
                ));
            }
            Resistance::Reference
        }
        Resistance::Effective(factor) => {
            if !factor.is_finite() {
                return Err(format!(
                    "substance {name:?} has a non-finite effective factor ({factor}) — a factor \
                     is integrated along the chord, and NaN poisons the sum and the sort order"
                ));
            }
            if factor < 0.0 {
                return Err(format!(
                    "substance {name:?} has a negative effective factor ({factor}) — negative \
                     resistance breaks §13.6 monotonicity: adding the volume would LOWER \
                     protection along the ray"
                ));
            }
            // `-0.0` is canonicalized so "no resistance" has exactly one bit pattern; the walk
            // coalesces canonical spans by factor BITS, and a second zero would cut a span where
            // the field never changed.
            Resistance::Effective(if factor == 0.0 { 0.0 } else { factor })
        }
    };
    Ok(HardSubstance {
        resistance,
        ..substance
    })
}

/// Why a registry could not be used. Every arm names the offending text so the failure is readable
/// without a debugger.
#[derive(Debug, Clone, PartialEq)]
pub enum SubstanceError {
    /// The RON did not parse, or an entry was refused.
    Parse(String),
    /// The entries parsed, but the registry does not name exactly one reference.
    Reference(String),
    /// A lookup asked for a name the registry does not carry. Carries the name AND the registry's
    /// full vocabulary, because the usual cause is a Blender-side rename and the fix is to read off
    /// which spelling the file actually has.
    Unknown { name: String, known: Vec<String> },
    /// A hard-world lookup named a witness, which carries no resistance to read.
    Witness { name: String },
    /// A hard-world lookup named an unworn substance, which carries no resistance to read.
    Unworn { name: String },
}

impl std::fmt::Display for SubstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "materials.ron failed to parse: {err}"),
            Self::Reference(err) => write!(f, "materials.ron names no usable reference: {err}"),
            Self::Unknown { name, known } => write!(
                f,
                "unknown substance {name:?} — assets/materials/materials.ron declares [{}]. \
                 A ballistic mesh is assigned a material datablock that the registry does not \
                 name; either the .blend link drifted or the entry was never authored.",
                known.join(", ")
            ),
            Self::Witness { name } => write!(
                f,
                "substance {name:?} is a witness: it states no resistance and is never a layer of \
                 the walk"
            ),
            Self::Unworn { name } => write!(
                f,
                "substance {name:?} is worn by no volume: it states no resistance and is never a \
                 layer of the walk"
            ),
        }
    }
}

impl SubstanceRegistry {
    /// Parse a registry from RON text: every entry valid, and exactly one reference.
    pub fn from_ron(text: &str) -> Result<Self, SubstanceError> {
        let file: RegistryFile =
            ron::de::from_str(text).map_err(|err| SubstanceError::Parse(err.to_string()))?;
        let SubstanceMap {
            substances,
            visuals,
        } = file.substances;
        let references: Vec<&String> = substances
            .iter()
            .filter(|(_, substance)| {
                substance
                    .hard()
                    .is_some_and(|hard| hard.resistance == Resistance::Reference)
            })
            .map(|(name, _)| name)
            .collect();
        let reference = match references.as_slice() {
            [one] => (*one).clone(),
            [] => {
                return Err(SubstanceError::Reference(
                    "no substance is `resistance: Reference` — the force law has no reference \
                     constants and the reference-mm no definition"
                        .into(),
                ));
            }
            several => {
                return Err(SubstanceError::Reference(format!(
                    "{several:?} are all `resistance: Reference` — the reference-mm is a \
                     millimetre of ONE substance"
                )));
            }
        };
        Ok(Self {
            substances,
            visuals,
            reference,
        })
    }

    /// The shipped registry, parsed once.
    pub fn shipped_static() -> &'static Self {
        static REGISTRY: std::sync::LazyLock<SubstanceRegistry> =
            std::sync::LazyLock::new(SubstanceRegistry::shipped);
        &REGISTRY
    }

    /// The shipped registry. Panics on a malformed file, matching [`crate::bake`]'s treatment of the
    /// embedded tank spec: a broken authored contract is a build defect, not a runtime condition to
    /// degrade through.
    pub fn shipped() -> Self {
        Self::from_ron(MATERIALS_RON)
            .unwrap_or_else(|err| panic!("substances: embedded materials.ron is unusable: {err}"))
    }

    /// Every material datablock name the registry declares, in name order. The join key list
    /// itself: `materials.blend` carries exactly these datablocks, and the source lint holds the
    /// tank's materials to them through the canon file.
    pub fn keys(&self) -> Vec<&str> {
        self.substances.keys().map(String::as_str).collect()
    }

    /// Look a substance up by material datablock name. Hard error on an unknown name — never a
    /// default (module doc).
    pub fn get(&self, name: &str) -> Result<Substance, SubstanceError> {
        self.substances.get(name).copied().ok_or_else(|| {
            SubstanceError::Unknown {
                name: name.to_string(),
                // Already ordered: the table is a `BTreeMap`, so the diagnostic is stable run to run.
                known: self.substances.keys().cloned().collect(),
            }
        })
    }

    /// The hard substance named `name` — hard error on an unknown name, a witness or an unworn
    /// substance.
    pub fn hard(&self, name: &str) -> Result<HardSubstance, SubstanceError> {
        let name = name.to_string();
        match self.get(&name)? {
            Substance::Hard(hard) => Ok(hard),
            Substance::Witness => Err(SubstanceError::Witness { name }),
            Substance::Unworn { .. } => Err(SubstanceError::Unworn { name }),
        }
    }

    /// The visual key (`assets/materials/visuals.ron`) of the substance named `name`: `Some` for
    /// every hard substance, `None` for a witness, an unworn substance and a name the registry does
    /// not carry. View data: the walk never reads it.
    pub fn visual(&self, name: &str) -> Option<&str> {
        self.visuals.get(name).map(String::as_str)
    }

    /// The identity of the substance named `name` in this registry — hard error on an unknown name.
    pub fn id(&self, name: &str) -> Result<SubstanceId, SubstanceError> {
        self.get(name)?;
        let index = self
            .substances
            .keys()
            .position(|key| key == name)
            .expect("present, checked above");
        Ok(SubstanceId(index as u64))
    }

    /// The reference steel's material facts — the force law's reference constants.
    pub fn reference_facts(&self) -> MaterialFacts {
        self.substances[&self.reference]
            .hard()
            .and_then(|reference| reference.facts)
            .expect("the reference is hard and states its facts, checked at parse")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every substance the shipped library declares. A rename or a deletion has to come here first,
    /// which is the point: the Blender link key and the code's vocabulary are one contract.
    const EXPECTED: [&str; 15] = [
        "RHA",
        "GunSteel",
        "Cast",
        "MildSteel",
        "CastMild",
        "TrackSteel",
        "EngineBlock",
        "Ammunition",
        "Rubber",
        "TungstenCarbide",
        "Flesh",
        "Wood",
        "Brick",
        "88mm_burst_core",
        "Amatol60_40",
    ];

    /// The expected entries that are witnesses.
    const WITNESSES: [&str; 1] = ["Flesh"];

    /// The expected entries worn by no volume; every entry neither this nor a witness is hard.
    const UNWORN: [&str; 2] = ["88mm_burst_core", "Amatol60_40"];

    /// The expected entries that state burst facts, whatever their kind.
    const BURST_CORES: [&str; 2] = ["88mm_burst_core", "Amatol60_40"];

    /// A minimal valid registry around one extra entry, for the refusal tests.
    fn with(extra: &str) -> String {
        format!(
            r#"(substances: {{
                "Ref": Hard(resistance: Reference, visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: Some((
                        density: (7850.0, Tuned("fixture")),
                        flow_strength: (1.0e9, Tuned("fixture")),
                        youngs_modulus: (205.0e9, Tuned("fixture")),
                        poisson_ratio: (0.29, Tuned("fixture"))))),
                {extra}
            }})"#
        )
    }

    #[test]
    fn shipped_registry_carries_every_expected_substance() {
        let registry = SubstanceRegistry::shipped();
        assert_eq!(registry.substances.len(), EXPECTED.len());
        for name in EXPECTED {
            let substance = registry
                .get(name)
                .unwrap_or_else(|err| panic!("{name} must be in the registry: {err}"));
            match substance {
                Substance::Hard(hard) => assert!(
                    hard.factor() > 0.0,
                    "{name} must resist something (factor > 0)"
                ),
                Substance::Witness => assert!(
                    WITNESSES.contains(&name),
                    "{name} is a witness the library does not expect"
                ),
                Substance::Unworn { .. } => assert!(
                    UNWORN.contains(&name),
                    "{name} is an unworn substance the library does not expect"
                ),
            }
            assert_eq!(
                substance.burst().is_some(),
                BURST_CORES.contains(&name),
                "{name}: burst facts"
            );
        }
        for name in WITNESSES {
            assert_eq!(registry.get(name).unwrap(), Substance::Witness, "{name}");
        }
        for name in UNWORN {
            assert_eq!(registry.get(name).unwrap().matter(), None, "{name}");
        }
    }

    /// EVERY WORN SURFACE NAMES ITS LOOK. The tank surface material draws a primitive as the visual
    /// its substance names, so a hard row is refused without one and a blank one, and the key is
    /// the registry's to answer by name; a witness or an unworn row is worn by no drawn surface and
    /// names none. A mutant dropping the key at parse, or answering a witness with one, fails here.
    #[test]
    fn every_hard_substance_names_a_visual_and_nothing_else_does() {
        let registry = SubstanceRegistry::shipped();
        for name in registry.keys() {
            let visual = registry.visual(name);
            match registry.get(name).unwrap() {
                Substance::Hard(_) => assert!(
                    visual.is_some_and(|key| !key.trim().is_empty()),
                    "{name} is hard and names no visual"
                ),
                Substance::Witness | Substance::Unworn { .. } => {
                    assert_eq!(visual, None, "{name} is worn by no drawn surface")
                }
            }
        }
        assert_eq!(registry.visual("not a substance"), None);

        let parsed = SubstanceRegistry::from_ron(&with(
            r#""Mild": Hard(resistance: Effective((800.0, Tuned("fixture"))), visual: "cast", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),"#,
        ))
        .expect("a hard row naming a visual parses");
        assert_eq!(parsed.visual("Mild"), Some("cast"));
        for refused in [
            r#""Mild": Hard(resistance: Effective((800.0, Tuned("fixture"))), debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),"#,
            r#""Mild": Hard(resistance: Effective((800.0, Tuned("fixture"))), visual: " ", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),"#,
        ] {
            let err = SubstanceRegistry::from_ron(&with(refused))
                .expect_err("a hard row without a visual must not parse");
            assert!(matches!(err, SubstanceError::Parse(_)), "{err}");
        }
    }

    /// A ROW'S FRICTION IS OPTIONAL AND IN DOMAIN: absent reads as `None` (and `friction_of`
    /// refuses it, naming the row, for a body that must not fall back to Avian's default), a stated
    /// value is carried, and a negative or non-finite one is refused. The launched turret and the
    /// loose building pieces read it.
    #[test]
    fn a_rows_friction_is_optional_and_in_domain() {
        let row = |extra: &str| {
            with(&format!(
                r#""Clay": Hard(resistance: Effective((20.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None{extra}),"#
            ))
        };
        let bare = SubstanceRegistry::from_ron(&row("")).unwrap();
        assert_eq!(bare.hard("Clay").unwrap().friction, None);
        assert!(
            bare.hard("Clay")
                .unwrap()
                .friction_of("Clay")
                .unwrap_err()
                .contains("Clay")
        );
        let stated =
            SubstanceRegistry::from_ron(&row(r#", friction: Some((0.7, Tuned("fixture")))"#))
                .unwrap();
        assert_eq!(stated.hard("Clay").unwrap().friction_of("Clay"), Ok(0.7));
        for bad in ["-0.1", "inf"] {
            assert!(
                SubstanceRegistry::from_ron(&row(&format!(
                    r#", friction: Some(({bad}, Tuned("fixture")))"#
                )))
                .is_err(),
                "{bad}"
            );
        }
    }

    /// A ROW STATES ITS DENSITY ONCE: bulk facts give a non-metal its density, which the building
    /// bake reads for every chunk's mass, and a row stating them beside material facts (each with a
    /// density) is refused, as is a bulk fact out of its domain. A mutant reading the density off
    /// the wrong block, or letting two densities stand, fails here.
    #[test]
    fn bulk_facts_state_a_non_metal_s_density_once() {
        let bulk = r#"bulk: Some((density: (1800.0, Tuned("fixture")), toughness: (5.0e5, Tuned("fixture"))))"#;
        let row = |extra: &str| {
            with(&format!(
                r#""Clay": Hard(resistance: Effective((20.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), {extra}),"#
            ))
        };
        let registry = SubstanceRegistry::from_ron(&row(&format!("facts: None, {bulk}")))
            .expect("a non-metal with bulk facts parses");
        assert_eq!(registry.hard("Clay").unwrap().density(), Some(1800.0));
        assert_eq!(registry.hard("Ref").unwrap().density(), Some(7850.0));
        let bare = SubstanceRegistry::from_ron(&row("facts: None")).unwrap();
        assert_eq!(bare.hard("Clay").unwrap().density(), None);
        let both = format!(
            r#"facts: Some((density: (7850.0, Tuned("f")), flow_strength: (1.0e9, Tuned("f")), youngs_modulus: (205.0e9, Tuned("f")), poisson_ratio: (0.29, Tuned("f")))), {bulk}"#
        );
        for refused in [
            both,
            format!("facts: None, {}", bulk.replace("1800.0", "0.0")),
            format!("facts: None, {}", bulk.replace("5.0e5", "-1.0")),
            format!(
                "facts: None, {}",
                bulk.replace(
                    "))))",
                    r#")), tensile: Some((-1.0, Tuned("fixture"))), fracture_energy: Some((10.0, Tuned("fixture")))))"#
                )
            ),
            format!(
                "facts: None, {}",
                bulk.replace("))))", r#")), tensile: Some((1.0e6, Tuned("fixture")))))"#)
            ),
        ] {
            let err = SubstanceRegistry::from_ron(&row(&refused)).expect_err(&refused);
            assert!(matches!(err, SubstanceError::Parse(_)), "{err}");
        }
    }

    /// A witness is a bare `Witness`: it states no resistance, so a resistance on one does not parse,
    /// and it can be neither the reference nor counted as one.
    #[test]
    fn a_witness_carries_no_resistance() {
        let registry = SubstanceRegistry::from_ron(&with(r#""Crew": Witness,"#))
            .expect("a bare witness parses");
        assert_eq!(registry.get("Crew").unwrap(), Substance::Witness);
        assert_eq!(
            registry.get("Crew").unwrap().matter(),
            Some(Matter::Witness)
        );
        assert!(registry.get("Crew").unwrap().hard().is_none());
        for refused in [
            r#""Crew": Witness(resistance: Effective((10.0, Tuned("fixture")))),"#,
            r#""Crew": Witness(visual: "fixture"),"#,
        ] {
            let err = SubstanceRegistry::from_ron(&with(refused))
                .expect_err("a witness stating a field must not parse");
            assert!(matches!(err, SubstanceError::Parse(_)), "{err}");
        }
        let only = SubstanceRegistry::from_ron(r#"(substances: { "Crew": Witness })"#)
            .expect_err("a witness is no reference");
        assert!(matches!(only, SubstanceError::Reference(_)), "{only}");
    }

    /// A burst core worn by no volume states its two facts and nothing the walk reads: the walk's
    /// resistance lookup ([`SubstanceRegistry::hard`], which `ballistics::walk::VolumeMaterial::of`
    /// reads for every volume) refuses it by name, and it is no primitive's matter, so a volume
    /// that names it fails closed instead of joining the walk. An unworn row stating no burst facts,
    /// a fact out of its domain or without provenance, and a resistance on an unworn row do not
    /// load. A mutant that hands an unworn substance to the walk as hard matter of any factor, or as
    /// a witness, or that reads an unworn row stating nothing as a substance, fails here.
    #[test]
    fn an_unworn_burst_core_is_refused_by_the_walk_and_states_only_its_facts() {
        let entry = |density: &str| {
            with(&format!(
                r#""Core": Unworn(burst: Some((density: ({density}, Tuned("fixture")), speed_constant_mps: (2680.0, Tuned("fixture"))))),"#
            ))
        };
        let registry = SubstanceRegistry::from_ron(&entry("1650.0")).expect("a burst core parses");
        let core = registry.get("Core").unwrap();
        assert_eq!(
            core.burst(),
            Some(BurstFacts {
                density: 1650.0,
                speed_constant: 2680.0,
                chemical_energy: None,
            })
        );
        assert_eq!(core.matter(), None);
        assert!(core.hard().is_none());
        let refused = registry
            .hard("Core")
            .expect_err("the walk reads no unworn substance");
        assert_eq!(
            refused,
            SubstanceError::Unworn {
                name: "Core".into()
            }
        );
        let walked = crate::ballistics::VolumeMaterial::of(&registry, "Core")
            .expect_err("a volume naming an unworn substance fails closed");
        assert!(walked.to_string().contains("Core"), "{walked}");
        assert_eq!(registry.get("Ref").unwrap().burst(), None);

        for density in ["0.0", "-1.0", "NaN", "inf"] {
            let err = SubstanceRegistry::from_ron(&entry(density)).expect_err(density);
            assert!(
                err.to_string().contains("Core") && err.to_string().contains("density"),
                "{density}: {err}"
            );
        }
        let empty = r#""Core": Unworn(burst: None),"#;
        let err = SubstanceRegistry::from_ron(&with(empty)).expect_err(empty);
        assert!(
            err.to_string().contains("Core") && err.to_string().contains("no burst facts"),
            "{empty}: {err}"
        );
        for refused in [
            r#""Core": Unworn(burst: Some((density: 1650.0, speed_constant_mps: (2680.0, Tuned("fixture"))))),"#,
            r#""Core": Unworn(burst: Some((density: (1650.0, Tuned("fixture")), speed_constant_mps: (2680.0, Tuned("fixture")))), resistance: Reference),"#,
        ] {
            let err = SubstanceRegistry::from_ron(&with(refused)).expect_err(refused);
            assert!(matches!(err, SubstanceError::Parse(_)), "{err}");
        }
    }

    /// Burst facts are an optional field of a hard row too: a hard substance that states them is a
    /// burst core a round's block may name, and stays matter the walk reads at its own factor,
    /// unchanged by them; a hard row without them states none. Its burst facts are checked as an
    /// unworn row's are. A mutant that drops a hard row's burst facts, or lets them move what the
    /// walk reads, fails here.
    #[test]
    fn a_hard_row_may_state_burst_facts_too() {
        let hard = |burst: &str| {
            with(&format!(
                r#""Filler": Hard(resistance: Effective((200.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None{burst}),"#
            ))
        };
        let plain = SubstanceRegistry::from_ron(&hard("")).expect("a hard row without burst facts");
        let both = SubstanceRegistry::from_ron(&hard(
            r#", burst: Some((density: (1650.0, Tuned("fixture")), speed_constant_mps: (2680.0, Tuned("fixture"))))"#,
        ))
        .expect("a hard row with burst facts");
        assert_eq!(plain.get("Filler").unwrap().burst(), None);
        assert_eq!(
            both.get("Filler").unwrap().burst(),
            Some(BurstFacts {
                density: 1650.0,
                speed_constant: 2680.0,
                chemical_energy: None,
            })
        );
        assert_eq!(
            both.get("Filler").unwrap().matter(),
            plain.get("Filler").unwrap().matter()
        );
        assert_eq!(
            both.get("Filler").unwrap().matter(),
            Some(Matter::Hard {
                factor: 200.0,
                debris_ratio: plain.hard("Filler").unwrap().debris_ratio,
            })
        );
        let err = SubstanceRegistry::from_ron(&hard(
            r#", burst: Some((density: (1650.0, Tuned("fixture")), speed_constant_mps: (0.0, Tuned("fixture"))))"#,
        ))
        .expect_err("a speed constant of zero");
        assert!(
            err.to_string().contains("Filler") && err.to_string().contains("speed_constant_mps"),
            "{err}"
        );
    }

    #[test]
    fn unknown_name_is_an_error_naming_the_name_never_a_default() {
        let registry = SubstanceRegistry::shipped();
        let err = registry
            .get("Unobtainium")
            .expect_err("an unknown substance must not resolve");
        match &err {
            SubstanceError::Unknown { name, known } => {
                assert_eq!(name, "Unobtainium");
                assert!(known.contains(&"RHA".to_string()));
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
        assert!(err.to_string().contains("Unobtainium"));
    }

    #[test]
    fn an_unknown_field_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(&with(
            r#""Steel": Hard(resistance: Effective((900.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None, hardness: 3.0),"#,
        ))
        .expect_err("deny_unknown_fields must reject a stray key");
        assert!(matches!(err, SubstanceError::Parse(_)));
    }

    /// A factor or a fact authored without its provenance does not load.
    #[test]
    fn a_number_without_provenance_fails_the_parse() {
        let bare_factor = with(
            r#""Cast": Hard(resistance: Effective(900.0), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),"#,
        );
        let bare_fact = with(
            r#""Mild": Hard(resistance: Effective((800.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")),
                facts: Some((density: 7900.0, flow_strength: (0.4e9, Tuned("fixture")),
                youngs_modulus: (190.0e9, Tuned("fixture")), poisson_ratio: (0.29, Tuned("fixture"))))),"#,
        );
        for text in [bare_factor, bare_fact] {
            let err = SubstanceRegistry::from_ron(&text).expect_err("a bare number must not load");
            assert!(matches!(err, SubstanceError::Parse(_)), "{err}");
            assert!(err.to_string().contains("provenance"), "{err}");
        }
    }

    /// A duplicated datablock name means two different answers for one join key. Serde's default
    /// map fill would silently keep the last one; the file must be refused instead, naming the key.
    #[test]
    fn a_duplicated_substance_name_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(&with(
            r#""Cast": Hard(resistance: Effective((900.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),
               "Cast": Hard(resistance: Effective((10.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),"#,
        ))
        .expect_err("a duplicate key must not silently overwrite");
        assert!(err.to_string().contains("Cast"), "{err}");
        assert!(err.to_string().contains("twice"), "{err}");
    }

    /// A negative factor breaks §13.6 monotonicity — adding the volume would LOWER protection.
    #[test]
    fn a_negative_factor_fails_the_parse() {
        let err = SubstanceRegistry::from_ron(&with(
            r#""Rubber": Hard(resistance: Effective((-50.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),"#,
        ))
        .expect_err("a negative factor must not parse");
        assert!(err.to_string().contains("Rubber"), "{err}");
        assert!(err.to_string().contains("negative"), "{err}");
    }

    /// A non-finite factor poisons the chord integral and the total order the walk sorts on.
    #[test]
    fn a_non_finite_factor_fails_the_parse() {
        for literal in ["NaN", "inf", "-inf"] {
            let err = SubstanceRegistry::from_ron(&with(&format!(
                r#""Flesh": Hard(resistance: Effective(({literal}, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),"#
            )))
            .unwrap_err();
            assert!(
                err.to_string().contains("Flesh"),
                "factor {literal} must be refused by name, got {err}"
            );
        }
    }

    /// The debris half-angle converts once, at parse, to the field law's ratio `1/sin α` (90° to
    /// exactly 1, the hemisphere), and an angle outside `(0°, 90°]` — or one the law cannot
    /// resolve, or none at all — is refused naming the substance. The spall cone's width rests on it: an
    /// angle past 90° would throw back into the plate the cone leaves, and a ratio below 1 is a
    /// field the ball law does not have. Fixture angles.
    #[test]
    fn a_debris_half_angle_outside_the_quarter_turn_is_refused() {
        let entry = |degrees: &str| {
            with(&format!(
                r#""Odd": Hard(resistance: Effective((500.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: ({degrees}, Tuned("fixture")), facts: None),"#
            ))
        };
        let ratio = |degrees: &str| {
            SubstanceRegistry::from_ron(&entry(degrees))
                .unwrap()
                .hard("Odd")
                .unwrap()
                .debris_ratio
        };
        assert_eq!(ratio("90.0"), 1.0);
        let thirty = ratio("30.0");
        assert!((thirty - 2.0).abs() < 2.0 * f32::EPSILON, "{thirty}");
        assert!(
            ratio("26.2") > ratio("40.0"),
            "a narrower cone has the larger ratio"
        );
        for degrees in ["0.0", "-5.0", "90.5", "180.0", "NaN", "inf", "1.0e-9"] {
            let err = SubstanceRegistry::from_ron(&entry(degrees)).expect_err(degrees);
            assert!(err.to_string().contains("Odd"), "{degrees}: {err}");
            assert!(
                err.to_string().contains("debris half-angle"),
                "{degrees}: {err}"
            );
        }
        let missing = SubstanceRegistry::from_ron(&with(
            r#""Odd": Hard(resistance: Effective((500.0, Tuned("fixture"))), visual: "fixture", facts: None),"#,
        ))
        .expect_err("a hard entry states its debris half-angle");
        assert!(
            missing.to_string().contains("debris_half_angle_deg"),
            "{missing}"
        );
    }

    /// `-0.0` is canonicalized: "no resistance" gets exactly one bit pattern, because the walk
    /// coalesces its canonical spans by factor BITS and a second zero would cut a span where the
    /// field never changed.
    #[test]
    fn negative_zero_is_canonicalized() {
        let registry = SubstanceRegistry::from_ron(&with(
            r#""Void": Hard(resistance: Effective((-0.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None),"#,
        ))
        .expect("zero resistance is legal, it is just nothing");
        assert_eq!(
            registry
                .get("Void")
                .unwrap()
                .hard()
                .unwrap()
                .factor()
                .to_bits(),
            0.0f32.to_bits()
        );
    }

    /// Every fact out of its domain is refused by entry and by field.
    #[test]
    fn facts_out_of_domain_fail_the_parse() {
        let base = [
            ("density", "7850.0"),
            ("flow_strength", "1.0e9"),
            ("youngs_modulus", "205.0e9"),
            ("poisson_ratio", "0.29"),
        ];
        for (field, bad) in [
            ("density", "0.0"),
            ("density", "NaN"),
            ("flow_strength", "-1.0e9"),
            ("youngs_modulus", "inf"),
            ("poisson_ratio", "0.5"),
            ("poisson_ratio", "0.0"),
            // E / (3 (1 − ν) Y) far below e⁻¹: the cavity law states no positive strength.
            ("youngs_modulus", "1.0e6"),
        ] {
            let facts = base
                .iter()
                .map(|(name, value)| {
                    let value = if *name == field { bad } else { value };
                    format!(r#"{name}: ({value}, Tuned("fixture"))"#)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let err = SubstanceRegistry::from_ron(&with(&format!(
                r#""Odd": Hard(resistance: Effective((500.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: Some(({facts}))),"#
            )))
            .expect_err("facts out of domain must not parse");
            assert!(
                err.to_string().contains("Odd"),
                "{field} = {bad} must be refused by name, got {err}"
            );
        }
    }

    /// The reference states the force law's constants, so it must carry facts; and there is
    /// exactly one.
    #[test]
    fn the_reference_is_exactly_one_entry_with_facts() {
        let factless = SubstanceRegistry::from_ron(
            r#"(substances: { "Ref": Hard(resistance: Reference, visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None) })"#,
        )
        .expect_err("a reference without facts must not parse");
        assert!(factless.to_string().contains("Ref"), "{factless}");

        let none = SubstanceRegistry::from_ron(
            r#"(substances: { "Cast": Hard(resistance: Effective((900.0, Tuned("fixture"))), visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: None) })"#,
        )
        .expect_err("a registry without a reference must not load");
        assert!(matches!(none, SubstanceError::Reference(_)), "{none}");

        let two = SubstanceRegistry::from_ron(&with(
            r#""Other": Hard(resistance: Reference, visual: "fixture", debris_half_angle_deg: (26.2, Tuned("fixture")), facts: Some((
                density: (7800.0, Tuned("fixture")),
                flow_strength: (1.18e9, Tuned("fixture")),
                youngs_modulus: (190.0e9, Tuned("fixture")),
                poisson_ratio: (0.29, Tuned("fixture"))))),"#,
        ))
        .expect_err("two references must not load");
        assert!(matches!(two, SubstanceError::Reference(_)), "{two}");
    }

    /// At `ν = ½` the compressible cavity form reduces to the incompressible one,
    /// `(2/3) · (1 + ln(2E / (3Y))) · Y`, for any material.
    #[test]
    fn cavity_strength_reduces_to_the_incompressible_form() {
        for (flow_strength, youngs_modulus) in [
            (1.0e9f64, 205.0e9f64),
            (0.276e9, 69.0e9),
            (0.4e9, 190.0e9),
            (2.7e9, 600.0e9),
        ] {
            let incompressible = 2.0 / 3.0
                * (1.0 + (2.0 * youngs_modulus / (3.0 * flow_strength)).ln())
                * flow_strength;
            let got = MaterialFacts {
                density: 1.0,
                flow_strength,
                youngs_modulus,
                poisson_ratio: 0.5,
            }
            .cavity_strength();
            assert!(
                (got / incompressible - 1.0).abs() < 1.0e-12,
                "Y {flow_strength}, E {youngs_modulus}: {got} vs {incompressible}"
            );
        }
    }

    /// The reference steel's facts give the DERIVED strength term
    /// `(2/3) · (1 + ln(205e9 / (3 · 0.71 · 1e9))) · 1e9 ≈ 3.711_258_7e9` Pa, to f32 precision.
    #[test]
    fn the_reference_cavity_strength_is_the_derived_value() {
        let a = SubstanceRegistry::shipped()
            .reference_facts()
            .cavity_strength();
        assert!(
            (a / 3.711_258_7e9 - 1.0).abs() < f32::EPSILON as f64,
            "strength term {a}"
        );
    }
}
