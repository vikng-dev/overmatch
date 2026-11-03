//! The GROUND-TYPE TABLE: each kind of ground the map is made of, from squishy mud to hard concrete,
//! as data (`assets/terrain/ground_types.ron`). Each terrain pack names the type it is
//! (`terrain_blend::BlendLayer::ground`, `terrain_blend::BASE_GROUND`).
//!
//! A type carries how it takes a track print (cosmetic terrain marks, view only,
//! [`crate::track_prints`]): [`PrintResponse`]; the friction coefficient it brings to a contact
//! ([`TractionLaw::Rigid`], `crate::substances` module doc, "Friction"); the rolling resistance
//! it puts on a moving track ([`MotionResistance`]); how readily a muzzle blast lifts
//! it as dust ([`DustResponse`], view only, `vfx::muzzle`); and what a round's strike throws from it
//! ([`StrikeResponse`], view only, `vfx::impact`). A type is a record with one block
//! per consumer, so the handling a type will drive later (sinking) joins as a sibling of
//! [`GroundType::print`] without touching the print's fields. Every field is required: the table
//! states each value with its provenance (below), and nothing in code stands in for one. A missing
//! field, a field the table does not know, a value outside its domain, and a table lacking a
//! ground a terrain pack names are all refused at load, naming the ground and the field
//! ([`GroundTypes::parse`]).
//!
//! # What the simulation reads
//!
//! The track law reads each terrain contact's own ground: the map's painted blend weights, decoded
//! once at the data edge into the sim's [`crate::ground_field::GroundField`], blend the four packs'
//! soil capacities, rigid pair capacities and rolling resistances at the contact's point (`track::forces::contact_side`). The
//! terrain's one Avian collider carries an explicit solid-body solver stand-in
//! ([`GroundTypes::terrain_friction`]): the hull's and the pieces' solver contacts with the ground
//! read that stand-in; the solver has no soil contact footprint. The law's narrative is the track design doc's
//! (`.agents/docs/design/track-model/architecture.md`, "Ground under a contact").
//!
//! # Rolling resistance
//!
//! `rolling_resistance` is a ground's coefficient of motion resistance for a tracked vehicle, as the
//! published tables state it per ground (soil compaction and the running gear's own losses
//! lumped): the force a moving track loses is that coefficient times its normal load. The table
//! labels each value with its job ([`crate::provenance::Labelled`]), and the tuning panel's save
//! writes the label back, so a saved table keeps every value's source.
//!
//! # A print's depth
//!
//! One number per tank per ground type: the type's softness times the tank's ground pressure
//! (mass over track footprint, both read from tank data — `track_prints::ground_pressure_kpa`),
//! capped by the type ([`PrintResponse::depth_m`]). No per-contact load, no weight distribution;
//! the baked link shape supplies the pattern inside that depth (`track_prints::store`).
//!
//! # Where the starting values come from
//!
//! Print values are ASSUMED; traction and rolling facts carry their own provenance. Print ordering follows the ground-stamping note's §2
//! table (`.agents/scratch/impact-feedback-2026-10-28/track-prints-ground-stamping.md`): its
//! Bekker fits at a DERIVED 111 kPa put soft mud deepest (held at the note's 0.15 m visual cap), dry
//! sand next (92 mm, so 0.8 mm/kPa), then firm dirt (5.9 mm) and gravel (7.6 mm), and intact paving
//! at zero. Dirt and gravel are raised to about 5 cm under a 104 kPa fixture tank, because at the
//! note's depths the base ground's prints were nearly invisible in play, and at 3 cm still faint in
//! the slice's captures on the busy, stony base pack; a print must read, whole footprint and
//! pattern, at 10–30 m on every shipped pack. Darkening and roughness start from the first slice's
//! per-pack looks, dirt and gravel darkened further for the same reason. The owner tunes them by
//! eye (`track_prints::tuning`), and the chosen values are written back here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::provenance::Labelled;

/// The table's path under the asset root.
pub(crate) const GROUND_TYPES_PATH: &str = "terrain/ground_types.ron";

/// How one ground type takes a track print. Each field's domain is [`GroundTypes::parse`]'s.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrintResponse {
    /// Print depth per unit of ground pressure, mm per kPa: finite, not negative.
    pub(crate) softness_mm_per_kpa: f32,
    /// The deepest print this ground takes, mm, whatever the pressure: finite, not negative.
    pub(crate) depth_cap_mm: f32,
    /// The share of the ground's albedo a print takes away where a sole lay, 0..=1.
    pub(crate) darkening: f32,
    /// Roughness multiplier where a sole lay, finite and positive: below 1 reads wet.
    pub(crate) roughness_gain: f32,
}

impl PrintResponse {
    /// The shipping depth law for one ground at full weight, at finite, nonnegative `kpa`.
    #[cfg(any(feature = "dev_ui", test))]
    pub(crate) fn depth_m(&self, kpa: f32) -> f32 {
        let params = crate::terrain_blend::TerrainBlendParams {
            print_softness: Vec4::splat(self.softness_mm_per_kpa / 1000.0),
            print_cap: Vec4::splat(self.depth_cap_mm / 1000.0),
            ..Default::default()
        };
        crate::terrain_blend::print_depth_m(&params, Vec4::X, kpa)
    }

