//! The GROUND-TYPE TABLE: each kind of ground the map is made of, from squishy mud to hard concrete,
//! as data (`assets/terrain/ground_types.ron`). Each terrain pack names the type it is
//! (`terrain_blend::BlendLayer::ground`, `terrain_blend::BASE_GROUND`).
//!
//! A type carries how it takes a track print (cosmetic terrain marks, view only,
//! [`crate::track_prints`]): [`PrintResponse`]; and the friction coefficient it brings to a
//! contact ([`GroundType::friction`], `crate::substances` module doc, "Friction"). The terrain is
//! one collider, so it carries the base pack's ground ([`GroundTypes::terrain_friction`]); the
//! track law's own ground grip does not read it. A type is a record with one block per consumer,
//! so the handling a type will drive later (rolling resistance, sinking) joins as a sibling of
//! [`GroundType::print`] without touching the print's fields. Every field is required: the table
//! states each value with its provenance (below), and nothing in code stands in for one. A missing
//! field, a field the table does not know, a value outside its domain, and a table lacking a
//! ground a terrain pack names are all refused at load, naming the ground and the field
//! ([`GroundTypes::parse`]).
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
//! Every value in the shipped file is ASSUMED. The ORDER follows the ground-stamping note's §2
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
    /// The print depth, m, under a ground pressure of `kpa`: softness × pressure, capped, never
    /// negative. Zero on a ground with no softness, whatever the pressure.
    pub(crate) fn depth_m(&self, kpa: f32) -> f32 {
        (self.softness_mm_per_kpa * kpa)
            .min(self.depth_cap_mm)
            .max(0.0)
            / 1000.0
    }

    /// The albedo multiplier where a sole lay: 0..=1, as `darkening` is.
    pub(crate) fn albedo_gain(&self) -> f32 {
        1.0 - self.darkening
    }
}

/// One ground type: one block per thing the ground drives.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroundType {
    pub(crate) print: PrintResponse,
    /// The friction coefficient this ground brings to a contact, dimensionless: one side's value
    /// ([`crate::substances::PAIR_RULE`] combines it with the other side's), finite and not
    /// negative ([`GroundTypes::parse`]).
    pub(crate) friction: f32,
}

/// The table, keyed by the name a pack's contract uses.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroundTypes {
    pub(crate) types: BTreeMap<String, GroundType>,
}

/// The comment the table is written back under: the file stays self-describing after a save.
const FILE_HEADER: &str = "\
// The ground-type table (`src/ground_types.rs`): how each kind of ground takes a track print, the
// game's cosmetic terrain marks. Each terrain pack names its type (`src/terrain_blend.rs`).
//
//   softness_mm_per_kpa  print depth per unit of the tank's ground pressure, mm per kPa
//   depth_cap_mm         the deepest print this ground takes, mm
//   darkening            the share of albedo a print takes away, 0..1
//   roughness_gain       roughness multiplier under a sole; below 1 reads wet
//   friction             the friction coefficient the ground brings to a contact, one side's
//
// Every print value is ASSUMED: ordered by the Bekker fits in the ground-stamping note's §2, dirt
// and gravel raised so a print reads at 10-30 m, then tuned by eye in play (`track_prints::tuning`).
// The module doc of `src/ground_types.rs` carries the provenance. Every friction is ASSUMED, no
// source read: gravel, the terrain collider's ground, at Avian's own default 0.5; the others
// ordered by hand, paving gripping best and mud least.
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
    Serialize(ron::Error),
}

