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
//! THE LIBRARY IS THE GAME'S, THE PLACEMENT IS THE MAP'S. What a prototype id the placement
//! references MEANS to the game — a construction's box, a fir's size — is `prototypes.ron` beside the
//! manifest ([`PROTOTYPES_FILE`], [`Prototype`]), keyed by the ids the placement uses. The
//! manifest's own `prototypes` block is the author's export record (which glb, whether it shipped)
//! and is rewritten on every export; it is read only for its keys, so the library is checked
//! against every id the author declared, instanced or not.
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
//! FAILURE LAW (ADR-0011). Every build loads a map: an absent manifest is refused at load, naming
//! the path it looked at and the fix ([`read_manifest`]). Everything a present manifest declares
//! must hold: a file that cannot be read, an unparseable one, a mismatched coordinate system, an
//! extent that describes no world, or a heightmap that is missing or undecodable panics too. The
//! flat slab is a fixture world, selected only by `terrain_grid::ForceFlatWorld`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::construction::ConstructionLibrary;
use crate::terrain_grid::{RowOrder, TerrainExtent};
use crate::trees::TreeLibrary;

/// The map every build loads: `assets/maps/<MAP_ID>/`. `scripts/lod/config.py` reads this
/// declaration for the world the LOD ladder is cut for.
pub(crate) const MAP_ID: &str = "kalinovo";

/// Directory holding one folder per map, relative to the resolved asset root
/// (`crate::assets::asset_root`).
const MAPS_DIR: &str = "maps";

/// The manifest file inside a map's directory.
const LEVEL_FILE: &str = "level.json";

/// The game's prototype library inside a map's directory (module doc). REQUIRED beside a manifest:
/// a placement whose ids resolve to nothing is present-but-broken (ADR-0011).
const PROTOTYPES_FILE: &str = "prototypes.ron";

/// Subdirectory of a map holding what WE generate from the authored data (the UI map image), as
/// opposed to what the author shipped.
const DERIVED_DIR: &str = "derived";

/// `<root>/maps/<id>` — the map's directory.
pub(crate) fn map_dir(root: &Path) -> PathBuf {
    root.join(MAPS_DIR).join(MAP_ID)
}

/// The selected map's manifest.
pub(crate) fn level_path(root: &Path) -> PathBuf {
    map_dir(root).join(LEVEL_FILE)
}

/// The selected map's prototype library.
fn prototypes_path(root: &Path) -> PathBuf {
    map_dir(root).join(PROTOTYPES_FILE)
}

/// An asset-server path (`/`-separated, relative to the asset root) into the selected map's own
/// directory — what the author shipped, as the asset server names it.
pub(crate) fn map_asset(name: &str) -> String {
    format!("{MAPS_DIR}/{MAP_ID}/{name}")
}

/// An asset-server path (`/`-separated, relative to the asset root) into the selected map's
/// `derived/` folder — the view-only copies generated from the authored data.
pub(crate) fn derived_asset(name: &str) -> String {
    format!("{MAPS_DIR}/{MAP_ID}/{DERIVED_DIR}/{name}")
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
    /// Surface-weight mask file name, as declared.
    masks: String,
    /// The mask image's declared side in texels, `[width, height]`. A SAMPLE SPACING, not a
    /// registration: the mask is stretched over the same world square the heightmap is, at
    /// whatever resolution it was cut at.
    pub(crate) masks_resolution: [u32; 2],
    /// Nonempty, finite horizontal spawn points inside the declared map square.
    pub(crate) spawn_points: SpawnPoints,
    /// The author's object placement, in file order.
    pub(crate) instances: Vec<InstanceRecord>,
    /// The game's library, keyed by the ids the placement uses. Every instance's prototype and
    /// every id the author's block declares resolves here; [`parse`] checked it.
    pub(crate) prototypes: BTreeMap<String, Prototype>,
    /// Every tree asset a [`Prototype::Tree`] names, loaded and validated ([`load`]).
    pub(crate) trees: TreeLibrary,
    /// Every construction asset a [`Prototype::Construction`] names, loaded and validated ([`load`]).
    pub(crate) constructions: ConstructionLibrary,
}

/// Authored XZ spawn points certified once at the map load edge. The private slice cannot be
/// emptied or changed after validation; the server can always draw an index from it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpawnPoints(Box<[Vec2]>);

