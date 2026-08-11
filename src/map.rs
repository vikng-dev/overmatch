//! The map manifest: `assets/maps/<id>/level.json`, the ONE file that says what a map is.
//!
//! ONE MAP IS ONE DIRECTORY. The exporter writes a single file per map carrying the terrain block
//! (which heightmap, the square it hangs at, how its pixels decode to metres), the author's object
//! placement, and the coordinate conventions the whole export was written in. Every asset it names
//! resolves against its OWN directory, so a map moves as a directory and the systems that read it
//! never learn an id.
//!
//! ONE PARSE. [`crate::terrain_grid::decode_height_grid`] reads the file at Startup and publishes
//! [`MapManifest`]; the terrain decode takes its extent and heightmap from that resource and
//! [`crate::scatter`] takes its instances from the same one. Nothing else opens the file.
//!
//! THE READER VERIFIES THE CONVENTIONS. The exporter declares handedness, up axis, units, transform
//! space and quaternion order; [`CoordinateSystem::check`] refuses anything but the game's own —
//! a manifest whose poses mean something else is present-but-broken, not absent.
//!
//! FAILURE LAW (ADR-0011). Absent → `None` → the flat-slab fallback world, the ONLY tolerated
//! absence. Present → everything it declares must hold: an unparseable file, a mismatched
//! coordinate system, an extent that describes no world, or a heightmap that is missing or
//! undecodable panics, because a peer silently falling back to flat while others load the map
//! would desync the deterministic sim.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::terrain_grid::TerrainExtent;

/// The map loaded when nothing selects another.
pub(crate) const DEFAULT_MAP_ID: &str = "kalinovo";

/// Dev override for the map id — the whole of map selection.
const MAP_ENV: &str = "OVERMATCH_MAP";

/// Directory holding one folder per map, relative to the resolved asset root
/// (`crate::assets::asset_root`).
const MAPS_DIR: &str = "maps";

/// The manifest file inside a map's directory.
const LEVEL_FILE: &str = "level.json";

/// Subdirectory of a map holding what WE generate from the authored data (the UI map image), as
/// opposed to what the author shipped.
const DERIVED_DIR: &str = "derived";

/// The map id this process loads. Read through `crate::env_value`, so the shared off vocabulary
/// (empty, `0`, `false`) reads as "no override" exactly as it does for every other switch.
pub(crate) fn map_id() -> String {
    resolve_map_id(crate::env_value(MAP_ENV))
}

/// The id rule, pure: the override when one is named, [`DEFAULT_MAP_ID`] otherwise.
fn resolve_map_id(env: Option<String>) -> String {
    env.unwrap_or_else(|| DEFAULT_MAP_ID.to_owned())
}

/// `<root>/maps/<id>` — the selected map's directory.
pub(crate) fn map_dir(root: &Path) -> PathBuf {
    root.join(MAPS_DIR).join(map_id())
}

/// The selected map's manifest.
pub(crate) fn level_path(root: &Path) -> PathBuf {
    map_dir(root).join(LEVEL_FILE)
}

/// An asset-server path (`/`-separated, relative to the asset root) into the selected map's
/// `derived/` folder — the view-only copies generated from the authored data.
pub(crate) fn derived_asset(name: &str) -> String {
    format!("{MAPS_DIR}/{}/{DERIVED_DIR}/{name}", map_id())
}

/// One map's `level.json`, parsed and checked. The extent is a claim the map's author makes about
/// their export, never something derived from the pixels.
#[derive(Resource, Debug, Clone, PartialEq)]
pub(crate) struct MapManifest {
    /// The manifest's own directory — what every asset name in it resolves against.
    dir: PathBuf,
    /// Heightmap file name, as declared.
    heightmap: String,
    /// The square the map hangs at and the metres its samples span.
    pub(crate) extent: TerrainExtent,
    /// The author's object placement, in file order.
    pub(crate) instances: Vec<InstanceRecord>,
}