impl std::fmt::Display for GroundTableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Ron(err) => write!(f, "{err}"),
            Self::Refused(why) => f.write_str(why),
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
        Self::parse(&text)
    }

    /// The table `text` holds, or why it is refused: a malformed file, a missing field or one the
    /// table does not know, a value outside its domain (a negative or non-finite friction is a NaN
    /// contact friction under the pair rule's square root; a darkening outside 0..=1 is an albedo
    /// gain no material holds; a negative depth or softness prints upward), or a table lacking a
    /// ground a terrain pack names ([`crate::terrain_blend::BASE_GROUND`],
    /// [`crate::terrain_blend::BLEND_LAYERS`]).
    pub(crate) fn parse(text: &str) -> Result<Self, GroundTableError> {
        let table: Self = ron::de::from_str(text).map_err(GroundTableError::Ron)?;
        for (name, ground) in &table.types {
            let print = &ground.print;
            for (field, value, in_domain, domain) in [
                (
                    "friction",
                    ground.friction,
                    ground.friction.is_finite() && ground.friction >= 0.0,
                    "a finite coefficient of 0 or more",
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
        let packs = std::iter::once(crate::terrain_blend::BASE_GROUND).chain(
            crate::terrain_blend::BLEND_LAYERS
                .iter()
                .map(|layer| layer.ground),
        );
        for ground in packs {
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

    /// The friction the terrain collider carries: the base pack's ground
    /// ([`crate::terrain_blend::BASE_GROUND`], a key [`Self::parse`] requires) under the one pair
    /// rule.
    pub(crate) fn terrain_friction(&self) -> avian3d::prelude::Friction {
        crate::substances::contact_friction(self.get(crate::terrain_blend::BASE_GROUND).friction)
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

    /// The table as the file holds it.
    pub(crate) fn to_file_text(&self) -> Result<String, GroundTableError> {
        let body = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())
            .map_err(GroundTableError::Serialize)?;
        Ok(format!("{FILE_HEADER}{body}\n"))
    }

    /// Write the table to `path`, replacing it through a sibling temporary file so a failed write
    /// never leaves half a table.
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

    /// A minimal table text: every ground a terrain pack names, each with `print` and `friction`
    /// authored, the first one (`gravel`, the base pack's) carrying `gravel` in place of its block.
    /// FIXTURE DATA: the numbers are round and stand for no ground.
    fn table_text(gravel: &str) -> String {
        let block = "print: (softness_mm_per_kpa: 0.5, depth_cap_mm: 50.0, darkening: 0.2, \
                     roughness_gain: 1.0), friction: 0.5";
        format!(
            "(types: {{\"gravel\": ({gravel}), \"dirt\": ({block}), \"sand\": ({block}), \
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
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        for (name, ground) in &table.types {
            let print = ground.print;
            if print.softness_mm_per_kpa <= 0.0 || print.depth_cap_mm <= 0.0 {
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
    /// take no print and a ground with no `friction` would hand the terrain collider Avian's
    /// default, and a table lacking a pack's ground would leave that pack nothing to read. Each is
    /// refused at parse, naming what is missing, so no consumer meets one. Mutant: a
    /// `#[serde(default)]` on either field, or a parse that skips the pack check, loads one of
    /// these. The terrain collider reads the base ground's friction under the pair rule.
    #[test]
    fn a_ground_that_states_too_little_is_refused_at_load() {
        let block = "print: (softness_mm_per_kpa: 0.5, depth_cap_mm: 50.0, darkening: 0.2, \
                     roughness_gain: 1.0)";
        for (missing, gravel) in [("print", "friction: 0.5"), ("friction", block)] {
            let error = GroundTypes::parse(&table_text(gravel))
                .expect_err(missing)
                .to_string();
            assert!(error.contains(missing), "{missing}: {error}");
        }
        let without_mud = table_text(&format!("{block}, friction: 0.5"))
            .replace(", \"mud\": (", ", \"marsh\": (");
        let error = GroundTypes::parse(&without_mud)
            .expect_err("a pack's ground is missing")
            .to_string();
        assert!(error.contains("\"mud\""), "{error}");
        let table = GroundTypes::parse(&table_text(&format!("{block}, friction: 0.5")))
            .expect("the full fixture table loads");
        assert_eq!(
            table.terrain_friction().combine_rule,
            crate::substances::PAIR_RULE
        );
    }

    /// A VALUE OUT OF DOMAIN IS REFUSED AT LOAD, NAMING THE GROUND AND THE FIELD: a negative
    /// friction under the pair rule's `√(μa·μb)` is a NaN contact friction and an infinite one an
    /// unbounded grip; a darkening outside 0..=1 is an albedo gain no material holds; a negative
    /// softness or cap prints upward, and a roughness gain of 0 or less is no surface. Every
    /// terrain collider and the prints read the table through [`GroundTypes::parse`]. Mutant: a
    /// parse that skips a field's check, or checks only one of sign and finiteness, loads one of
    /// these. A zero friction is in domain and loads, and a misspelt field is refused rather than
    /// read as a ground that states none.
    #[test]
    fn a_value_out_of_domain_is_refused_at_load_naming_the_ground() {
        let gravel =
            |softness: &str, cap: &str, darkening: &str, roughness: &str, friction: &str| {
                format!(
                    "print: (softness_mm_per_kpa: {softness}, depth_cap_mm: {cap}, darkening: \
                 {darkening}, roughness_gain: {roughness}), friction: {friction}"
                )
            };
        let ok = ["0.5", "50.0", "0.2", "1.0", "0.5"];
        let fields = [
            "print.softness_mm_per_kpa",
            "print.depth_cap_mm",
            "print.darkening",
            "print.roughness_gain",
            "friction",
        ];
        let bad: [&[&str]; 5] = [
            &["-0.1", "inf", "NaN"],
            &["-1.0", "inf", "NaN"],
            &["-0.1", "1.1", "NaN"],
            &["0.0", "-1.0", "inf", "NaN"],
            &["-0.1", "inf", "-inf", "NaN"],
        ];
        for (index, field) in fields.iter().enumerate() {
            for value in bad[index] {
                let mut values = ok;
                values[index] = value;
                let [softness, cap, darkening, roughness, friction] = values;
                let text = table_text(&gravel(softness, cap, darkening, roughness, friction));
                let error = GroundTypes::parse(&text).expect_err(value).to_string();
                assert!(
                    error.contains("\"gravel\"") && error.contains(&format!("`{field}`")),
                    "{field} {value}: {error}"
                );
            }
        }
        let [softness, cap, darkening, roughness, _] = ok;
        let zero = GroundTypes::parse(&table_text(&gravel(
            softness, cap, darkening, roughness, "0.0",
        )))
        .expect("zero friction is in domain");
        assert_eq!(zero.get("gravel").friction, 0.0);
        let misspelt = table_text(&gravel(softness, cap, darkening, roughness, "0.5")).replacen(
            "friction:",
            "fricton:",
            1,
        );
        assert!(GroundTypes::parse(&misspelt).is_err());
    }

    /// A saved table reads back as the table: the owner's tuned values survive the round trip
    /// through the file, header comment and all.
    #[test]
    fn a_saved_table_reads_back_unchanged() {
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        let text = table.to_file_text().expect("serialises");
        assert_eq!(GroundTypes::parse(&text).expect("parses"), table);
    }
}