    /// The albedo multiplier where a sole lay: 0..=1, as `darkening` is.
    pub(crate) fn albedo_gain(&self) -> f32 {
        1.0 - self.darkening
    }
}

/// How readily one ground type gives up dust to a muzzle blast, and what that dust looks like
/// (view only, `vfx::muzzle`). The domain is [`GroundTypes::parse`]'s.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DustResponse {
    /// The share of a gun's authored dust cloud this ground lifts, 0..=1: loose dry fines give
    /// it all, paving or wet cohesive ground little.
    pub(crate) erodibility: f32,
    /// The lifted soil's albedo, sRGB, each channel 0..=1: the colour of the ground's own fines,
    /// which the key light lights like the ground they came from.
    pub(crate) colour_srgb: [f32; 3],
}

/// What a round's strike on this ground throws (view only, `vfx::impact`): its own fact beside
/// [`DustResponse`], since a muzzle blast's gas and a round's strike do not lift a ground alike.
/// The thrown soil's colour is [`DustResponse::colour_srgb`]. The domain is [`GroundTypes::parse`]'s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StrikeResponse {
    /// The share of a strike's authored powder read this ground throws, 0..=1.
    pub(crate) lifted: Labelled<f32>,
    /// Whether a strike on this ground throws contact sparks.
    pub(crate) sparks: Labelled<bool>,
}

/// One painted ground's part in a strike, per [`GroundTypes::strike_sources`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StrikeSource {
    pub(crate) colour_srgb: [f32; 3],
    pub(crate) sparks: bool,
}

/// The surface's constitutive choice; rigid paving needs no soil parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum TractionLaw {
    Rigid {
        friction: Labelled<f32>,
    },
    Soil {
        cohesion_pa: Labelled<f32>,
        friction_angle_deg: Labelled<f32>,
        shear_deformation_m: Labelled<f32>,
    },
}

/// A total ground resistance includes running-gear losses. Undeforming support reads the
/// same running-gear fact as a rigid piece, rather than repeating its number in a row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum MotionResistance {
    RunningGear,
    Total(Labelled<f32>),
}

impl MotionResistance {
    pub(crate) fn coefficient(&self) -> f32 {
        match self {
            Self::RunningGear => crate::track::forces::rigid_rolling_resistance(),
            Self::Total(value) => value.value,
        }
    }
}

/// One ground type: one block per thing the ground drives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroundType {
    pub(crate) traction: TractionLaw,
    pub(crate) print: PrintResponse,
    /// Total motion resistance, or the shared running-gear fact on undeforming support.
    pub(crate) rolling_resistance: MotionResistance,
    pub(crate) dust: DustResponse,
    pub(crate) strike: StrikeResponse,
}

impl GroundType {
    /// What this ground brings to a track contact standing wholly on it.
    pub(crate) fn contact(&self) -> crate::ground_field::ContactGround {
        crate::ground_field::ContactGround {
            friction: match &self.traction {
                TractionLaw::Rigid { friction } => friction.value,
                TractionLaw::Soil { .. } => 0.0,
            },
            traction: match &self.traction {
                TractionLaw::Rigid { .. } => crate::ground_field::ContactTraction::RIGID,
                TractionLaw::Soil {
                    cohesion_pa,
                    friction_angle_deg,
                    shear_deformation_m,
                } => crate::ground_field::ContactTraction {
                    cohesion_pa: cohesion_pa.value,
                    tan_phi: friction_angle_deg.value.to_radians().tan(),
                    shear_m: shear_deformation_m.value,
                    rigid_share: 0.0,
                },
            },
            rolling_resistance: self.rolling_resistance.coefficient(),
        }
    }
}

/// Every terrain pack's ground in the shader's slot order: the base, then the blend layers.
const PACK_GROUNDS: [&str; 4] = [
    crate::terrain_blend::BASE_GROUND,
    crate::terrain_blend::BLEND_LAYERS[0].ground,
    crate::terrain_blend::BLEND_LAYERS[1].ground,
    crate::terrain_blend::BLEND_LAYERS[2].ground,
];

/// The table, keyed by the name a pack's contract uses.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroundTypes {
    /// Stand-in for Avian solid-body contacts, which have no soil footprint area.
    pub(crate) terrain_solver_friction: Labelled<f32>,
    pub(crate) types: BTreeMap<String, GroundType>,
}

