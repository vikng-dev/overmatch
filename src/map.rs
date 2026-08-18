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
//! THE MAP IS A COMPATIBILITY TERM. Nothing here replicates — both peers derive their terrain and
//! their scatter from these bytes — so which map a process loaded belongs in the connect-time
//! handshake beside the wire manifest. [`content_digest`] is what it enters as.
//!
//! THE READER VERIFIES THE CONVENTIONS. The exporter declares handedness, up axis, units, transform
//! space and quaternion order; [`CoordinateSystem::check`] refuses anything but the game's own —
//! a manifest whose poses mean something else is present-but-broken, not absent. It also declares
//! which way its heightmap's pixels run ([`HeightmapBlock::row_order`]), and THAT one is honoured
//! rather than refused: the reader reverses a `-Z` image's rows once at decode, so the world stands
//! the way the author placed their objects on it.
//!
//! FAILURE LAW (ADR-0011). Absent → `None` → the flat-slab fallback world, the ONLY tolerated
//! absence, and absent means NOT FOUND and nothing else ([`read_manifest`]). Present → everything
//! it declares must hold: a file that cannot be read, an unparseable one, a mismatched coordinate
//! system, an extent that describes no world, or a heightmap that is missing or undecodable panics,
//! because a peer silently falling back to flat while others load the map would desync the
//! deterministic sim.

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::terrain_grid::{RowOrder, TerrainExtent};

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
    /// Which world direction the heightmap's rows increase toward, as declared — the decode's one
    /// orientation input ([`crate::terrain_grid::grid_from_png`]).
    pub(crate) rows: RowOrder,
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
    /// Which world directions the image's columns and rows run in.
    image_axes: ImageAxes,
    /// The corner pixels' world centres — a second, redundant statement of the same axes, which is
    /// what makes it a cross-check. Optional: an exporter that omits it simply offers no second
    /// opinion.
    sample_centers_xz: Option<SampleCenters>,
    height_decode: HeightDecode,
}

/// The image's own axes, as the exporter declares them.
#[derive(serde::Deserialize)]
struct ImageAxes {
    column_increases_toward: String,
    row_increases_toward: String,
}

/// The world XZ the corner pixels' centres sit at, as declared.
#[derive(serde::Deserialize)]
struct SampleCenters {
    pixel_0_0: [f32; 2],
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

impl HeightmapBlock {
    /// The declared row direction, as the decode's own type.
    ///
    /// Columns are not a choice: the decode walks a row-major image with the column index as X, so
    /// anything but `+X` describes a different image than the one that would be read. Rows ARE —
    /// an exporter whose image origin sits at the `+Z` corner declares `-Z`, and
    /// [`crate::terrain_grid::grid_from_png`] reverses the row order once at decode.
    ///
    /// [`SampleCenters`], when present, must agree: a manifest that contradicts itself is
    /// present-but-broken (ADR-0011), because nothing here can know which of the two declarations
    /// is the lie.
    fn row_order(&self, path: &Path) -> RowOrder {
        let ImageAxes {
            column_increases_toward: columns,
            row_increases_toward: declared_rows,
        } = &self.image_axes;
        assert!(
            columns == "+X",
            "map: {} declares image_axes.column_increases_toward = {columns:?}, the game reads \
             \"+X\"",
            path.display(),
        );
        let rows = match declared_rows.as_str() {
            "+Z" => RowOrder::TowardPositiveZ,
            "-Z" => RowOrder::TowardNegativeZ,
            other => panic!(
                "map: {} declares image_axes.row_increases_toward = {other:?}, the game reads \
                 \"+Z\" or \"-Z\"",
                path.display(),
            ),
        };
        if let Some(centers) = &self.sample_centers_xz {
            let [x, z] = centers.pixel_0_0;
            // Pixel (0, 0) is the first column of the first row: on the `-X` side because columns
            // run toward `+X`, and on the far side of whichever way the rows run.
            let z_agrees = (z > 0.0) == (rows == RowOrder::TowardNegativeZ);
            assert!(
                x < 0.0 && z_agrees,
                "map: {} declares sample_centers_xz.pixel_0_0 = [{x}, {z}], which is not the \
                 corner image_axes ({columns} columns, {declared_rows} rows) puts it in",
                path.display(),
            );
        }
        rows
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
    let text = read_manifest(&path)?;
    Some(parse(&text, &path))
}

/// The manifest's text, or `None` when there is NO manifest.
///
/// ABSENT IS NOT THE SAME AS UNREADABLE. Only [`std::io::ErrorKind::NotFound`] is absence; a file
/// that exists and cannot be read — a directory in its place, wrong permissions, bytes that are not
/// UTF-8 — is present-but-broken and panics naming the path and the error (ADR-0011). Folding those
/// into the fallback would drop this peer onto the flat slab while its opponents load the map, which
/// is a desync dressed up as a log line.
fn read_manifest(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            info!("map: no level manifest at {} — flat world", path.display());
            None
        }
        Err(err) => panic!("map: {} cannot be read: {err}", path.display()),
    }
}