impl SpawnPoints {
    pub(crate) fn checked(points: Vec<[f32; 2]>, half_extent: f32, path: &Path) -> Self {
        assert!(
            !points.is_empty(),
            "map: {} field spawn_points is empty — author at least one XZ point",
            path.display(),
        );
        let points: Box<[Vec2]> = points.into_iter().map(Vec2::from_array).collect();
        for (index, point) in points.iter().enumerate() {
            assert!(
                point.is_finite() && point.abs().max_element() < half_extent,
                "map: {} field spawn_points[{index}] = {point:?} — use finite XZ metres inside the terrain square",
                path.display(),
            );
        }
        Self(points)
    }

    pub(crate) fn as_slice(&self) -> &[Vec2] {
        &self.0
    }
}

impl MapManifest {
    /// The tree asset ids the library names, each once.
    fn tree_assets(&self) -> impl Iterator<Item = &str> {
        self.prototypes
            .values()
            .filter_map(|prototype| match prototype {
                Prototype::Tree { asset } => Some(asset.as_str()),
                Prototype::Block { .. } | Prototype::Construction { .. } => None,
            })
    }

    /// The construction asset ids the library names, each once.
    fn construction_assets(&self) -> impl Iterator<Item = &str> {
        self.prototypes
            .values()
            .filter_map(|prototype| match prototype {
                Prototype::Construction { asset } => Some(asset.as_str()),
                Prototype::Block { .. } | Prototype::Tree { .. } => None,
            })
    }

    /// The game's environment destruction model admits every authored placement's scaled
    /// structure at the map edge, with the asset path, instance, field and repair on refusal.
    fn load_constructions(&mut self, root: &Path, path: &Path) {
        let mut constructions = ConstructionLibrary::load(root, self.construction_assets());
        for instance in &self.instances {
            if let Prototype::Construction { asset } = &self.prototypes[&instance.prototype] {
                constructions
                    .admit_placement(asset, instance.scale)
                    .unwrap_or_else(|error| {
                        panic!(
                            "map: {} instance {:?} field scale: {error}",
                            path.display(),
                            instance.id
                        )
                    });
            }
        }
        self.constructions = constructions;
    }

    /// The declared heightmap's path.
    pub(crate) fn heightmap_path(&self) -> PathBuf {
        self.dir.join(&self.heightmap)
    }

    /// The declared heightmap's name, for messages.
    pub(crate) fn heightmap(&self) -> &str {
        &self.heightmap
    }

    /// Asset-root-relative masks path: synchronously decoded for ground facts and loaded
    /// uploaded from that decode for rendering.
    pub(crate) fn masks_asset(&self) -> String {
        map_asset(&self.masks)
    }
}

/// The unit cube's face lanes in +X, -X, +Z, -Z, +Y, -Y order, and the cube edge its top's
/// tangent runs along.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BoxSurface {
    pub(crate) faces: [crate::construction_surface::SurfaceLane; 6],
    pub(crate) top_tangent: crate::construction_surface::BoxAxis,
}

/// What one prototype id resolves to (`prototypes.ron`, module doc). Metres at instance scale 1;
/// the instance's own scale multiplies the collider as it does the visual.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Prototype {
    /// A block: an axis-aligned box in the prototype's local frame, `min`/`max` corners around
    /// the instance origin, of `substance` (a hard row of `materials.ron`, whose friction its
    /// collider carries). A `min.y` below zero is a skirt that sinks into the surface.
    Block {
        min: [f32; 3],
        max: [f32; 3],
        substance: String,
        surface: BoxSurface,
    },
    /// A tree of the damage model ([`crate::trees`]): the asset `assets/trees/<asset>/`.
    Tree { asset: String },
    /// A construction of the destruction model ([`crate::construction`]): the asset
    /// `assets/constructions/<asset>/<asset>.construction.ron`.
    Construction { asset: String },
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
/// also carries per-asset shipping state, which describes the author's export rather than the
/// world we build.
#[derive(serde::Deserialize)]
struct LevelFile {
    coordinate_system: CoordinateSystem,
    terrain: TerrainBlock,
    spawn_points: Vec<[f32; 2]>,
    instances: Vec<InstanceRecord>,
    /// The author's prototype registry, read for its KEYS only (module doc). An export may omit
    /// the block.
    #[serde(default)]
    prototypes: BTreeMap<String, serde::de::IgnoredAny>,
}