/// The comment the table is written back under: the file stays self-describing after a save.
#[cfg(any(feature = "dev_ui", test))]
const FILE_HEADER: &str = "\
// The ground-type table (`src/ground_types.rs`): physical traction and motion resistance,
// and cosmetic prints and dust. Each terrain pack names its type (`src/terrain_blend.rs`).
//
//   softness_mm_per_kpa  print depth per unit of the tank's ground pressure, mm per kPa
//   depth_cap_mm         the deepest print this ground takes, mm
//   darkening            the share of albedo a print takes away, 0..1
//   roughness_gain       roughness multiplier under a sole; below 1 reads wet
//   traction.friction    rigid surfaces only: their labelled sliding coefficient
//   traction             rigid friction or labelled soil cohesion, friction angle and shear distance
//   rolling_resistance   the ground's motion resistance coefficient on a moving track, labelled
//   dust.erodibility     the share of a gun's muzzle dust cloud this ground lifts, 0..1
//   dust.colour_srgb     the lifted soil's albedo, sRGB, each channel 0..1
//   strike.lifted        the share of a round's strike read this ground throws, 0..1, labelled
//   strike.sparks        whether a strike on this ground throws contact sparks, labelled
//
// Every print value is ASSUMED: ordered by the Bekker fits in the ground-stamping note's §2, dirt
// and gravel raised so a print reads at 10-30 m, then tuned by eye in play (`track_prints::tuning`).
// Rigid traction owns its labelled sliding coefficient; soil owns only its strength facts.
// terrain_solver_friction is the explicit Avian solid-body stand-in, not track grip.
// RunningGear reads grip.ron's shared undeforming-support resistance; Total includes it.
// Each strike value carries its own label and note. Every erodibility is ASSUMED, no source read: ordered by hand, loose dry dirt
// and sand giving the whole cloud, gravel most of it, wet mud and intact paving little. Each
// pack's dust colour is DERIVED: the mean of the pack's shipped full-resolution albedo slice
// (`coast_sand_rocks_02_diff.ktx2`, `blend_diff.ktx2`), MEASURED; concrete names no pack, and its
// light grey is ASSUMED.
";

/// Why the table could not be read or written: the file's I/O, its RON, a value [`GroundTypes::parse`]
/// refuses, or the table's own serialization. Each keeps its source, so the editor's status line
/// (`track_prints::tuning`) and the load-time panic say which.
#[derive(Debug)]
pub(crate) enum GroundTableError {
    /// Reading or writing `path` failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The text is not the table's RON (a malformed file, a missing or unknown field).
    Ron(ron::error::SpannedError),
    /// The text parses and states a value outside its domain, or lacks a pack's ground: the
    /// refusal names the ground and the field.
    Refused(String),
    /// The table could not be written as RON.
    #[cfg(any(feature = "dev_ui", test))]
    Serialize(ron::Error),
}

impl std::fmt::Display for GroundTableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Ron(err) => write!(f, "{err}"),
            Self::Refused(why) => f.write_str(why),
            #[cfg(any(feature = "dev_ui", test))]
            Self::Serialize(err) => write!(f, "cannot write the table as RON: {err}"),
        }
    }
}

impl GroundTypes {
    /// The table at `path`, or why it could not be read.
    pub(crate) fn load(path: &Path) -> Result<Self, GroundTableError> {
        let text = std::fs::read_to_string(path).map_err(|source| GroundTableError::Io {
            path: path.to_owned(),
            source,
        })?;
        crate::provenance::with_shipped_policy(|| Self::parse(&text))
    }