impl MapManifest {
    /// The declared heightmap's path.
    pub(crate) fn heightmap_path(&self) -> PathBuf {
        self.dir.join(&self.heightmap)
    }

    /// The declared heightmap's name, for messages.
    pub(crate) fn heightmap(&self) -> &str {
        &self.heightmap
    }
}

/// One instance record as authored. `rotation` is XYZW, `translation`/`scale` are XYZ.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct InstanceRecord {
    pub(crate) prototype: String,
    pub(crate) translation: [f32; 3],
    pub(crate) rotation: [f32; 4],
    pub(crate) scale: [f32; 3],
    /// The file's own instance id — the sort key that fixes iteration order.
    pub(crate) id: String,
}

/// `level.json` as far as this crate reads it. Deliberately NOT `deny_unknown_fields`: the file
/// also carries the author's prototype registry and per-asset shipping state, which describe their
/// export rather than the world we build.
#[derive(serde::Deserialize)]
struct LevelFile {
    coordinate_system: CoordinateSystem,
    terrain: TerrainBlock,
    instances: Vec<InstanceRecord>,
}

#[derive(serde::Deserialize)]
struct TerrainBlock {
    heightmap: HeightmapBlock,
}

#[derive(serde::Deserialize)]
struct HeightmapBlock {
    /// Heightmap file name, resolved against the manifest's own directory.
    asset: String,
    world_extent_xz: ExtentXz,
    height_decode: HeightDecode,
}

/// The world square, as `[X, Z]` corner pairs in metres.
#[derive(serde::Deserialize)]
struct ExtentXz {
    minimum: [f32; 2],
    maximum: [f32; 2],
}

/// `height_m = (u16 / 65535) * scale_m + offset_m` — the metre floor a full-scale-zero sample maps
/// to, and the metres a full-scale sample adds on top.
#[derive(serde::Deserialize)]
struct HeightDecode {
    offset_m: f32,
    scale_m: f32,
}

/// The conventions the export was written in.
#[derive(serde::Deserialize)]
struct CoordinateSystem {
    handedness: String,
    up_axis: String,
    horizontal_axes: Vec<String>,
    units: String,
    transform_space: String,
    rotation: String,
}

impl CoordinateSystem {
    /// Refuse any convention but the game's: right-handed, +Y up, +X/+Z horizontal, metres,
    /// world-space transforms, XYZW quaternions. A mismatch places every instance somewhere else,
    /// so it panics rather than loading (ADR-0011).
    fn check(&self, path: &Path) {
        let horizontal = self.horizontal_axes.join(",");
        for (field, got, want) in [
            ("handedness", self.handedness.as_str(), "right"),
            ("up_axis", self.up_axis.as_str(), "+Y"),
            ("horizontal_axes", horizontal.as_str(), "+X,+Z"),
            ("units", self.units.as_str(), "meters"),
            ("transform_space", self.transform_space.as_str(), "world"),
            ("rotation", self.rotation.as_str(), "quaternion_xyzw"),
        ] {
            assert!(
                got == want,
                "map: {} declares coordinate_system.{field} = {got:?}, the game reads {want:?}",
                path.display(),
            );
        }
    }
}

impl ExtentXz {
    /// The declared square's side, in metres. Square and origin-centred are requirements, not
    /// preferences: [`TerrainExtent`] is one side length spanning `[-half, +half]`, so anything
    /// else would be silently re-hung somewhere the author did not place it.
    fn side_m(&self, path: &Path) -> f32 {
        let ([min_x, min_z], [max_x, max_z]) = (self.minimum, self.maximum);
        assert!(
            min_x.is_finite() && min_z.is_finite() && max_x.is_finite() && max_z.is_finite(),
            "map: {} declares a non-finite world_extent_xz",
            path.display(),
        );
        assert!(
            max_x - min_x == max_z - min_z,
            "map: {} declares world_extent_xz {min_x}..{max_x} by {min_z}..{max_z} — the world is \
             a square",
            path.display(),
        );
        assert!(
            min_x == -max_x && min_z == -max_z,
            "map: {} declares world_extent_xz {min_x}..{max_x} by {min_z}..{max_z} — the world is \
             centred on the origin",
            path.display(),
        );
        let side = max_x - min_x;
        assert!(
            side > 0.0,
            "map: {} declares a {side} m world",
            path.display(),
        );
        side
    }
}