#[derive(serde::Deserialize)]
struct TerrainBlock {
    heightmap: HeightmapBlock,
    masks: MasksBlock,
}

/// The surface-weight masks a map paints its ground blend with — R = recesses, G = slopes,
/// B = lowlands, independent linear weights. REQUIRED of every map, on the failure law's own
/// terms: the ground is drawn by blending them ([`crate::terrain_blend`]), so a manifest without
/// them describes a world this build cannot render.
#[derive(serde::Deserialize)]
struct MasksBlock {
    /// Mask file name, resolved against the manifest's own directory.
    asset: String,
    resolution: [u32; 2],
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

/// Read the selected map's manifest. A manifest that is absent or does not hold panics (module
/// doc).
pub(crate) fn load(root: &Path) -> MapManifest {
    let path = level_path(root);
    let text = read_manifest(&path);
    let library = read_prototypes(root);
    let mut manifest = parse(&text, &library, &path);
    manifest.trees = TreeLibrary::load(root, manifest.tree_assets());
    manifest.load_constructions(root, &path);
    manifest
}

/// The selected map's prototype library text. Only read beside a manifest that exists, so there is
/// no absence to tolerate: any failure is present-but-broken and panics naming the path.
fn read_prototypes(root: &Path) -> String {
    let path = prototypes_path(root);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("map: {} cannot be read: {err}", path.display()))
}

/// The manifest's text. Both failures panic naming the path (ADR-0011): an ABSENT manifest says
/// where the asset root resolved and how to point it at a tree that ships `maps/`, and a file that
/// exists and cannot be read — a directory in its place, wrong permissions, bytes that are not
/// UTF-8 — carries the error.
fn read_manifest(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => panic!(
            "map: no level manifest at {} — every build loads a map. Run from a checkout or \
             archive that ships `{MAPS_DIR}/` in its assets directory, or set BEVY_ASSET_ROOT to \
             the directory that holds `{MAPS_DIR}/`",
            path.display(),
        ),
        Err(err) => panic!("map: {} cannot be read: {err}", path.display()),
    }
}

/// The CONTENT digest of the world this process will build: the manifest's own bytes, its prototype
/// library's, the bytes of the heightmap it names, the species table, the wood row and every tree
/// asset the library names, then every construction asset it names — the files that decide the terrain
/// and the scatter, none of which crosses the wire.
///
/// CONTENT, not the id: two peers can select the same map name over different files (a stale
/// checkout, a half-pulled LFS object, a hand-edited `level.json`), and the id would happily agree
/// while the ground disagreed. `crate::net::protocol::protocol_id` folds this into the handshake.
///
/// Same failure law as [`load`]: a manifest that is absent or does not hold panics.
pub(crate) fn content_digest(root: &Path) -> u64 {
    let path = level_path(root);
    let text = read_manifest(&path);
    let library = read_prototypes(root);
    let mut manifest = parse(&text, &library, &path);
    let trees = TreeLibrary::load(root, manifest.tree_assets());
    manifest.load_constructions(root, &path);
    let constructions = &manifest.constructions;
    let heightmap = manifest.heightmap_path();
    let bytes = std::fs::read(&heightmap).unwrap_or_else(|err| {
        panic!(
            "map: {} names heightmap {} — {}: {err}",
            path.display(),
            manifest.heightmap(),
            heightmap.display(),
        )
    });
    let mut files = vec![text.as_bytes(), library.as_bytes(), bytes.as_slice()];
    files.extend(trees.texts());
    files.extend(constructions.texts());
    digest_of(&files)
}

/// The fold itself, pure: [`crate::fingerprint`]'s FNV-1a, byte-driven and integer-only, so every
/// platform folds the same map to the same digest. Each file's LENGTH is folded before its bytes,
/// so no two different sets of files can shift bytes across a boundary into the same digest.
fn digest_of(files: &[&[u8]]) -> u64 {
    use crate::fingerprint::{FNV_OFFSET_BASIS, fnv1a_64};
    files.iter().fold(
        fnv1a_64(FNV_OFFSET_BASIS, b"overmatch-map-content-v2"),
        |hash, bytes| {
            let hash = fnv1a_64(hash, &(bytes.len() as u64).to_le_bytes());
            fnv1a_64(hash, bytes)
        },
    )
}