/// FNV-1a offset basis — the seed every fold below starts from.
const DIGEST_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// The digest a peer with NO map carries. The flat-slab fallback is a WORLD like any other, so it
/// gets its own labelled term rather than a zero some map's content could land on.
const NO_MAP_DIGEST: u64 = fnv1a_64(DIGEST_SEED, b"overmatch-map-absent-v1");

/// FNV-1a over bytes. Byte-driven and integer-only — no floats, no hasher whose seed or word size
/// could differ — so every platform folds the same map to the same digest.
const fn fnv1a_64(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

/// The CONTENT digest of the world this process will build: the manifest's own bytes plus the bytes
/// of the heightmap it names — the two files that decide the terrain and the scatter, neither of
/// which crosses the wire.
///
/// CONTENT, not the id: two peers can select the same map name over different files (a stale
/// checkout, a half-pulled LFS object, a hand-edited `level.json`), and the id would happily agree
/// while the ground disagreed. `crate::net::protocol::protocol_id` folds this into the handshake.
///
/// Same failure law as [`load`]: no manifest is a legal world and answers [`NO_MAP_DIGEST`];
/// a manifest that is present and does not hold panics.
pub(crate) fn content_digest(root: &Path) -> u64 {
    let path = level_path(root);
    let Some(text) = read_manifest(&path) else {
        return NO_MAP_DIGEST;
    };
    let manifest = parse(&text, &path);
    let heightmap = manifest.heightmap_path();
    let bytes = std::fs::read(&heightmap).unwrap_or_else(|err| {
        panic!(
            "map: {} names heightmap {} — {}: {err}",
            path.display(),
            manifest.heightmap(),
            heightmap.display(),
        )
    });
    digest_of(text.as_bytes(), &bytes)
}

/// The fold itself, pure. Each file's LENGTH is folded before its bytes, so no two different pairs
/// of files can shift bytes across the boundary into the same digest.
fn digest_of(level: &[u8], heightmap: &[u8]) -> u64 {
    let hash = fnv1a_64(DIGEST_SEED, b"overmatch-map-content-v1");
    let hash = fnv1a_64(hash, &(level.len() as u64).to_le_bytes());
    let hash = fnv1a_64(hash, level);
    let hash = fnv1a_64(hash, &(heightmap.len() as u64).to_le_bytes());
    fnv1a_64(hash, heightmap)
}

/// Parse and check one manifest's text. `path` is where it came from — both the directory every
/// asset name resolves against and the name every refusal carries.
pub(crate) fn parse(text: &str, path: &Path) -> MapManifest {
    let level: LevelFile = serde_json::from_str(text)
        .unwrap_or_else(|err| panic!("map: {} failed to parse: {err}", path.display()));
    level.coordinate_system.check(path);
    let heightmap = level.terrain.heightmap;
    let side_m = heightmap.world_extent_xz.side_m(path);
    let rows = heightmap.row_order(path);
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
        rows,
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
    /// checks, and no instances. `rows` is the declared row direction, and `pixel_0_0` the corner
    /// centre that must agree with it.
    fn manifest_text(half_m: f32, offset_m: f32, scale_m: f32, rows: &str) -> String {
        let corner_z = if rows == "-Z" { half_m } else { -half_m };
        format!(
            r#"{{"coordinate_system":{{"handedness":"right","up_axis":"+Y",
               "horizontal_axes":["+X","+Z"],"units":"meters","transform_space":"world",
               "rotation":"quaternion_xyzw"}},
               "terrain":{{"heightmap":{{"asset":"h.png",
               "world_extent_xz":{{"minimum":[{min},{min}],"maximum":[{half_m},{half_m}]}},
               "image_axes":{{"column_increases_toward":"+X","row_increases_toward":"{rows}"}},
               "sample_centers_xz":{{"pixel_0_0":[{min},{corner_z}]}},
               "height_decode":{{"offset_m":{offset_m},"scale_m":{scale_m}}}}}}},
               "instances":[]}}"#,
            min = -half_m,
        )
    }

    /// The terrain block a decode test hangs its fixture PNG at, parsed through the real reader —
    /// in the grid's own row direction, so a decode test that is not ABOUT orientation reads the
    /// image as written.
    pub(crate) fn fixture_manifest(half_m: f32, offset_m: f32, scale_m: f32) -> MapManifest {
        fixture_manifest_with_rows(half_m, offset_m, scale_m, "+Z")
    }

    /// The same fixture at a declared row direction — the orientation law's one input.
    pub(crate) fn fixture_manifest_with_rows(
        half_m: f32,
        offset_m: f32,
        scale_m: f32,
        rows: &str,
    ) -> MapManifest {
        parse(
            &manifest_text(half_m, offset_m, scale_m, rows),
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

    /// The surface-weight masks, pinned the way the heightmap is: DECODED from the shipped bytes,
    /// so a Git LFS pointer file shipping in their place fails here in CI. Nothing reads them yet —
    /// `terrain.masks` in `level.json` is the declaration, and this is what proves the bytes behind
    /// it are the image that block describes: 4096² 8-bit RGB, R = recesses, G = slopes,
    /// B = lowlands, on the heightmap's own pixel grid.
    #[test]
    fn shipped_terrain_masks_decode_as_declared() {
        let path = map_dir(&shipped_assets()).join("terrain_masks.png");
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|err| panic!("terrain masks missing at {}: {err}", path.display()));
        assert!(
            bytes.len() > 1024,
            "{} is {} bytes — a Git LFS POINTER, not the masks (checkout without lfs pull)",
            path.display(),
            bytes.len(),
        );
        let image = image::load_from_memory(&bytes).expect("the masks must decode as PNG");
        let color = image.color();
        let image::DynamicImage::ImageRgb8(masks) = image else {
            panic!("the masks must be 8-bit RGB, not {color:?}");
        };
        assert_eq!(
            masks.dimensions(),
            (4096, 4096),
            "the masks share the heightmap's pixel grid",
        );
    }

    /// The exporter declares the conventions, the reader verifies them: a manifest written in
    /// another frame is present-but-broken, never quietly loaded.
    #[test]
    #[should_panic(expected = "declares coordinate_system.up_axis = \"+Z\"")]
    fn a_foreign_coordinate_system_panics() {
        let text =
            manifest_text(750.0, 0.0, 50.0, "+Z").replace(r#""up_axis":"+Y""#, r#""up_axis":"+Z""#);
        parse(&text, Path::new("fixture/level.json"));
    }

    /// The rotation order is part of the frame — an XYZW reader given WXYZ places every object at a
    /// different yaw.
    #[test]
    #[should_panic(expected = "declares coordinate_system.rotation")]
    fn a_foreign_quaternion_order_panics() {
        let text =
            manifest_text(750.0, 0.0, 50.0, "+Z").replace("quaternion_xyzw", "quaternion_wxyz");
        parse(&text, Path::new("fixture/level.json"));
    }

    /// `TerrainExtent` is one side spanning `[-half, +half]`: an off-centre square would be hung
    /// somewhere the author did not place it.
    #[test]
    #[should_panic(expected = "centred on the origin")]
    fn an_off_centre_extent_panics() {
        // Still a 1 500 m square, shifted 250 m along +X.
        let text = manifest_text(750.0, 0.0, 50.0, "+Z").replace(
            r#""minimum":[-750,-750],"maximum":[750,750]"#,
            r#""minimum":[-500,-750],"maximum":[1000,750]"#,
        );
        parse(&text, Path::new("fixture/level.json"));
    }

    /// A non-square world has no single side length to hang the samples at.
    #[test]
    #[should_panic(expected = "the world is a square")]
    fn a_non_square_extent_panics() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z").replace("[750,750]", "[750,500]");
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

    /// The declared row direction reaches the decode as itself, both ways — the ONE orientation
    /// input `terrain_grid::grid_from_png` acts on.
    #[test]
    fn the_declared_row_direction_reaches_the_decode() {
        assert_eq!(
            fixture_manifest_with_rows(750.0, 0.0, 50.0, "+Z").rows,
            RowOrder::TowardPositiveZ,
        );
        assert_eq!(
            fixture_manifest_with_rows(750.0, 0.0, 50.0, "-Z").rows,
            RowOrder::TowardNegativeZ,
        );
    }

    /// THE SHIPPED BUNDLE'S OWN DECLARATION. It exports rows toward `-Z`; if that ever changes in
    /// the file it must change here too, because the grid is mirrored north–south between the two
    /// answers and every authored object then stands on different ground.
    #[test]
    fn the_shipped_map_declares_the_row_direction_it_was_exported_with() {
        assert_eq!(shipped_manifest().rows, RowOrder::TowardNegativeZ);
    }

    /// A tree with no `maps/` in it at all is the flat-slab fallback world — the one absence the
    /// loader tolerates.
    #[test]
    fn a_missing_manifest_is_the_only_absence() {
        assert!(read_manifest(&level_path(Path::new("/nonexistent-asset-root"))).is_none());
        assert!(load(Path::new("/nonexistent-asset-root")).is_none());
    }

    /// PRESENT-BUT-BROKEN (ADR-0011): a manifest path that exists and cannot be read is NOT
    /// absence. Read here as a directory standing where the file should be — the same
    /// `read_to_string` failure a permission bit or a non-UTF-8 file produces, and the class that
    /// used to select the flat world silently while every other peer loaded the map.
    #[test]
    #[should_panic(expected = "cannot be read")]
    fn an_unreadable_manifest_panics() {
        read_manifest(&map_dir(&shipped_assets()));
    }

    /// THE COMPATIBILITY TERM. The digest is a function of the CONTENT of both files, each length-
    /// delimited, so neither a one-byte edit to either nor a re-cut of the boundary between them
    /// can leave it standing. This is what `net::protocol::protocol_id` folds into the handshake:
    /// two peers whose worlds differ must not be able to agree on a tag.
    #[test]
    fn the_digest_moves_with_either_file() {
        let base = digest_of(b"level", b"height");
        assert_ne!(base, digest_of(b"levex", b"height"), "the manifest counts");
        assert_ne!(base, digest_of(b"level", b"heighu"), "the heightmap counts");
        assert_ne!(
            digest_of(b"ab", b"cd"),
            digest_of(b"abc", b"d"),
            "the boundary between the two files is part of the digest",
        );
    }

    /// The flat-slab fallback is a WORLD, and it gets its own term: a peer with no map and a peer
    /// with one must never hand the handshake the same tag.
    #[test]
    fn the_map_and_its_absence_digest_differently() {
        let shipped = content_digest(&shipped_assets());
        assert_eq!(
            shipped,
            content_digest(&shipped_assets()),
            "the same tree must digest the same on every read",
        );
        assert_ne!(shipped, NO_MAP_DIGEST);
        assert_eq!(
            content_digest(Path::new("/nonexistent-asset-root")),
            NO_MAP_DIGEST,
        );
    }

    /// The column index IS X — the decode walks a row-major image, so a `-X` export describes
    /// pixels nobody reads.
    #[test]
    #[should_panic(expected = "image_axes.column_increases_toward = \"-X\"")]
    fn a_foreign_column_axis_panics() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z").replace(
            r#""column_increases_toward":"+X""#,
            r#""column_increases_toward":"-X""#,
        );
        parse(&text, Path::new("fixture/level.json"));
    }

    /// Rows run along Z or the manifest is not describing this world at all.
    #[test]
    #[should_panic(expected = "image_axes.row_increases_toward = \"+X\"")]
    fn an_off_axis_row_direction_panics() {
        parse(
            &manifest_text(750.0, 0.0, 50.0, "+X"),
            Path::new("fixture/level.json"),
        );
    }

    /// The corner centres are the exporter's second opinion on the same axes. When the two
    /// disagree, one of them is a lie and nothing here can tell which — so neither is believed.
    #[test]
    #[should_panic(expected = "not the corner image_axes")]
    fn corner_centres_contradicting_the_axes_panic() {
        // `-Z` rows put pixel (0, 0) at +Z; this one claims the opposite corner.
        let text = manifest_text(750.0, 0.0, 50.0, "-Z")
            .replace(r#""pixel_0_0":[-750,750]"#, r#""pixel_0_0":[-750,-750]"#);
        parse(&text, Path::new("fixture/level.json"));
    }
}
