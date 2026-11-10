//! The GROUND-TYPE TABLE: each kind of ground the map is made of, from squishy mud to hard concrete,
//! as data (`assets/terrain/ground_types.ron`). Each terrain pack names the type it is
//! (`terrain_blend::BlendLayer::ground`, `terrain_blend::BASE_GROUND`).
//!
//! Required, labelled `grip`, `shear_length_m` and reference-pressure `rolling_resistance` become a
//! [`crate::ground_field::ContactGround`]. `GroundTypes::parse` refuses malformed or incomplete
//! rows and values outside their domains before publication. `terrain_solver_friction` is
//! separate: Avian solid-body contacts have no track footprint and read that explicit stand-in.
//! The laws, proxy derivations and limitations live in
//! `.agents/docs/design/track-model/architecture.md`, section 5a.
//!
//! # A print's depth
//!
//! One number per tank per ground type: the type's softness times the tank's ground pressure
//! (mass over track footprint, both read from tank data — `track::rig_geom::ground_pressure_kpa`),
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

/// One ground type: one block per thing the ground drives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroundType {
    /// Effective track grip: capacity divided by normal load.
    pub(crate) grip: Labelled<f32>,
    /// Ground slip (m) that builds the full grip: the soil's shear deformation length. Zero is
    /// a ground that stores no shear of its own.
    pub(crate) shear_length_m: Labelled<f32>,
    pub(crate) print: PrintResponse,
    /// Total motion resistance quoted at the global reference pressure.
    pub(crate) rolling_resistance: Labelled<f32>,
    pub(crate) dust: DustResponse,
    pub(crate) strike: StrikeResponse,
}