    /// The table `text` holds, or why it is refused: a malformed file, a missing field or one the
    /// table does not know, a value outside its domain (a negative or non-finite friction is a NaN
    /// contact friction under the pair rule's square root; a darkening outside 0..=1 is an albedo
    /// gain no material holds; a negative depth or softness prints upward), or a table lacking a
    /// ground a terrain pack names ([`crate::terrain_blend::BASE_GROUND`],
    /// [`crate::terrain_blend::BLEND_LAYERS`]).
    pub(crate) fn parse(text: &str) -> Result<Self, GroundTableError> {
        let table: Self = ron::de::from_str(text).map_err(GroundTableError::Ron)?;
        if !table.terrain_solver_friction.value.is_finite()
            || table.terrain_solver_friction.value < 0.0
            || !matches!(
                table.terrain_solver_friction.label,
                crate::provenance::Provenance::StandIn(_)
                    | crate::provenance::Provenance::Fixture(_)
            )
        {
            return Err(GroundTableError::Refused(format!(
                "{GROUND_TYPES_PATH}: terrain_solver_friction must be finite, nonnegative and StandIn; state the missing solid-body/soil contact law"
            )));
        }
        let fact = |value: &Labelled<f32>| {
            matches!(
                value.label,
                crate::provenance::Provenance::Measured(_)
                    | crate::provenance::Provenance::Derived(_)
                    | crate::provenance::Provenance::Unknown(_)
                    | crate::provenance::Provenance::Fixture(_)
            )
        };
        for (name, ground) in &table.types {
            if let TractionLaw::Rigid { friction } = &ground.traction
                && !fact(friction)
            {
                return Err(GroundTableError::Refused(format!(
                    "{GROUND_TYPES_PATH}: ground {name:?}: traction.friction must state a fact or Unknown; measure the rigid sliding pair"
                )));
            }
            if let MotionResistance::Total(value) = &ground.rolling_resistance
                && !fact(value)
            {
                return Err(GroundTableError::Refused(format!(
                    "{GROUND_TYPES_PATH}: ground {name:?}: rolling_resistance must state a fact or Unknown; measure a coast-down"
                )));
            }
            if let TractionLaw::Soil {
                cohesion_pa,
                friction_angle_deg,
                shear_deformation_m,
            } = &ground.traction
            {
                for (field, labelled) in [
                    ("cohesion_pa", cohesion_pa),
                    ("friction_angle_deg", friction_angle_deg),
                    ("shear_deformation_m", shear_deformation_m),
                ] {
                    if !matches!(
                        labelled.label,
                        crate::provenance::Provenance::Measured(_)
                            | crate::provenance::Provenance::Derived(_)
                            | crate::provenance::Provenance::Unknown(_)
                            | crate::provenance::Provenance::Fixture(_)
                    ) {
                        return Err(GroundTableError::Refused(format!(
                            "{GROUND_TYPES_PATH}: ground {name:?}: traction.{field} must state a fact or Unknown with its settling measurement, never a taste or solver setting"
                        )));
                    }
                }
                for (field, value, valid) in [
                    (
                        "traction.cohesion_pa",
                        cohesion_pa.value,
                        cohesion_pa.value.is_finite() && cohesion_pa.value >= 0.0,
                    ),
                    (
                        "traction.friction_angle_deg",
                        friction_angle_deg.value,
                        (0.0..90.0).contains(&friction_angle_deg.value),
                    ),
                    (
                        "traction.shear_deformation_m",
                        shear_deformation_m.value,
                        shear_deformation_m.value.is_finite() && shear_deformation_m.value > 0.0,
                    ),
                ] {
                    if !valid {
                        return Err(GroundTableError::Refused(format!(
                            "{GROUND_TYPES_PATH}: ground {name:?}: `{field}` is {value}; supply finite nonnegative cohesion, an angle in [0,90), and positive shear deformation"
                        )));
                    }
                }
            }
            let print = &ground.print;
            for (field, value, in_domain, domain) in [
                (
                    "friction",
                    ground.contact().friction,
                    ground.contact().friction.is_finite() && ground.contact().friction >= 0.0,
                    "a finite coefficient of 0 or more",
                ),
                (
                    "rolling_resistance",
                    ground.rolling_resistance.coefficient(),
                    ground.rolling_resistance.coefficient().is_finite()
                        && ground.rolling_resistance.coefficient() >= 0.0,
                    "a finite coefficient of 0 or more",
                ),
                (
                    "dust.erodibility",
                    ground.dust.erodibility,
                    (0.0..=1.0).contains(&ground.dust.erodibility),
                    "a share in 0..=1",
                ),
                (
                    "dust.colour_srgb",
                    ground.dust.colour_srgb[0],
                    (0.0..=1.0).contains(&ground.dust.colour_srgb[0]),
                    "an sRGB channel in 0..=1",
                ),
                (
                    "dust.colour_srgb",
                    ground.dust.colour_srgb[1],
                    (0.0..=1.0).contains(&ground.dust.colour_srgb[1]),
                    "an sRGB channel in 0..=1",
                ),
                (
                    "dust.colour_srgb",
                    ground.dust.colour_srgb[2],
                    (0.0..=1.0).contains(&ground.dust.colour_srgb[2]),
                    "an sRGB channel in 0..=1",
                ),
                (
                    "strike.lifted",
                    ground.strike.lifted.value,
                    (0.0..=1.0).contains(&ground.strike.lifted.value),
                    "a share in 0..=1",
                ),
                (
                    "print.softness_mm_per_kpa",
                    print.softness_mm_per_kpa,
                    print.softness_mm_per_kpa.is_finite() && print.softness_mm_per_kpa >= 0.0,
                    "finite and 0 or more",
                ),
                (
                    "print.depth_cap_mm",
                    print.depth_cap_mm,
                    print.depth_cap_mm.is_finite() && print.depth_cap_mm >= 0.0,
                    "finite and 0 or more",
                ),
                (
                    "print.darkening",
                    print.darkening,
                    (0.0..=1.0).contains(&print.darkening),
                    "a share in 0..=1",
                ),
                (
                    "print.roughness_gain",
                    print.roughness_gain,
                    print.roughness_gain.is_finite() && print.roughness_gain > 0.0,
                    "a finite multiplier above 0",
                ),
            ] {
                if !in_domain {
                    return Err(GroundTableError::Refused(format!(
                        "ground type {name:?}: `{field}` is {value}; state it as {domain}"
                    )));
                }
            }
        }
        for ground in PACK_GROUNDS {
            if !table.types.contains_key(ground) {
                return Err(GroundTableError::Refused(format!(
                    "a terrain pack is ground type {ground:?}, which the table lacks (it has \
                     {:?}); add a {ground:?} entry with its print block and friction",
                    table.types.keys().collect::<Vec<_>>()
                )));
            }
        }
        Ok(table)
    }