/// Refuse a graybox prototype that describes no collider, naming the library file, the id, the
/// field and the fix: a box whose corners are not finite and ordered on every axis, or whose
/// substance states no friction (its collider carries the substance's, `scatter`). A tree or a structure is its own
/// asset's to check (`TreeLibrary::load`, `ConstructionLibrary::load`).
fn check_prototype(id: &str, prototype: &Prototype, library: &Path) {
    let substance = match prototype {
        Prototype::Block {
            min,
            max,
            substance,
            surface: _,
        } => {
            for axis in 0..3 {
                assert!(
                    min[axis].is_finite() && max[axis].is_finite() && min[axis] < max[axis],
                    "map: {} prototype {id:?}: `min` {min:?} / `max` {max:?} is no box on axis \
                     {axis} — state finite corners with min < max on every axis",
                    library.display(),
                );
            }
            substance
        }
        Prototype::Tree { .. } | Prototype::Construction { .. } => return,
    };
    if let Err(err) =
        crate::substances::SubstanceRegistry::shipped_static().contact_friction(substance)
    {
        panic!(
            "map: {} prototype {id:?}: `substance`: {err} — name a hard row of materials.ron \
             that states its friction",
            library.display(),
        );
    }
}

/// Parse and check one manifest's text against its prototype library's text. `path` is where the
/// manifest came from — both the directory every asset name resolves against and the name every
/// refusal carries.
pub(crate) fn parse(text: &str, library: &str, path: &Path) -> MapManifest {
    let level: LevelFile = serde_json::from_str(text)
        .unwrap_or_else(|err| panic!("map: {} failed to parse: {err}", path.display()));
    level.coordinate_system.check(path);
    let prototypes: BTreeMap<String, Prototype> = ron::from_str(library).unwrap_or_else(|err| {
        panic!(
            "map: {} failed to parse: {err}",
            path.with_file_name(PROTOTYPES_FILE).display()
        )
    });
    let library_path = path.with_file_name(PROTOTYPES_FILE);
    for (id, prototype) in &prototypes {
        check_prototype(id, prototype, &library_path);
    }
    // Every id the placement uses AND every id the author declared must resolve (ADR-0011): a
    // prototype with no instances today is one the next export may place.
    for id in level
        .instances
        .iter()
        .map(|instance| instance.prototype.as_str())
        .chain(level.prototypes.keys().map(String::as_str))
    {
        assert!(
            prototypes.contains_key(id),
            "map: {} references prototype {id:?}, which {PROTOTYPES_FILE} does not carry",
            path.display(),
        );
    }
    for instance in &level.instances {
        for (axis, value) in instance.scale.iter().enumerate() {
            assert!(
                value.is_finite() && *value > 0.0,
                "map: {} instance {:?} field scale[{axis}] = {value}; author a finite positive scale on every axis",
                path.display(),
                instance.id
            );
        }
    }
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
    let masks = level.terrain.masks;
    assert!(
        masks.resolution[0] > 0 && masks.resolution[1] > 0,
        "map: {} declares masks resolution {:?}, which is no image",
        path.display(),
        masks.resolution,
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
        masks: masks.asset,
        masks_resolution: masks.resolution,
        spawn_points: SpawnPoints::checked(level.spawn_points, side_m * 0.5, path),
        instances: level.instances,
        prototypes,
        trees: TreeLibrary::default(),
        constructions: ConstructionLibrary::default(),
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
        load(&shipped_assets())
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
               "height_decode":{{"offset_m":{offset_m},"scale_m":{scale_m}}}}},
               "masks":{{"asset":"m.png","resolution":[4,4]}}}},
               "spawn_points":[[0,0]],"instances":[]}}"#,
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
            EMPTY_LIBRARY,
            Path::new("fixture/level.json"),
        )
    }

    /// A library with nothing in it — enough for a placement with no instances.
    const EMPTY_LIBRARY: &str = "{}";

    pub(crate) fn fixture_box_surface() -> BoxSurface {
        use crate::construction_surface::SurfaceLane::{Primary, Secondary};
        BoxSurface {
            faces: [Primary, Primary, Primary, Primary, Secondary, Primary],
            top_tangent: crate::construction_surface::BoxAxis::Z,
        }
    }

    /// A single-construction library and a placement that uses it, for the resolution law.
    const HOUSE_LIBRARY: &str = r#"{"house": Block(min: (-1.0, -0.5, -1.0), max: (1.0, 2.0, 1.0), substance: "Brick", surface: (faces: (Primary, Primary, Primary, Primary, Secondary, Primary), top_tangent: Z))}"#;
    const HOUSE_INSTANCE: &str = r#"{"prototype":"house","translation":[0,0,0],
        "rotation":[0,0,0,1],"scale":[1,1,1],"id":"house_0001"}"#;

    /// One instance of every prototype kind — a graybox house, a tree of the damage
    /// model and a construction of the destruction model, the latter read from shipped assets —
    /// on the fixture extent: what a layer that spawns a map asserts against, whatever the shipped
    /// map happens to place.
    pub(crate) fn mixed_fixture_manifest() -> MapManifest {
        const LIBRARY: &str = r#"{
            "house": Block(min: (-1.0, -0.5, -1.0), max: (1.0, 2.0, 1.0), substance: "Brick", surface: (faces: (Primary, Primary, Primary, Primary, Secondary, Primary), top_tangent: Z)),
            "fir": Tree(asset: "fir_1"),
            "brick": Construction(asset: "house_1"),
        }"#;
        const INSTANCES: &str = r#"{"prototype":"house","translation":[0,0,0],
            "rotation":[0,0,0,1],"scale":[1,1,1],"id":"house_0001"},
            {"prototype":"fir","translation":[0,0,20],
            "rotation":[0,0,0,1],"scale":[1,1,1],"id":"fir_0001"},
            {"prototype":"brick","translation":[0,0,-20],
            "rotation":[0,0,0,1],"scale":[1,1,1],"id":"brick_0001"}"#;
        let text = manifest_text(500.0, 0.0, 100.0, "+Z").replace(
            r#""instances":[]"#,
            &format!(r#""instances":[{INSTANCES}]"#),
        );
        let mut manifest = parse(&text, LIBRARY, Path::new("fixture/level.json"));
        manifest.trees = crate::trees::tests::fixture_library("fir_1");
        manifest.constructions = ConstructionLibrary::load(&shipped_assets(), ["house_1"]);
        manifest
    }

    /// Fixture: common construction placements. Independent ledger/reset fixtures use
    /// `mixed_fixture_manifest` to exercise their separate topology streams.
    pub(crate) fn construction_fixture_manifest() -> MapManifest {
        let mut manifest = mixed_fixture_manifest();
        for prototype in manifest.prototypes.values_mut() {
            if let Prototype::Tree { asset } = prototype {
                *prototype = Prototype::Construction {
                    asset: asset.clone(),
                };
            }
        }
        manifest.trees = TreeLibrary::default();
        manifest.load_constructions(&shipped_assets(), Path::new("fixture/level.json"));
        manifest
    }

    /// The collider-spawning layer relies on the file gate refusing collapsed or inverted
    /// placements. Every refusal must identify the manifest, instance, field and repair.
    #[test]
    fn instance_scale_is_refused_at_the_manifest_with_its_repair() {
        for axis in 0..3 {
            for bad in [0.0, -1.0] {
                let mut record: serde_json::Value = serde_json::from_str(HOUSE_INSTANCE).unwrap();
                record["scale"][axis] = serde_json::json!(bad);
                let text = manifest_text(500.0, 0.0, 100.0, "+Z")
                    .replace(r#""instances":[]"#, &format!(r#""instances":[{record}]"#));
                let refused = std::panic::catch_unwind(|| {
                    parse(&text, HOUSE_LIBRARY, Path::new("fixture/level.json"));
                })
                .expect_err("bad scale must fail before a manifest reaches scatter");
                let reason = refused.downcast_ref::<String>().unwrap();
                for required in ["fixture/level.json", "house_0001", "scale[", "author"] {
                    assert!(reason.contains(required), "{reason}");
                }
            }
        }
    }

    /// THE MANIFEST IS THE MAP (`terrain_grid::decode_height_grid`): the shipped tree must carry
    /// one at the default id, it must parse and pass every check through the real loader, and the
    /// heightmap it names must exist as more than an LFS pointer. A map half-shipped — a PNG with
    /// no manifest, or a manifest naming a file nobody added — fails here in CI rather than at
    /// first boot.
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

    /// The server must never enter play with no automatic spawn candidates. Mutant: accepting an
    /// empty list leaves the first alone player with no point to draw from.
    #[test]
    #[should_panic(expected = "fixture/level.json field spawn_points is empty")]
    fn a_map_with_no_spawn_points_is_refused_at_load() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z")
            .replace("\"spawn_points\":[[0,0]]", "\"spawn_points\":[]");
        parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
    }

    /// Missing or out-of-world placement is refused at the edge, before the server samples it.
    #[test]
    fn spawn_points_must_be_present_finite_and_inside_the_map() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z");
        for replacement in [
            "",
            "\"spawn_points\":[[750,0]],",
            "\"spawn_points\":[[1e100,0]],",
        ] {
            let invalid = text.replace("\"spawn_points\":[[0,0]],", replacement);
            let refusal = std::panic::catch_unwind(|| {
                parse(&invalid, EMPTY_LIBRARY, Path::new("fixture/level.json"));
            })
            .expect_err("invalid spawn data must be refused");
            let message = refusal
                .downcast_ref::<String>()
                .expect("formatted load refusal");
            assert!(message.contains("fixture/level.json") && message.contains("spawn_points"));
        }
    }

    /// The surface-weight masks, pinned the way the heightmap is: DECODED from the shipped bytes,
    /// so a Git LFS pointer file shipping in their place fails here in CI. `terrain.masks` in
    /// `level.json` is the declaration, and this is what proves the bytes behind it are the image
    /// that block describes — 8-bit RGB at the DECLARED resolution, R = recesses, G = slopes,
    /// B = lowlands. The resolution is read out of the manifest rather than written here, so
    /// re-cutting the masks at another sample spacing is one edit, in the file that declares it.
    #[test]
    fn shipped_terrain_masks_decode_as_declared() {
        let manifest = shipped_manifest();
        let path = shipped_assets().join(manifest.masks_asset());
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
        let [width, height] = manifest.masks_resolution;
        assert_eq!(
            masks.dimensions(),
            (width, height),
            "the masks must be the image level.json declares",
        );
    }

    /// The exporter declares the conventions, the reader verifies them: a manifest written in
    /// another frame is present-but-broken, never quietly loaded.
    #[test]
    #[should_panic(expected = "declares coordinate_system.up_axis = \"+Z\"")]
    fn a_foreign_coordinate_system_panics() {
        let text =
            manifest_text(750.0, 0.0, 50.0, "+Z").replace(r#""up_axis":"+Y""#, r#""up_axis":"+Z""#);
        parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
    }

    /// The rotation order is part of the frame — an XYZW reader given WXYZ places every object at a
    /// different yaw.
    #[test]
    #[should_panic(expected = "declares coordinate_system.rotation")]
    fn a_foreign_quaternion_order_panics() {
        let text =
            manifest_text(750.0, 0.0, 50.0, "+Z").replace("quaternion_xyzw", "quaternion_wxyz");
        parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
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
        parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
    }

    /// A non-square world has no single side length to hang the samples at.
    #[test]
    #[should_panic(expected = "the world is a square")]
    fn a_non_square_extent_panics() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z").replace("[750,750]", "[750,500]");
        parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
    }

    /// THE LIBRARY COVERS THE PLACEMENT (module doc): an instance whose prototype the library does
    /// not carry is present-but-broken, refused at parse rather than at spawn. Dropping the check
    /// would let a placement load and panic later, in `scatter`, with the terrain already built.
    #[test]
    #[should_panic(
        expected = "references prototype \"house\", which prototypes.ron does not carry"
    )]
    fn an_instance_outside_the_library_panics() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z").replace(
            r#""instances":[]"#,
            &format!(r#""instances":[{HOUSE_INSTANCE}]"#),
        );
        parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
    }

    /// The author's own registry is checked too, instanced or not: an id declared there is one the
    /// next export may place, and the library must already answer for it.
    #[test]
    #[should_panic(expected = "references prototype \"barn\"")]
    fn a_declared_but_uninstanced_prototype_outside_the_library_panics() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z").replace(
            r#""instances":[]"#,
            r#""prototypes":{"barn":{"asset":"barn.glb","included":false}},"instances":[]"#,
        );
        parse(&text, HOUSE_LIBRARY, Path::new("fixture/level.json"));
    }

    /// A graybox prototype that describes no collider is refused when the library is read, naming
    /// the library file, the id and the field: `scatter` builds the box and its
    /// friction from these with no check of its own. Mutant: dropping any arm of `check_prototype`
    /// loads its case. FIXTURE DATA: the dimensions are round and stand for no construction.
    #[test]
    fn a_graybox_prototype_that_describes_no_collider_is_refused_at_parse() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z");
        for (library, field) in [
            (
                r#"{"p": Block(min: (1.0, 0.0, 0.0), max: (1.0, 1.0, 1.0), substance: "Brick", surface: (faces: (Primary, Primary, Primary, Primary, Secondary, Primary), top_tangent: Z))}"#,
                "`min`",
            ),
            (
                r#"{"p": Block(min: (0.0, 0.0, 0.0), max: (1.0, NaN, 1.0), substance: "Brick", surface: (faces: (Primary, Primary, Primary, Primary, Secondary, Primary), top_tangent: Z))}"#,
                "`min`",
            ),
            (
                r#"{"p": Block(min: (0.0, 0.0, 0.0), max: (1.0, 1.0, 1.0), substance: "Nope", surface: (faces: (Primary, Primary, Primary, Primary, Secondary, Primary), top_tangent: Z))}"#,
                "`substance`",
            ),
        ] {
            let refusal =
                std::panic::catch_unwind(|| parse(&text, library, Path::new("fixture/level.json")))
                    .expect_err(library);
            let message = refusal
                .downcast_ref::<String>()
                .expect("a formatted refusal");
            assert!(
                message.contains("prototypes.ron")
                    && message.contains("\"p\"")
                    && message.contains(field),
                "{library}: {message}"
            );
        }
    }

    /// A resolved instance reaches the manifest with its library entry beside it — the one
    /// coupling `scatter` builds a body from.
    #[test]
    fn an_instance_resolves_to_its_library_entry() {
        let text = manifest_text(750.0, 0.0, 50.0, "+Z").replace(
            r#""instances":[]"#,
            &format!(r#""instances":[{HOUSE_INSTANCE}]"#),
        );
        let manifest = parse(&text, HOUSE_LIBRARY, Path::new("fixture/level.json"));
        assert_eq!(
            manifest.prototypes[&manifest.instances[0].prototype],
            Prototype::Block {
                min: [-1.0, -0.5, -1.0],
                max: [1.0, 2.0, 1.0],
                substance: "Brick".to_owned(),
                surface: fixture_box_surface(),
            },
        );
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

    /// A tree with no `maps/` in it is refused at load (ADR-0011), naming the path and the fix: a
    /// build that silently built the flat slab would run offline on a world no map declares, and
    /// a server would hand its peers a digest no shipped map produces.
    #[test]
    #[should_panic(expected = "no level manifest at /nonexistent-asset-root/maps/")]
    fn a_missing_manifest_is_refused() {
        load(Path::new("/nonexistent-asset-root"));
    }

    /// The handshake digest takes the same refusal: no tree without a map can name a world.
    #[test]
    #[should_panic(expected = "set BEVY_ASSET_ROOT")]
    fn a_missing_manifest_has_no_digest() {
        content_digest(Path::new("/nonexistent-asset-root"));
    }

    /// PRESENT-BUT-BROKEN (ADR-0011): a manifest path that exists and cannot be read carries its
    /// own error, not the absence message. Read here as a directory standing where the file should
    /// be — the same `read_to_string` failure a permission bit or a non-UTF-8 file produces.
    #[test]
    #[should_panic(expected = "cannot be read")]
    fn an_unreadable_manifest_panics() {
        read_manifest(&map_dir(&shipped_assets()));
    }

    /// THE COMPATIBILITY TERM. The digest is a function of the CONTENT of every file, each length-
    /// delimited, so neither a one-byte edit to any of them nor a re-cut of a boundary between two
    /// can leave it standing. This is what `net::protocol::protocol_id` folds into the handshake:
    /// two peers whose worlds differ must not be able to agree on a tag.
    #[test]
    fn the_digest_moves_with_any_file() {
        let base = digest_of(&[b"level", b"library", b"height"]);
        assert_ne!(
            base,
            digest_of(&[b"levex", b"library", b"height"]),
            "the manifest counts",
        );
        assert_ne!(
            base,
            digest_of(&[b"level", b"librarz", b"height"]),
            "the prototype library counts",
        );
        assert_ne!(
            base,
            digest_of(&[b"level", b"library", b"heighu"]),
            "the heightmap counts",
        );
        assert_ne!(
            digest_of(&[b"ab", b"cd"]),
            digest_of(&[b"abc", b"d"]),
            "the boundary between two files is part of the digest",
        );
    }

    /// The game's environment destruction model admits placement scales while loading the map,
    /// before spawning any body. Removing the placement gate admits this nonuniform fixture.
    #[test]
    fn construction_placement_admission_names_the_manifest_instance_asset_and_scale() {
        let root = shipped_assets();
        let path = level_path(&root);
        let mut manifest = load(&root);
        let index = manifest
            .instances
            .iter()
            .position(|instance| {
                matches!(
                    manifest.prototypes[&instance.prototype],
                    Prototype::Construction { .. }
                )
            })
            .expect("the fixture map places a structure");
        let instance = &mut manifest.instances[index];
        instance.scale[0] *= 2.0;
        let id = instance.id.clone();
        let Prototype::Construction { asset } = &manifest.prototypes[&instance.prototype] else {
            unreachable!()
        };
        let asset = asset.clone();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            manifest.load_constructions(&root, &path)
        }))
        .unwrap_err();
        let message = panic
            .downcast_ref::<String>()
            .expect("the load refusal carries its context");
        for required in [
            path.to_str().unwrap(),
            &id,
            &format!("{asset}.construction.ron"),
            "scale",
            "uniform",
        ] {
            assert!(message.contains(required), "{message}");
        }
    }

    /// Peers derive construction geometry and dressing from the same admitted bytes. Omitting
    /// either file from the compatibility term would admit a different collision or foliage view.
    #[test]
    fn the_digest_folds_every_admitted_construction_file() {
        let root = shipped_assets();
        let path = level_path(&root);
        let text = read_manifest(&path);
        let library = read_prototypes(&root);
        let manifest = load(&root);
        assert!(
            manifest.tree_assets().next().is_none(),
            "live scatter uses constructions"
        );
        let heightmap = std::fs::read(manifest.heightmap_path()).unwrap();
        let mut files = vec![text.as_bytes(), library.as_bytes(), heightmap.as_slice()];
        let assets: std::collections::BTreeSet<_> = manifest.construction_assets().collect();
        let construction_texts: Vec<_> = assets
            .iter()
            .flat_map(|asset| {
                ["construction", "dressing"].map(|suffix| {
                    std::fs::read(
                        root.join("constructions")
                            .join(asset)
                            .join(format!("{asset}.{suffix}.ron")),
                    )
                    .unwrap()
                })
            })
            .collect();
        let construction_files: Vec<_> = construction_texts.iter().map(Vec::as_slice).collect();
        let shipped = content_digest(&root);
        for omitted in 0..construction_files.len() {
            let mut incomplete = files.clone();
            incomplete.extend(
                construction_files
                    .iter()
                    .enumerate()
                    .filter_map(|(index, bytes)| (index != omitted).then_some(*bytes)),
            );
            assert_ne!(
                shipped,
                digest_of(&incomplete),
                "construction file {omitted} counts"
            );
        }
        files.extend(construction_files);
        assert_eq!(shipped, digest_of(&files));
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
        parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
    }

    /// Rows run along Z or the manifest is not describing this world at all.
    #[test]
    #[should_panic(expected = "image_axes.row_increases_toward = \"+X\"")]
    fn an_off_axis_row_direction_panics() {
        parse(
            &manifest_text(750.0, 0.0, 50.0, "+X"),
            EMPTY_LIBRARY,
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
        parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
    }
}
