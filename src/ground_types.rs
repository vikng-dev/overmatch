//! The GROUND-TYPE TABLE: each kind of ground the map is made of, from squishy mud to hard concrete,
//! as data (`assets/terrain/ground_types.ron`). Each terrain pack names the type it is
//! (`terrain_blend::BlendLayer::ground`, `terrain_blend::BASE_GROUND`).
//!
//! Today a type carries only how it takes a track print (cosmetic terrain marks, view only,
//! [`crate::track_prints`]): [`PrintResponse`]. A type is a record with one block per consumer, so
//! the handling a type will drive later (grip, rolling resistance, sinking) joins as a sibling of
//! [`GroundType::print`] without touching the print's fields; every field is `#[serde(default)]`,
//! so a file written before a field existed still loads.
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

/// How one ground type takes a track print.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PrintResponse {
    /// Print depth per unit of ground pressure, mm per kPa.
    pub(crate) softness_mm_per_kpa: f32,
    /// The deepest print this ground takes, mm, whatever the pressure.
    pub(crate) depth_cap_mm: f32,
    /// The share of the ground's albedo a print takes away where a sole lay, 0..1.
    pub(crate) darkening: f32,
    /// Roughness multiplier where a sole lay: below 1 reads wet.
    pub(crate) roughness_gain: f32,
}

impl Default for PrintResponse {
    /// A ground that takes no print and shows none.
    fn default() -> Self {
        Self {
            softness_mm_per_kpa: 0.0,
            depth_cap_mm: 0.0,
            darkening: 0.0,
            roughness_gain: 1.0,
        }
    }
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

    /// The albedo multiplier where a sole lay.
    pub(crate) fn albedo_gain(&self) -> f32 {
        1.0 - self.darkening.clamp(0.0, 1.0)
    }
}

/// One ground type: one block per thing the ground drives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct GroundType {
    pub(crate) print: PrintResponse,
}

/// The table, keyed by the name a pack's contract uses.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
//
// Every value is ASSUMED: ordered by the Bekker fits in the ground-stamping note's §2, dirt and
// gravel raised so a print reads at 10-30 m, then tuned by eye in play (`track_prints::tuning`).
// The module doc of `src/ground_types.rs` carries the provenance.
";

impl GroundTypes {
    /// The table at `path`, or why it could not be read.
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
        Self::parse(&text).map_err(|err| format!("{}: {err}", path.display()))
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        ron::de::from_str(text).map_err(|err| err.to_string())
    }

    /// The shipped table's path.
    pub(crate) fn shipped_path() -> PathBuf {
        crate::assets::asset_root().join(GROUND_TYPES_PATH)
    }

    /// The shipped table. A missing or unreadable table is a broken build, not a ground with no
    /// prints, so it aborts naming the file (ADR-0011).
    pub(crate) fn load_shipped() -> Self {
        let path = Self::shipped_path();
        Self::load(&path).unwrap_or_else(|err| panic!("the ground-type table: {err}"))
    }

    /// The type named `name`; a pack naming a type the table lacks aborts, naming both.
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
    pub(crate) fn to_file_text(&self) -> Result<String, String> {
        let body = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())
            .map_err(|err| err.to_string())?;
        Ok(format!("{FILE_HEADER}{body}\n"))
    }

    /// Write the table to `path`, replacing it through a sibling temporary file so a failed write
    /// never leaves half a table.
    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        let text = self.to_file_text()?;
        let temporary = path.with_extension("ron.tmp");
        std::fs::write(&temporary, text)
            .map_err(|err| format!("{}: {err}", temporary.display()))?;
        std::fs::rename(&temporary, path).map_err(|err| format!("{}: {err}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CONCRETE PRINTS NO DEPTH: a ground with no softness takes no print at any pressure, which is
    /// what keeps a road a road under the heaviest tank. The shipped table's hardest type is the
    /// one the owner reads as concrete, and every softer type prints.
    #[test]
    fn concrete_prints_no_depth() {
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        let concrete = table.get("concrete").print;
        for kpa in [0.0, 50.0, 111.0, 255.0] {
            assert_eq!(concrete.depth_m(kpa), 0.0, "concrete at {kpa} kPa");
        }
        for (name, ground) in &table.types {
            if name != "concrete" {
                assert!(ground.print.depth_m(100.0) > 0.0, "{name} takes a print");
            }
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
            if print.softness_mm_per_kpa <= 0.0 {
                continue;
            }
            let (light, heavy) = (20.0, 40.0);
            assert!(
                print.depth_m(heavy) > print.depth_m(light),
                "{name}: {heavy} kPa prints deeper than {light} kPa"
            );
            assert!(
                (print.depth_m(1.0e6) - print.depth_cap_mm / 1000.0).abs() < 1e-6,
                "{name}: the cap holds any pressure"
            );
        }
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