impl GroundType {
    /// What this ground brings to a track contact standing wholly on it.
    pub(crate) fn contact(&self) -> crate::ground_field::ContactGround {
        crate::ground_field::ContactGround {
            grip: self.grip.value,
            rolling_resistance: self.rolling_resistance.value,
            shear_length_m: self.shear_length_m.value,
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
//   grip                 effective track capacity / normal load, labelled
//   shear_length_m       ground slip, m, that builds the full grip, labelled
//   rolling_resistance   total motion resistance at grip.ron rolling_reference_kpa, labelled
//   dust.erodibility     the share of a gun's muzzle dust cloud this ground lifts, 0..1
//   dust.colour_srgb     the lifted soil's albedo, sRGB, each channel 0..1
//   strike.lifted        the share of a round's strike read this ground throws, 0..1, labelled
//   strike.sparks        whether a strike on this ground throws contact sparks, labelled
//
// Every print value is ASSUMED: ordered by the Bekker fits in the ground-stamping note's §2, dirt
// and gravel raised so a print reads at 10-30 m, then tuned by eye in play (`track_prints::tuning`).
// terrain_solver_friction is the explicit Avian solid-body stand-in, not track grip.
// Track laws and row derivations: .agents/docs/design/track-model/architecture.md section 5a.
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
    /// table does not know, a value outside its domain (grip and shear length must be finite and
    /// nonnegative; a darkening outside 0..=1 is an albedo
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
            for (field, number, settle) in [
                (
                    "grip",
                    &ground.grip,
                    "measure track pull at a stated pressure",
                ),
                (
                    "shear_length_m",
                    &ground.shear_length_m,
                    "measure shear against slip on this ground",
                ),
                (
                    "rolling_resistance",
                    &ground.rolling_resistance,
                    "measure coast-down at a stated pressure",
                ),
            ] {
                if !fact(number) {
                    return Err(GroundTableError::Refused(format!(
                        "{GROUND_TYPES_PATH}: ground {name:?}: {field} must state a fact or Unknown; {settle}"
                    )));
                }
            }
            let print = &ground.print;
            for (field, value, in_domain, domain) in [
                (
                    "grip",
                    ground.grip.value,
                    ground.grip.value.is_finite() && ground.grip.value >= 0.0,
                    "a finite coefficient of 0 or more",
                ),
                (
                    "shear_length_m",
                    ground.shear_length_m.value,
                    ground.shear_length_m.value.is_finite() && ground.shear_length_m.value >= 0.0,
                    "a finite length of 0 m or more",
                ),
                (
                    "rolling_resistance",
                    ground.rolling_resistance.value,
                    ground.rolling_resistance.value.is_finite()
                        && ground.rolling_resistance.value
                            >= crate::track::forces::rigid_rolling_resistance(),
                    "a finite coefficient at least grip.ron rigid_rolling_resistance; correct the reference-pressure coast-down fact",
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
                     {:?}); add a {ground:?} entry with every required ground field",
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
    /// Fixture placeholders stand for no ground; rolling reads the shared hard datum so a new
    /// measurement cannot invalidate an otherwise valid fixture.
    fn table_text(gravel: &str) -> String {
        let hard = crate::track::forces::rigid_rolling_resistance();
        let block = format!(
            "grip: (0.5, Fixture(\"fixture\")), shear_length_m: (0.01, Fixture(\"fixture\")), \
                     print: (softness_mm_per_kpa: 0.5, depth_cap_mm: 50.0, darkening: 0.2, \
                     roughness_gain: 1.0), rolling_resistance: ({hard}, \
                     Fixture(\"fixture\")), dust: (erodibility: 0.5, colour_srgb: (0.5, 0.4, 0.3)), \
                     strike: (lifted: (0.5, Fixture(\"fixture\")), sparks: (false, \
                     Fixture(\"fixture\")))"
        );
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
    /// take no print, a row with no `grip` would lack a capacity fact, a row with no
    /// `shear_length_m` would build its grip over a length nobody stated,
    /// a ground with no `rolling_resistance` (or one with no label) would roll on a
    /// coefficient nobody stated, a ground with no `dust` would lift a cloud nobody stated, a ground
    /// with no `strike` would answer a round with a muzzle number or nothing, and a
    /// table lacking a pack's ground would leave that pack nothing to read. Each is refused at
    /// parse, naming what is missing, so no consumer meets one. Mutant: a `#[serde(default)]` on a
    /// required field, or a parse that skips the pack check, loads one
    /// of these. The terrain collider reads the table's explicit solid-body solver stand-in.
    #[test]
    fn a_ground_that_states_too_little_is_refused_at_load() {
        let shear = "shear_length_m: (0.01, Fixture(\"fixture\")), ";
        let block = format!(
            "grip: (0.5, Fixture(\"fixture\")), {shear}print: (softness_mm_per_kpa: 0.5, depth_cap_mm: 50.0, darkening: 0.2, \
                     roughness_gain: 1.0)"
        );
        let dust = "dust: (erodibility: 0.5, colour_srgb: (0.5, 0.4, 0.3)), strike: (lifted: (0.5, \
                    Fixture(\"fixture\")), sparks: (false, Fixture(\"fixture\")))";
        let bare_dust = "dust: (erodibility: 0.5, colour_srgb: (0.5, 0.4, 0.3))";
        let hard = crate::track::forces::rigid_rolling_resistance();
        let rolling = format!("rolling_resistance: ({hard}, Fixture(\"fixture\"))");
        let with_strike =
            |strike: &str| format!("{block}, {rolling}, {bare_dust}, strike: ({strike})");
        for (missing, gravel) in [
            (
                "print",
                format!("grip: (0.5, Fixture(\"fixture\")), {shear}{rolling}, {dust}"),
            ),
            (
                "shear_length_m",
                format!("{block}, {rolling}, {dust}").replace(shear, ""),
            ),
            (
                "provenance",
                format!("{block}, {rolling}, {dust}").replace(shear, "shear_length_m: 0.01, "),
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
                "grip",
                format!("{block}, {rolling}, {dust}")
                    .replace("grip: (0.5, Fixture(\"fixture\")), ", ""),
            ),
            ("rolling_resistance", format!("{block}, {dust}")),
            (
                "provenance",
                format!("{block}, rolling_resistance: {hard}, {dust}"),
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

    /// Ground coefficients must be facts; accepting a Taste hides a missing measurement.
    #[test]
    fn ground_coefficients_require_fact_provenance() {
        let table = GroundTypes::load_shipped();
        let text = table.to_file_text().unwrap();
        for field in ["grip", "shear_length_m", "rolling_resistance"] {
            let mut invalid = table.clone();
            let ground = invalid.types.get_mut("gravel").unwrap();
            let number = match field {
                "grip" => &mut ground.grip,
                "shear_length_m" => &mut ground.shear_length_m,
                _ => &mut ground.rolling_resistance,
            };
            number.label = crate::provenance::Provenance::Taste("fixture invalid job".into());
            let error = GroundTypes::parse(&invalid.to_file_text().unwrap())
                .unwrap_err()
                .to_string();
            assert!(error.contains(field) && error.contains("gravel"));
        }
        GroundTypes::parse(&text).unwrap();
    }

    /// A VALUE OUT OF DOMAIN IS REFUSED AT LOAD, NAMING THE GROUND AND THE FIELD: a negative
    /// grip is a negative capacity and an infinite one an
    /// unbounded grip; a negative shear length stores slip backwards and an infinite one never
    /// builds grip; a darkening outside 0..=1 is an albedo gain no material holds; a negative
    /// softness or cap prints upward, and a roughness gain of 0 or less is no surface. Every
    /// terrain collider and the prints read the table through [`GroundTypes::parse`]. Mutant: a
    /// parse that skips a field's check, or checks only one of sign and finiteness, loads one of
    /// these. Every soil colour channel is checked independently: lifting dust relies on all of
    /// them. A zero grip and a zero shear length are in domain and load, and a misspelt field is
    /// refused rather than read as a ground that states none.
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
            shear,
        ]: [&str; 12]| {
            format!(
                "grip: ({friction}, Fixture(\"fixture\")), shear_length_m: ({shear}, Fixture(\"fixture\")), \
                 print: (softness_mm_per_kpa: {softness}, depth_cap_mm: {cap}, darkening: \
                 {darkening}, roughness_gain: {roughness}), \
                 rolling_resistance: ({rolling}, Fixture(\"fixture\")), \
                 dust: (erodibility: {erodibility}, colour_srgb: ({red}, {green}, {blue})), \
                 strike: (lifted: ({lifted}, Fixture(\"fixture\")), sparks: (false, \
                 Fixture(\"fixture\")))"
            )
        };
        let hard = crate::track::forces::rigid_rolling_resistance();
        let below_hard = if hard > 0.0 { hard / 2.0 } else { -1.0 }.to_string();
        let hard = hard.to_string();
        let ok = [
            "0.5",
            "50.0",
            "0.2",
            "1.0",
            "0.5",
            hard.as_str(),
            "0.5",
            "0.5",
            "0.4",
            "0.3",
            "0.5",
            "0.01",
        ];
        let fields = [
            "print.softness_mm_per_kpa",
            "print.depth_cap_mm",
            "print.darkening",
            "print.roughness_gain",
            "grip",
            "rolling_resistance",
            "dust.erodibility",
            "dust.colour_srgb",
            "dust.colour_srgb",
            "dust.colour_srgb",
            "strike.lifted",
            "shear_length_m",
        ];
        let bad: [&[&str]; 12] = [
            &["-0.1", "inf", "NaN"],
            &["-1.0", "inf", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["0.0", "-1.0", "inf", "NaN"],
            &["-0.1", "inf", "-inf", "NaN"],
            &["-0.01", below_hard.as_str(), "inf", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["-0.001", "inf", "NaN"],
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
        zero[11] = "0.0";
        let zero = GroundTypes::parse(&table_text(&gravel(zero)))
            .expect("zero grip and zero shear length are in domain");
        assert_eq!(zero.get("gravel").contact().grip, 0.0);
        assert_eq!(zero.get("gravel").contact().shear_length_m, 0.0);
        let misspelt = table_text(&gravel(ok)).replacen("grip:", "gripp:", 1);
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
}