/// Read the selected map's manifest. `None` is "no map here" — the ONLY tolerated absence (see the
/// module doc); a manifest that exists and does not hold panics.
pub(crate) fn load(root: &Path) -> Option<MapManifest> {
    let path = level_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(parse(&text, &path)),
        Err(err) => {
            info!(
                "map: no level manifest at {} ({err}) — flat world",
                path.display()
            );
            None
        }
    }
}

/// Parse and check one manifest's text. `path` is where it came from — both the directory every
/// asset name resolves against and the name every refusal carries.
pub(crate) fn parse(text: &str, path: &Path) -> MapManifest {
    let level: LevelFile = serde_json::from_str(text)
        .unwrap_or_else(|err| panic!("map: {} failed to parse: {err}", path.display()));
    level.coordinate_system.check(path);
    let heightmap = level.terrain.heightmap;
    let side_m = heightmap.world_extent_xz.side_m(path);
    let (offset_m, scale_m) = (
        heightmap.height_decode.offset_m,
        heightmap.height_decode.scale_m,
    );
    // A zero span is legal — that is a deliberately level map; a negative or non-finite one NaNs
    // every sample the grid hands the sim.
    assert!(
        offset_m.is_finite() && scale_m.is_finite() && scale_m >= 0.0,
        "map: {} declares height_decode offset {offset_m} / scale {scale_m}",
        path.display(),
    );
    MapManifest {
        dir: path.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
        heightmap: heightmap.asset,
        extent: TerrainExtent {
            world_size_m: side_m,
            height_offset_m: offset_m,
            height_span_m: scale_m,
        },
        instances: level.instances,
    }
}

#[cfg(test)]
// `pub(crate)` so every layer that asserts against the SHIPPED map reads it through the one loader.
pub(crate) mod tests {
    use super::*;