    /// The shipped table's path.
    pub(crate) fn shipped_path() -> PathBuf {
        crate::assets::asset_root().join(GROUND_TYPES_PATH)
    }

    /// The shipped table. A missing or unreadable table is a broken build, not a ground with no
    /// prints, so it aborts naming the file (ADR-0011).
    pub(crate) fn load_shipped() -> Self {
        let path = Self::shipped_path();
        Self::load(&path)
            .unwrap_or_else(|err| panic!("the ground-type table {}: {err}", path.display()))
    }

    /// Avian solid-body contacts have no soil footprint; their explicit solver stand-in
    /// remains separate from the track's constitutive ground rows.
    pub(crate) fn terrain_friction(&self) -> avian3d::prelude::Friction {
        crate::substances::contact_friction(self.terrain_solver_friction.value)
    }

    /// The type named `name`. Every pack's ground is a key ([`Self::parse`]), so a lookup that
    /// misses is a caller naming a ground no pack is, and aborts naming both.
    pub(crate) fn get(&self, name: &str) -> &GroundType {
        self.types.get(name).unwrap_or_else(|| {
            panic!(
                "a terrain pack names ground type {name:?}, which the ground-type table lacks \
                 (it has {:?})",
                self.types.keys().collect::<Vec<_>>()
            )
        })
    }

    /// What a strike on ground of print weights `weights`
    /// ([`crate::terrain_blend::print_weights`]) throws, per slot: its share of a fully lifting
    /// ground's read (the slot's painted weight times its [`StrikeResponse::lifted`]) and its soil.
    /// The shares sum to the blended lift, in 0..=1.
    pub(crate) fn strike_sources(&self, weights: Vec4) -> [(f32, StrikeSource); 4] {
        std::array::from_fn(|slot| {
            let ground = self.get(PACK_GROUNDS[slot]);
            (
                weights[slot] * ground.strike.lifted.value,
                StrikeSource {
                    colour_srgb: ground.dust.colour_srgb,
                    sparks: ground.strike.sparks.value,
                },
            )
        })
    }