    /// The repo's `assets/` — what the shipped map is read out of, in tests, without an asset root.
    pub(crate) fn shipped_assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
    }

    /// The shipped map's manifest, parsed and checked through the real path.
    pub(crate) fn shipped_manifest() -> MapManifest {
        load(&shipped_assets()).expect("the shipped tree carries a level manifest")
    }

    /// A minimal manifest text: the terrain block a decode needs, the conventions the reader
    /// checks, and no instances.
    fn manifest_text(half_m: f32, offset_m: f32, scale_m: f32) -> String {
        format!(
            r#"{{"coordinate_system":{{"handedness":"right","up_axis":"+Y",
               "horizontal_axes":["+X","+Z"],"units":"meters","transform_space":"world",
               "rotation":"quaternion_xyzw"}},
               "terrain":{{"heightmap":{{"asset":"h.png",
               "world_extent_xz":{{"minimum":[{min},{min}],"maximum":[{half_m},{half_m}]}},
               "height_decode":{{"offset_m":{offset_m},"scale_m":{scale_m}}}}}}},
               "instances":[]}}"#,
            min = -half_m,
        )
    }

    /// The terrain block a decode test hangs its fixture PNG at, parsed through the real reader.
    pub(crate) fn fixture_manifest(half_m: f32, offset_m: f32, scale_m: f32) -> MapManifest {
        parse(
            &manifest_text(half_m, offset_m, scale_m),
            Path::new("fixture/level.json"),
        )
    }

    /// The default id is what a process with no override loads, and the path is the per-map layout.
    #[test]
    fn the_default_map_id_resolves_and_names_its_own_directory() {
        assert_eq!(resolve_map_id(None), DEFAULT_MAP_ID);
        assert_eq!(resolve_map_id(Some("test_range".to_owned())), "test_range");
        assert_eq!(
            level_path(Path::new("/root")),
            Path::new("/root/maps").join(map_id()).join("level.json"),
        );
        assert_eq!(
            derived_asset("map_ui.png"),
            format!("maps/{}/derived/map_ui.png", map_id()),
        );
    }

    /// THE MANIFEST IS THE MAP (`terrain_grid::decode_height_grid`): the shipped tree must carry
    /// one at the default id, it must parse and pass every check through the real loader, and the
    /// heightmap it names must exist as more than an LFS pointer. A map half-shipped — a PNG with
    /// no manifest, or a manifest naming a file nobody added — fails here in CI rather than
    /// dropping one peer onto the flat world and desyncing the sim.
    #[test]
    fn shipped_map_ships_a_valid_manifest() {
        let manifest = shipped_manifest();
        let heightmap = manifest.heightmap_path();
        assert_eq!(
            heightmap.parent(),
            Some(map_dir(&shipped_assets()).as_path()),
            "a map's assets resolve inside the map's own directory",
        );
        let bytes = std::fs::read(&heightmap).unwrap_or_else(|err| {
            panic!(
                "level.json names heightmap {} — {}: {err}",
                manifest.heightmap(),
                heightmap.display(),
            )
        });
        assert!(
            bytes.len() > 1024,
            "{} is {} bytes — a Git LFS POINTER, not the map (checkout without lfs pull)",
            heightmap.display(),
            bytes.len(),
        );
        assert!(
            manifest.extent.world_size_m > 0.0 && !manifest.instances.is_empty(),
            "the shipped map declares a world and objects on it",
        );
    }

    /// The exporter declares the conventions, the reader verifies them: a manifest written in
    /// another frame is present-but-broken, never quietly loaded.
    #[test]
    #[should_panic(expected = "declares coordinate_system.up_axis = \"+Z\"")]
    fn a_foreign_coordinate_system_panics() {
        let text =
            manifest_text(750.0, 0.0, 50.0).replace(r#""up_axis":"+Y""#, r#""up_axis":"+Z""#);
        parse(&text, Path::new("fixture/level.json"));
    }

    /// The rotation order is part of the frame — an XYZW reader given WXYZ places every object at a
    /// different yaw.
    #[test]
    #[should_panic(expected = "declares coordinate_system.rotation")]
    fn a_foreign_quaternion_order_panics() {
        let text = manifest_text(750.0, 0.0, 50.0).replace("quaternion_xyzw", "quaternion_wxyz");
        parse(&text, Path::new("fixture/level.json"));
    }

    /// `TerrainExtent` is one side spanning `[-half, +half]`: an off-centre square would be hung
    /// somewhere the author did not place it.
    #[test]
    #[should_panic(expected = "centred on the origin")]
    fn an_off_centre_extent_panics() {
        // Still a 1 500 m square, shifted 250 m along +X.
        let text = manifest_text(750.0, 0.0, 50.0).replace(
            r#""minimum":[-750,-750],"maximum":[750,750]"#,
            r#""minimum":[-500,-750],"maximum":[1000,750]"#,
        );
        parse(&text, Path::new("fixture/level.json"));
    }

    /// A non-square world has no single side length to hang the samples at.
    #[test]
    #[should_panic(expected = "the world is a square")]
    fn a_non_square_extent_panics() {
        let text = manifest_text(750.0, 0.0, 50.0).replace("[750,750]", "[750,500]");
        parse(&text, Path::new("fixture/level.json"));
    }

    /// The extent and the decode reach [`TerrainExtent`] as declared — the values every consumer's
    /// metres are computed from.
    #[test]
    fn the_terrain_block_becomes_the_extent_it_declares() {
        let manifest = fixture_manifest(750.0, -12.5, 50.0);
        assert_eq!(
            manifest.extent,
            TerrainExtent {
                world_size_m: 1500.0,
                height_offset_m: -12.5,
                height_span_m: 50.0,
            },
        );
        assert_eq!(manifest.heightmap(), "h.png");
        assert_eq!(manifest.heightmap_path(), Path::new("fixture/h.png"));
    }
}