    /// The table as the file holds it.
    #[cfg(any(feature = "dev_ui", test))]
    pub(crate) fn to_file_text(&self) -> Result<String, GroundTableError> {
        let body = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())
            .map_err(GroundTableError::Serialize)?;
        Ok(format!("{FILE_HEADER}{body}\n"))
    }

    /// Write the table to `path`, replacing it through a sibling temporary file so a failed write
    /// never leaves half a table.
    #[cfg(feature = "dev_ui")]
    pub(crate) fn save(&self, path: &Path) -> Result<(), GroundTableError> {
        let text = self.to_file_text()?;
        let temporary = path.with_extension("ron.tmp");
        std::fs::write(&temporary, text).map_err(|source| GroundTableError::Io {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, path).map_err(|source| GroundTableError::Io {
            path: path.to_owned(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal table text: every ground a terrain pack names, each with every field
    /// authored, the first one (`gravel`, the base pack's) carrying `gravel` in place of its block.
    /// FIXTURE DATA: the numbers are round and stand for no ground.
    fn table_text(gravel: &str) -> String {
        let block = "traction: Rigid(friction: (0.5, Fixture(\"fixture\"))), print: (softness_mm_per_kpa: 0.5, depth_cap_mm: 50.0, darkening: 0.2, \
                     roughness_gain: 1.0), rolling_resistance: Total((0.05, \
                     Fixture(\"fixture\"))), dust: (erodibility: 0.5, colour_srgb: (0.5, 0.4, 0.3)), \
                     strike: (lifted: (0.5, Fixture(\"fixture\")), sparks: (false, \
                     Fixture(\"fixture\")))";
        format!(
            "(terrain_solver_friction: (0.5, Fixture(\"fixture\")), types: {{\"gravel\": ({gravel}), \"dirt\": ({block}), \"sand\": ({block}), \
             \"mud\": ({block})}})"
        )
    }

    /// A RIGID GROUND PRINTS NO DEPTH: a ground with no softness takes no print at any pressure,
    /// which is what keeps a road a road under the heaviest tank. FIXTURE DATA (a labelled
    /// zero-softness response); the class invariant holds for every shipped row too: a row prints
    /// exactly when its softness and its cap are both above zero.
    #[test]
    fn a_ground_with_no_softness_prints_no_depth() {
        let rigid = PrintResponse {
            softness_mm_per_kpa: 0.0,
            depth_cap_mm: 150.0,
            darkening: 0.0,
            roughness_gain: 1.0,
        };
        for kpa in [0.0, 50.0, 111.0, 255.0] {
            assert_eq!(rigid.depth_m(kpa), 0.0, "a rigid ground at {kpa} kPa");
        }
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        for (name, ground) in &table.types {
            let print = ground.print;
            let prints = print.softness_mm_per_kpa > 0.0 && print.depth_cap_mm > 0.0;
            assert_eq!(print.depth_m(100.0) > 0.0, prints, "{name}");
        }
    }

    /// HIGHER GROUND PRESSURE PRINTS DEEPER on the same ground, until the ground's cap holds it:
    /// a heavier tank on the same footprint sinks further, and no tank sinks past what the ground
    /// allows. Two pressures, no tank: the law reads the pressure and nothing else about a vehicle.
    #[test]
    fn higher_ground_pressure_prints_deeper_on_the_same_ground() {
        // Fixture: keep the cap law exercised even if every shipped ground is tuned to be rigid.
        let deformable = PrintResponse {
            softness_mm_per_kpa: 0.5,
            depth_cap_mm: 50.0,
            darkening: 0.0,
            roughness_gain: 1.0,
        };
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        for (name, print) in std::iter::once(("inline fixture", deformable)).chain(
            table
                .types
                .iter()
                .map(|(name, ground)| (name.as_str(), ground.print)),
        ) {
            if print.softness_mm_per_kpa <= 0.0 || print.depth_cap_mm <= 0.0 {
                // Grounds that cannot print have no increasing interval.
                continue;
            }
            // Both witnesses must be below this ground's cap: a valid live-tuned table may
            // saturate at either of the fixed pressures a fixture happened to choose.
            let cap_kpa = print.depth_cap_mm / print.softness_mm_per_kpa;
            let (light, heavy) = (0.25 * cap_kpa, 0.5 * cap_kpa);
            assert!(
                print.depth_m(heavy) > print.depth_m(light),
                "{name}: {heavy} kPa prints deeper than {light} kPa"
            );
            assert!(
                (print.depth_m(2.0 * cap_kpa) - print.depth_cap_mm / 1000.0).abs() < 1e-6,
                "{name}: the cap holds any pressure"
            );
        }
    }

    /// A GROUND THAT STATES TOO LITTLE IS REFUSED AT LOAD: a ground with no `print` block would
    /// take no print, a rigid law with no `friction` would lack a sliding-pair fact,
    /// a ground with no `rolling_resistance` (or one with no label) would roll on a
    /// coefficient nobody stated, a ground with no `dust` would lift a cloud nobody stated, a ground
    /// with no `strike` would answer a round with a muzzle number or nothing, and a
    /// table lacking a pack's ground would leave that pack nothing to read. Each is refused at
    /// parse, naming what is missing, so no consumer meets one. Mutant: a `#[serde(default)]` on a
    /// required field (including each soil fact), or a parse that skips the pack check, loads one
    /// of these. The terrain collider reads the table's explicit solid-body solver stand-in.
    #[test]
    fn a_ground_that_states_too_little_is_refused_at_load() {
        let block = "traction: Rigid(friction: (0.5, Fixture(\"fixture\"))), print: (softness_mm_per_kpa: 0.5, depth_cap_mm: 50.0, darkening: 0.2, \
                     roughness_gain: 1.0)";
        let dust = "dust: (erodibility: 0.5, colour_srgb: (0.5, 0.4, 0.3)), strike: (lifted: (0.5, \
                    Fixture(\"fixture\")), sparks: (false, Fixture(\"fixture\")))";
        let bare_dust = "dust: (erodibility: 0.5, colour_srgb: (0.5, 0.4, 0.3))";
        let rolling = "rolling_resistance: Total((0.05, Fixture(\"fixture\")))";
        let with_strike =
            |strike: &str| format!("{block}, {rolling}, {bare_dust}, strike: ({strike})");
        for (missing, gravel) in [
            (
                "print",
                format!(
                    "traction: Rigid(friction: (0.5, Fixture(\"fixture\"))), {rolling}, {dust}"
                ),
            ),
            ("strike", format!("{block}, {rolling}, {bare_dust}")),
            (
                "lifted",
                with_strike("sparks: (false, Fixture(\"fixture\"))"),
            ),
            ("sparks", with_strike("lifted: (0.5, Fixture(\"fixture\"))")),
            (
                "provenance",
                with_strike("lifted: (0.5, Fixture(\"fixture\")), sparks: false"),
            ),
            (
                "friction",
                format!("{block}, {rolling}, {dust}")
                    .replace("friction: (0.5, Fixture(\"fixture\"))", ""),
            ),
            ("rolling_resistance", format!("{block}, {dust}")),
            (
                "provenance",
                format!("{block}, rolling_resistance: Total(0.05), {dust}"),
            ),
            (
                "dust",
                format!(
                    "{block}, {rolling}, strike: (lifted: (0.5, Fixture(\"fixture\")), sparks: (false, Fixture(\"fixture\")))"
                ),
            ),
            (
                "colour_srgb",
                format!(
                    "{block}, {rolling}, dust: (erodibility: 0.5), strike: (lifted: (0.5, Fixture(\"fixture\")), sparks: (false, Fixture(\"fixture\")))"
                ),
            ),
            (
                "erodibility",
                format!(
                    "{block}, {rolling}, dust: (colour_srgb: (0.5, 0.4, 0.3)), strike: (lifted: (0.5, Fixture(\"fixture\")), sparks: (false, Fixture(\"fixture\")))"
                ),
            ),
        ] {
            let error = GroundTypes::parse(&table_text(&gravel))
                .expect_err(missing)
                .to_string();
            assert!(error.contains(missing), "{missing}: {error}");
        }
        let without_mud = table_text(&format!("{block}, {rolling}, {dust}"))
            .replace(", \"mud\": (", ", \"marsh\": (");
        let error = GroundTypes::parse(&without_mud)
            .expect_err("a pack's ground is missing")
            .to_string();
        assert!(error.contains("\"mud\""), "{error}");
        let full = table_text(&format!("{block}, {rolling}, {dust}"));
        let table = GroundTypes::parse(&full).expect("the full fixture table loads");
        assert_eq!(
            table.terrain_friction().combine_rule,
            crate::substances::PAIR_RULE
        );
    }

    /// Missing, unlabelled or invalid soil facts must be refused before a contact can read
    /// them. The traction choice is required even on rigid surfaces; no silent default law.
    #[test]
    fn a_traction_law_requires_its_own_valid_facts() {
        // FIXTURE table with one soil row; the remaining rows are valid rigid surfaces.
        let base = "print: (softness_mm_per_kpa: 0.5, depth_cap_mm: 50.0, darkening: 0.2, roughness_gain: 1.0), rolling_resistance: Total((0.05, Fixture(\"fixture\"))), dust: (erodibility: 0.5, colour_srgb: (0.5, 0.4, 0.3)), strike: (lifted: (0.5, Fixture(\"fixture\")), sparks: (false, Fixture(\"fixture\")))";
        assert!(GroundTypes::parse(&table_text(base)).is_err());
        let law = "Soil(cohesion_pa: (1000.0, Fixture(\"fixture\")), friction_angle_deg: (30.0, Fixture(\"fixture\")), shear_deformation_m: (0.01, Fixture(\"fixture\")))";
        let text = table_text(&format!("traction: {law}, {base}"));
        GroundTypes::parse(&text).expect("a complete soil row loads");
        for (from, to, field) in [
            ("cohesion_pa: (1000.0", "cohesion_pa: (-1.0", "cohesion_pa"),
            (
                "friction_angle_deg: (30.0",
                "friction_angle_deg: (90.0",
                "friction_angle_deg",
            ),
            (
                "shear_deformation_m: (0.01",
                "shear_deformation_m: (0.0",
                "shear_deformation_m",
            ),
            (
                "shear_deformation_m: (0.01, Fixture",
                "shear_deformation_m: (0.01, Taste",
                "shear_deformation_m",
            ),
        ] {
            let error = GroundTypes::parse(&text.replace(from, to))
                .expect_err(field)
                .to_string();
            assert!(error.contains(field) && error.contains("gravel"), "{error}");
        }
        for field in ["cohesion_pa", "friction_angle_deg", "shear_deformation_m"] {
            let bad = text.replacen(field, "misspelt", 1);
            assert!(GroundTypes::parse(&bad).is_err());
        }
    }

    /// A VALUE OUT OF DOMAIN IS REFUSED AT LOAD, NAMING THE GROUND AND THE FIELD: a negative
    /// friction under the pair rule's `√(μa·μb)` is a NaN contact friction and an infinite one an
    /// unbounded grip; a darkening outside 0..=1 is an albedo gain no material holds; a negative
    /// softness or cap prints upward, and a roughness gain of 0 or less is no surface. Every
    /// terrain collider and the prints read the table through [`GroundTypes::parse`]. Mutant: a
    /// parse that skips a field's check, or checks only one of sign and finiteness, loads one of
    /// these. Every soil colour channel is checked independently: lifting dust relies on all of
    /// them. A zero friction is in domain and loads, and a misspelt field is refused rather than
    /// read as a ground that states none.
    #[test]
    fn a_value_out_of_domain_is_refused_at_load_naming_the_ground() {
        let gravel = |[
            softness,
            cap,
            darkening,
            roughness,
            friction,
            rolling,
            erodibility,
            red,
            green,
            blue,
            lifted,
        ]: [&str; 11]| {
            format!(
                "traction: Rigid(friction: ({friction}, Fixture(\"fixture\"))), print: (softness_mm_per_kpa: {softness}, depth_cap_mm: {cap}, darkening: \
                 {darkening}, roughness_gain: {roughness}), \
                 rolling_resistance: Total(({rolling}, Fixture(\"fixture\"))), \
                 dust: (erodibility: {erodibility}, colour_srgb: ({red}, {green}, {blue})), \
                 strike: (lifted: ({lifted}, Fixture(\"fixture\")), sparks: (false, \
                 Fixture(\"fixture\")))"
            )
        };
        let ok = [
            "0.5", "50.0", "0.2", "1.0", "0.5", "0.05", "0.5", "0.5", "0.4", "0.3", "0.5",
        ];
        let fields = [
            "print.softness_mm_per_kpa",
            "print.depth_cap_mm",
            "print.darkening",
            "print.roughness_gain",
            "friction",
            "rolling_resistance",
            "dust.erodibility",
            "dust.colour_srgb",
            "dust.colour_srgb",
            "dust.colour_srgb",
            "strike.lifted",
        ];
        let bad: [&[&str]; 11] = [
            &["-0.1", "inf", "NaN"],
            &["-1.0", "inf", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["0.0", "-1.0", "inf", "NaN"],
            &["-0.1", "inf", "-inf", "NaN"],
            &["-0.01", "inf", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.1", "1.1", "NaN"],
        ];
        for (index, field) in fields.iter().enumerate() {
            for value in bad[index] {
                let mut values = ok;
                values[index] = value;
                let text = table_text(&gravel(values));
                let error = GroundTypes::parse(&text).expect_err(value).to_string();
                assert!(
                    error.contains("\"gravel\"") && error.contains(&format!("`{field}`")),
                    "{field} {value}: {error}"
                );
            }
        }
        let mut zero = ok;
        zero[4] = "0.0";
        let zero =
            GroundTypes::parse(&table_text(&gravel(zero))).expect("zero friction is in domain");
        assert_eq!(zero.get("gravel").contact().friction, 0.0);
        let misspelt = table_text(&gravel(ok)).replacen("friction:", "fricton:", 1);
        assert!(GroundTypes::parse(&misspelt).is_err());
    }

    /// A strike's sources lie in the shader's slot order, each the slot's painted weight times that
    /// ground's own `strike.lifted`, never its muzzle `dust.erodibility`, and carry that ground's
    /// soil and spark fact. `vfx::impact` rolls one contact's read over them. Mutant: a share read
    /// from the muzzle number, or a slot order off the paint's, fails here. FIXTURE DATA.
    #[test]
    fn strike_sources_weigh_each_pack_by_its_own_lift() {
        let mut table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        for (slot, name) in PACK_GROUNDS.iter().enumerate() {
            let ground = table.types.get_mut(*name).expect("a pack's ground");
            ground.strike.lifted.value = 0.1 * (slot + 1) as f32;
            ground.strike.sparks.value = slot == 2;
            ground.dust.erodibility = 1.0 - ground.strike.lifted.value;
            ground.dust.colour_srgb = [0.1 * (slot + 1) as f32; 3];
        }
        let weights = Vec4::new(0.4, 0.3, 0.2, 0.1);
        for (slot, (share, soil)) in table.strike_sources(weights).into_iter().enumerate() {
            let own = 0.1 * (slot + 1) as f32;
            assert_eq!(share, weights[slot] * own, "slot {slot}");
            assert_eq!(soil.colour_srgb, [own; 3], "slot {slot}");
            assert_eq!(soil.sparks, slot == 2, "slot {slot}");
        }
    }

    /// A saved table reads back as the table: the owner's tuned values survive the round trip
    /// through the file, header comment and all, and every labelled value keeps its job and note
    /// (the tuning panel saves the whole table; an unlabelled value does not load back, so a save
    /// that dropped a label fails here).
    #[test]
    fn a_saved_table_reads_back_unchanged() {
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        let text = table.to_file_text().expect("serialises");
        assert_eq!(GroundTypes::parse(&text).expect("parses"), table);
    }
    /// The angle is a Mohr-Coulomb angle: converting it with sin instead of tan changes
    /// capacity. FIXTURE: 30 and 45 degrees, 1 kPa cohesion over 2 m2 at 10 kN load.
    #[test]
    fn soil_angle_conversion_reaches_the_capacity_ratio() {
        let base = "print: (softness_mm_per_kpa: 0.5, depth_cap_mm: 50.0, darkening: 0.2, roughness_gain: 1.0), rolling_resistance: Total((0.05, Fixture(\"fixture\"))), dust: (erodibility: 0.5, colour_srgb: (0.5, 0.4, 0.3)), strike: (lifted: (0.5, Fixture(\"fixture\")), sparks: (false, Fixture(\"fixture\")))";
        let row = format!(
            "traction: Soil(cohesion_pa: (1000.0, Fixture(\"fixture\")), friction_angle_deg: (30.0, Fixture(\"fixture\")), shear_deformation_m: (0.1, Fixture(\"fixture\"))), {base}"
        );
        let text = table_text(&row);
        let first = GroundTypes::parse(&text).unwrap().get("gravel").contact();
        let second = GroundTypes::parse(&text.replace("(30.0, Fixture", "(45.0, Fixture"))
            .unwrap()
            .get("gravel")
            .contact();
        let capacity = |g: crate::ground_field::ContactGround| {
            2.0 * g.traction.cohesion_pa + 10_000.0 * g.traction.tan_phi
        };
        let ratio = (2000.0 + 10_000.0 / 3.0_f32.sqrt()) / 12_000.0;
        assert!((capacity(first) / capacity(second) - ratio).abs() < 1e-6);
        assert!(
            GroundTypes::parse(&text.replace("traction: Soil(", "traction: Soil(friction: 0.5, "))
                .is_err(),
            "soil cannot author a dormant rigid-friction fact"
        );
    }
}
