//! Map admission: root data in `assets/maps/<id>/`, published as one checked snapshot.
//!
//! ONE MAP IS ONE DIRECTORY. The exporter writes a single file per map carrying the terrain block
//! (which heightmap, the square it hangs at, how its pixels decode to metres), the author's object
//! placement, and the coordinate conventions the whole export was written in. Every asset it names
//! resolves against its OWN directory, so a map moves as a directory and the systems that read it
//! never learn an id.
//!
//! ONE PARSE. Simulation plugins initialize [`MapManifest`] from [`load`]; a terrain-only root
//! admits it at Startup through [`crate::terrain_grid::decode_height_grid`]. The terrain decode
//! takes its extent and heightmap from that resource; [`crate::scatter`] takes its instances too. [`content_digest`] delegates to
//! [`load`], so compatibility checks cannot bypass admission.
//!
//! PROVISIONAL: the square play-area polygon stands in for the natural border (#200); its floor
//! and ceiling stand in for authored vertical bounds informed by logged escapes. Height and mask
//! decoders still reread files; the R6 remainder and replacements live in
//! `.agents/docs/design/map-surroundings.md`.
//! PROVISIONAL: admission encloses the current square spawn limit; a polygon inset replaces it
//! when the spawn picker supports irregular boundaries (#200).
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

mod play_area;

pub(crate) use play_area::PlayArea;

use crate::construction::ConstructionLibrary;
use crate::terrain_grid::{RowOrder, TerrainExtent};

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
const BOUNDARY_FILE: &str = "boundary.ron";

/// Map asset ownership (best practice): root files are authored inputs both binaries read,
/// belonging in the content digest and server pack. `view/` holds authored client-only
/// data; `derived/` holds generated client products (the UI map image). Neither client folder
/// enters the digest or server pack. See `.agents/docs/design/map-surroundings.md`.
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

/// One map's admitted root data. The terrain extent is the author's declaration, never something
/// derived from pixels; the play area is checked against that extent before publication.
#[derive(Resource, Debug, Clone, PartialEq)]
pub(crate) struct MapManifest {
    pub(crate) play_area: PlayArea,
    /// Compatibility digest of the exact bytes admitted by `load`.
    pub(crate) digest: u64,
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
    /// Admitted placements in id order, with their peer-stable ordinals.
    pub(crate) instances: Vec<InstanceRecord>,
    /// The game's library, keyed by the ids the placement uses. Every instance's prototype and
    /// every id the author's block declares resolves here; [`parse`] checked it.
    pub(crate) prototypes: BTreeMap<String, Prototype>,
    /// Every construction asset a [`Prototype::Construction`] names, loaded and validated ([`load`]).
    pub(crate) constructions: ConstructionLibrary,
}

impl FromWorld for MapManifest {
    fn from_world(_world: &mut World) -> Self {
        load(&crate::assets::asset_root())
    }
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
    pub(crate) fn boundary_path(&self) -> PathBuf {
        self.dir.join(BOUNDARY_FILE)
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

/// What one prototype id resolves to (`prototypes.ron`, module doc). Metres at the authored size;
/// placement supplies only position and rotation.
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
    /// A construction of the destruction model ([`crate::construction`]): the asset
    /// `assets/constructions/<asset>/<asset>.construction.ron`.
    Construction { asset: String },
}

impl Prototype {
    /// A graybox collider and spawn obstacle as a posed unit cube. Constructions own their geometry.
    pub(crate) fn block(&self, pose: Isometry3d) -> Option<Transform> {
        match self {
            Self::Block { min, max, .. } => {
                let (min, max) = (Vec3::from_array(*min), Vec3::from_array(*max));
                Some(Transform {
                    translation: Vec3::from(pose.translation) + pose.rotation * ((min + max) * 0.5),
                    rotation: pose.rotation,
                    scale: max - min,
                })
            }
            Self::Construction { .. } => None,
        }
    }
}

/// An admitted placement orientation and XZ; stale authored Y is checked, then discarded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlacementPose {
    xz: Vec2,
    rotation: Quat,
}

impl PlacementPose {
    fn checked(translation: [f32; 3], rotation: [f32; 4], id: &str) -> Result<Self, String> {
        let rotation = Quat::from_array(rotation);
        if !translation.iter().all(|value| value.is_finite()) {
            return Err(format!(
                "field instances[{id}].translation — author finite XYZ metres"
            ));
        }
        // Resolution: reject near-zero norms before normalization amplifies rounding error.
        const MIN_ROTATION_LENGTH_SQ: f32 = 1.0e-12;
        let length_sq = rotation.length_squared();
        if !(rotation.is_finite() && length_sq.is_finite() && length_sq > MIN_ROTATION_LENGTH_SQ) {
            return Err(format!(
                "field instances[{id}].rotation — author a finite quaternion with a normalizable length"
            ));
        }
        Ok(Self {
            xz: Vec2::new(translation[0], translation[2]),
            rotation: rotation.normalize(),
        })
    }

    pub(crate) fn at_height(self, height: f32) -> Isometry3d {
        Isometry3d::new(Vec3::new(self.xz.x, height, self.xz.y), self.rotation)
    }

    pub(crate) fn xz(self) -> Vec2 {
        self.xz
    }
}

/// One authored placement at prototype size.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InstanceRecord {
    pub(crate) ordinal: u16,
    pub(crate) prototype: String,
    pub(crate) pose: PlacementPose,
    /// The file's own instance id — the sort key that fixes iteration order.
    pub(crate) id: String,
}

#[derive(serde::Deserialize)]
#[serde(try_from = "InstanceFields")]
struct ParsedInstance {
    prototype: String,
    pose: PlacementPose,
    id: String,
}

/// Export metadata is permitted, but placement scaling is refused before it can enter the map.
#[derive(serde::Deserialize)]
struct InstanceFields {
    prototype: String,
    translation: [f32; 3],
    rotation: [f32; 4],
    id: String,
    #[serde(flatten)]
    extra: BTreeMap<String, serde::de::IgnoredAny>,
}

impl TryFrom<InstanceFields> for ParsedInstance {
    type Error = String;

    fn try_from(raw: InstanceFields) -> Result<Self, Self::Error> {
        if raw.extra.contains_key("scale") {
            return Err(format!(
                "instance {:?} field scale is forbidden; remove the scale field and author a separate size variant in the construction asset",
                raw.id
            ));
        }
        Ok(Self {
            prototype: raw.prototype,
            pose: PlacementPose::checked(raw.translation, raw.rotation, &raw.id)?,
            id: raw.id,
        })
    }
}

/// Admit the identity space once; every peer numbers the same id-sorted placements.
fn deserialize_instances<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<InstanceRecord>, D::Error> {
    use serde::{Deserialize, de::Error};
    let mut records = Vec::<ParsedInstance>::deserialize(deserializer)?;
    let maximum = usize::from(u16::MAX) + 1;
    if records.len() > maximum {
        return Err(D::Error::custom(format!(
            "field instances has {} placements — reduce to at most {maximum} so every instance has a u16 ordinal",
            records.len()
        )));
    }
    records.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(records
        .into_iter()
        .zip(0..=u16::MAX)
        .map(|(raw, ordinal)| InstanceRecord {
            prototype: raw.prototype,
            pose: raw.pose,
            id: raw.id,
            ordinal,
        })
        .collect())
}

/// `level.json` as far as this crate reads it. Deliberately NOT `deny_unknown_fields`: the file
/// also carries per-asset shipping state, which describes the author's export rather than the
/// world we build.
#[derive(serde::Deserialize)]
struct LevelFile {
    coordinate_system: CoordinateSystem,
    terrain: TerrainBlock,
    spawn_points: Vec<[f32; 2]>,
    #[serde(deserialize_with = "deserialize_instances")]
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
            side.is_finite() && side > 0.0,
            "map: {} field world_extent_xz declares a {side} m world — author a finite positive side",
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
    let boundary_path = path.with_file_name(BOUNDARY_FILE);
    let boundary = std::fs::read_to_string(&boundary_path).unwrap_or_else(|err| {
        panic!(
            "map: {} field boundary: {err} — restore the required boundary.ron",
            boundary_path.display()
        )
    });
    let parsed = parse(&text, &library, &path);
    let read = |field: &str, name: &str| {
        let file = parsed.dir.join(name);
        std::fs::read(&file).unwrap_or_else(|err| {
            panic!(
                "map: {} field {field}: {}: {err} — restore the declared image",
                path.display(),
                file.display()
            )
        })
    };
    let heightmap = read(
        "terrain.heightmap.asset",
        &parsed.level.terrain.heightmap.asset,
    );
    let masks = read("terrain.masks.asset", &parsed.level.terrain.masks.asset);
    let constructions = ConstructionLibrary::load(root, construction_assets(&parsed.prototypes));
    let play_area = PlayArea::checked(&boundary, parsed.extent, &boundary_path);
    parsed.check_play_area(&play_area, &constructions, &path);
    let mut files = vec![
        text.as_bytes(),
        library.as_bytes(),
        boundary.as_bytes(),
        heightmap.as_slice(),
        masks.as_slice(),
    ];
    files.extend(constructions.texts());
    let digest = digest_of(&files);
    parsed.publish(play_area, constructions, digest)
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

/// The map's compatibility term, after every load-gate check. Both peers derive their world
/// from these root bytes; client-only `view/` and `derived/` products do not participate.
pub(crate) fn content_digest(root: &Path) -> u64 {
    load(root).digest
}

fn construction_assets(prototypes: &BTreeMap<String, Prototype>) -> impl Iterator<Item = &str> {
    prototypes.values().filter_map(|prototype| match prototype {
        Prototype::Construction { asset } => Some(asset.as_str()),
        Prototype::Block { .. } => None,
    })
}

/// The fold itself, pure: [`crate::fingerprint`]'s FNV-1a, byte-driven and integer-only, so every
/// platform folds the same map to the same digest. Each file's LENGTH is folded before its bytes,
/// so no two different sets of files can shift bytes across a boundary into the same digest.
fn digest_of(files: &[&[u8]]) -> u64 {
    use crate::fingerprint::{FNV_OFFSET_BASIS, fnv1a_64};
    files.iter().fold(
        fnv1a_64(FNV_OFFSET_BASIS, b"overmatch-map-content-v3"),
        |hash, bytes| {
            let hash = fnv1a_64(hash, &(bytes.len() as u64).to_le_bytes());
            fnv1a_64(hash, bytes)
        },
    )
}

/// Refuse a graybox prototype that describes no collider, naming the library file, the id, the
/// field and the fix: a box whose corners are not finite and ordered on every axis, or whose
/// substance states no friction (its collider carries the substance's, `scatter`).
/// `ConstructionLibrary::load` checks each construction's asset.
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
        Prototype::Construction { .. } => return,
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
fn parse(text: &str, library: &str, path: &Path) -> ParsedMap {
    let mut level: LevelFile = serde_json::from_str(text)
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
    let heightmap = &level.terrain.heightmap;
    let side_m = heightmap.world_extent_xz.side_m(path);
    let rows = heightmap.row_order(path);
    let (offset_m, scale_m) = (
        heightmap.height_decode.offset_m,
        heightmap.height_decode.scale_m,
    );
    // A zero span is legal — that is a deliberately level map; a negative or non-finite one NaNs
    // every sample the grid hands the sim.
    assert!(
        offset_m.is_finite()
            && scale_m.is_finite()
            && scale_m >= 0.0
            && (offset_m + scale_m).is_finite(),
        "map: {} field height_decode offset {offset_m} / scale {scale_m} — author a finite range with nonnegative scale and finite offset + scale",
        path.display(),
    );
    let masks = &level.terrain.masks;
    assert!(
        masks.resolution[0] > 0 && masks.resolution[1] > 0,
        "map: {} declares masks resolution {:?}, which is no image",
        path.display(),
        masks.resolution,
    );
    let spawn_points =
        SpawnPoints::checked(std::mem::take(&mut level.spawn_points), side_m * 0.5, path);
    ParsedMap {
        dir: path
            .parent()
            .expect("a manifest path has a containing directory")
            .to_path_buf(),
        extent: TerrainExtent {
            world_size_m: side_m,
            height_offset_m: offset_m,
            height_span_m: scale_m,
        },
        rows,
        spawn_points,
        prototypes,
        level,
    }
}

/// Parsed inputs stay private until construction resolution and cross-file admission finish.
struct ParsedMap {
    dir: PathBuf,
    extent: TerrainExtent,
    rows: RowOrder,
    spawn_points: SpawnPoints,
    prototypes: BTreeMap<String, Prototype>,
    level: LevelFile,
}

impl ParsedMap {
    fn check_play_area(&self, area: &PlayArea, constructions: &ConstructionLibrary, path: &Path) {
        for (index, point) in self.spawn_points.as_slice().iter().enumerate() {
            assert!(
                area.contains_points(&[point.to_array()]),
                "map: {} field spawn_points[{index}] — move the point inside boundary.ron field polygon",
                path.display(),
            );
        }
        let limit = crate::spawn_limit(self.extent.half_extent());
        assert!(
            area.contains_points(&[
                [-limit, -limit],
                [limit, -limit],
                [limit, limit],
                [-limit, limit]
            ]),
            "map: {} field polygon — enclose the entire spawn-limit square declared by assets/ui/spawn_map.ron field limit_fraction, or reduce that fraction",
            path.with_file_name(BOUNDARY_FILE).display(),
        );
        for instance in &self.level.instances {
            // Scatter replaces stale authored Y with terrain height; projected bounds use zero.
            let pose = instance.pose.at_height(0.0);
            let bounds = match &self.prototypes[&instance.prototype] {
                Prototype::Construction { asset } => crate::construction::footprint(
                    constructions
                        .get(asset)
                        .expect("load resolved every construction before admission"),
                    pose,
                ),
                prototype @ Prototype::Block { .. } => prototype
                    .block(pose)
                    .expect("a block prototype supplies its unit-cube transform"),
            };
            let mut points = Vec::with_capacity(8);
            // A footprint is the convex projection of all eight corners of the posed unit cube.
            for x in [-0.5, 0.5] {
                for y in [-0.5, 0.5] {
                    for z in [-0.5, 0.5] {
                        let point = bounds.transform_point(Vec3::new(x, y, z));
                        points.push([point.x, point.z]);
                    }
                }
            }
            assert!(
                area.contains_points(&points),
                "map: {} field instances[{}].translation/rotation produces footprint {points:?} — move or rotate the instance inside boundary.ron field polygon",
                path.display(),
                instance.id,
            );
        }
    }

    fn publish(
        self,
        play_area: PlayArea,
        constructions: ConstructionLibrary,
        digest: u64,
    ) -> MapManifest {
        MapManifest {
            dir: self.dir,
            heightmap: self.level.terrain.heightmap.asset,
            extent: self.extent,
            rows: self.rows,
            masks: self.level.terrain.masks.asset,
            masks_resolution: self.level.terrain.masks.resolution,
            spawn_points: self.spawn_points,
            instances: self.level.instances,
            prototypes: self.prototypes,
            constructions,
            play_area,
            digest,
        }
    }
}

#[cfg(test)]
// `pub(crate)` so every layer that asserts against the SHIPPED map reads it through the one loader.
pub(crate) mod tests {
    use super::*;

    fn parse(text: &str, library: &str, path: &Path) -> MapManifest {
        let parsed = super::parse(text, library, path);
        let boundary = play_area::tests::boundary_text(parsed.extent);
        let area = PlayArea::checked(
            &boundary,
            parsed.extent,
            &path.with_file_name(BOUNDARY_FILE),
        );
        let digest = digest_of(&[text.as_bytes(), library.as_bytes(), boundary.as_bytes()]);
        parsed.publish(area, ConstructionLibrary::default(), digest)
    }

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
        "rotation":[0,0,0,1],"id":"house_0001"}"#;

    /// The game's environment destruction fixture: one graybox block and two common construction
    /// kinds read from shipped assets, independent of the shipped map's population.
    pub(crate) fn construction_fixture_manifest() -> MapManifest {
        const LIBRARY: &str = r#"{
            "house": Block(min: (-1.0, -0.5, -1.0), max: (1.0, 2.0, 1.0), substance: "Brick", surface: (faces: (Primary, Primary, Primary, Primary, Secondary, Primary), top_tangent: Z)),
            "fir": Construction(asset: "fir_1"),
            "brick": Construction(asset: "house_1"),
        }"#;
        const INSTANCES: &str = r#"{"prototype":"house","translation":[0,0,0],
            "rotation":[0,0,0,1],"id":"house_0001"},
            {"prototype":"fir","translation":[0,0,20],
            "rotation":[0,0,0,1],"id":"fir_0001"},
            {"prototype":"brick","translation":[0,0,-20],
            "rotation":[0,0,0,1],"id":"brick_0001"}"#;
        let text = manifest_text(500.0, 0.0, 100.0, "+Z").replace(
            r#""instances":[]"#,
            &format!(r#""instances":[{INSTANCES}]"#),
        );
        let mut manifest = parse(&text, LIBRARY, Path::new("fixture/level.json"));
        manifest.constructions =
            ConstructionLibrary::load(&shipped_assets(), construction_assets(&manifest.prototypes));
        manifest
    }

    /// Re-exports must not silently resize constructions, even when the stale scale is unity
    /// or not an array. The refusal identifies the file, instance, field and repair.
    #[test]
    fn instance_scale_is_refused_at_the_manifest_with_its_repair() {
        for scale in [
            serde_json::json!([1, 1, 1]),
            serde_json::json!([2, 1, 1]),
            serde_json::Value::Null,
            serde_json::json!({"x": 1}),
        ] {
            let mut record: serde_json::Value = serde_json::from_str(HOUSE_INSTANCE).unwrap();
            record["scale"] = scale;
            let text = manifest_text(500.0, 0.0, 100.0, "+Z")
                .replace(r#""instances":[]"#, &format!(r#""instances":[{record}]"#));
            let refused = std::panic::catch_unwind(|| {
                parse(&text, HOUSE_LIBRARY, Path::new("fixture/level.json"));
            })
            .expect_err("any scale field must fail before scatter");
            let reason = refused.downcast_ref::<String>().unwrap();
            for required in [
                "fixture/level.json",
                "house_0001",
                "field scale",
                "remove the scale field",
            ] {
                assert!(reason.contains(required), "{reason}");
            }
        }
    }

    /// CI checks the default map through the production loader and terrain decoder, refusing
    /// missing manifests, missing heightmaps and undecodable images before boot.
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
        crate::terrain_grid::grid_from_png(&bytes, manifest.extent, manifest.rows)
            .expect("the declared heightmap must pass the production terrain decoder");
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

    /// Decode the declared 8-bit RGB resolution, refusing LFS pointers. Channels are independent
    /// linear weights: R = recesses, G = slopes, B = lowlands.
    #[test]
    fn shipped_terrain_masks_decode_as_declared() {
        let manifest = shipped_manifest();
        let path = shipped_assets().join(manifest.masks_asset());
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|err| panic!("terrain masks missing at {}: {err}", path.display()));
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

    /// Peers derive construction geometry and dressing from the same admitted bytes. Omitting
    /// either file from the compatibility term would admit a different collision or foliage view.
    #[test]
    fn the_digest_folds_every_admitted_construction_file() {
        let root = shipped_assets();
        let path = level_path(&root);
        let text = read_manifest(&path);
        let library = read_prototypes(&root);
        let manifest = load(&root);
        let heightmap = std::fs::read(manifest.heightmap_path()).unwrap();
        let boundary = std::fs::read(path.with_file_name(BOUNDARY_FILE)).unwrap();
        let masks = std::fs::read(manifest.dir.join(&manifest.masks)).unwrap();
        let mut files = vec![
            text.as_bytes(),
            library.as_bytes(),
            boundary.as_slice(),
            heightmap.as_slice(),
            masks.as_slice(),
        ];
        let assets: std::collections::BTreeSet<_> =
            construction_assets(&manifest.prototypes).collect();
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

    struct MapFixture(PathBuf);

    impl MapFixture {
        fn new() -> Self {
            Self::with_half_extent(10.0) // Fixture: a small terrain square.
        }

        fn with_half_extent(half_m: f32) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "overmatch-wall-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let dir = map_dir(&root);
            std::fs::create_dir_all(&dir).unwrap();
            let text = manifest_text(half_m, -2.0, 5.0, "+Z");
            let extent = super::parse(&text, EMPTY_LIBRARY, &dir.join(LEVEL_FILE)).extent;
            std::fs::write(dir.join(LEVEL_FILE), text).unwrap();
            std::fs::write(dir.join(PROTOTYPES_FILE), EMPTY_LIBRARY).unwrap();
            std::fs::write(
                dir.join(BOUNDARY_FILE),
                play_area::tests::boundary_text(extent),
            )
            .unwrap();
            image::GrayImage::new(4, 4).save(dir.join("h.png")).unwrap();
            image::RgbImage::new(4, 4).save(dir.join("m.png")).unwrap();
            Self(root)
        }

        fn edit_level(&self, edit: impl FnOnce(&mut serde_json::Value)) {
            let path = level_path(&self.0);
            let mut level = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            edit(&mut level);
            std::fs::write(path, serde_json::to_string(&level).unwrap()).unwrap();
        }

        fn polygon(&self, polygon: &str) {
            let path = level_path(&self.0);
            let text = std::fs::read_to_string(&path).unwrap();
            let library = read_prototypes(&self.0);
            let extent = super::parse(&text, &library, &path).extent;
            std::fs::write(
                path.with_file_name(BOUNDARY_FILE),
                play_area::tests::polygon_text(extent, polygon),
            )
            .unwrap();
        }

        fn refuses(&self, parts: &[&str]) {
            for digest_only in [false, true] {
                let refusal = std::panic::catch_unwind(|| {
                    if digest_only {
                        content_digest(&self.0);
                    } else {
                        load(&self.0);
                    }
                })
                .expect_err("startup and compatibility must both refuse invalid map data");
                let reason = refusal.downcast_ref::<String>().expect("formatted refusal");
                for part in parts {
                    assert!(reason.contains(part), "{reason}");
                }
            }
        }
    }

    impl Drop for MapFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove the isolated map fixture");
        }
    }

    /// Both server startup and compatibility must refuse an unnameable instance before spawn.
    #[test]
    fn instance_ordinals_are_admitted_at_load_and_digest() {
        let fixture = MapFixture::new();
        std::fs::write(map_dir(&fixture.0).join(PROTOTYPES_FILE), HOUSE_LIBRARY).unwrap();
        let maximum = usize::from(u16::MAX) + 1;
        let records: Vec<_> = (0..maximum)
            .rev()
            .map(|i| {
                serde_json::json!({
                    "prototype":"house", "id":format!("house_{i:05}"),
                    "translation":[0,0,0], "rotation":[0,0,0,1]
                })
            })
            .collect();
        fixture.edit_level(|level| level["instances"] = serde_json::json!(records));
        let admitted = load(&fixture.0);
        assert_eq!(content_digest(&fixture.0), admitted.digest);
        for (instance, ordinal) in admitted.instances.iter().zip(0..=u16::MAX) {
            assert_eq!(instance.ordinal, ordinal);
            assert_eq!(instance.id, format!("house_{ordinal:05}"));
        }
        fixture.edit_level(|level| {
            level["instances"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "prototype":"house", "id":"overflow", "translation":[0,0,0],
                    "rotation":[0,0,0,1]
                }));
        });
        fixture.refuses(&["level.json", "field instances", "reduce to at most 65536"]);
    }

    pub(crate) fn fixture_records(value: serde_json::Value) -> Vec<InstanceRecord> {
        deserialize_instances(value).unwrap()
    }

    /// Authority spawn selection relies on admission to enclose every authored candidate.
    /// The terrain check alone admits a point outside a narrower play area.
    #[test]
    fn spawn_point_outside_the_polygon_is_refused_with_its_repair() {
        let fixture = MapFixture::new();
        fixture.polygon("[(-10,-10), (9.8,-10), (9.8,10), (-10,10)]");
        fixture.edit_level(|level| level["spawn_points"] = serde_json::json!([[9.9, 0]]));
        fixture.refuses(&[
            "level.json",
            "field spawn_points[0]",
            "move the point inside",
            "boundary.ron",
        ]);
    }

    /// Player picks fill a square, including its edges. Checking only authored spawns or only
    /// the square's corners admits picks across a concave notch.
    #[test]
    fn the_entire_spawn_limit_square_must_be_inside_the_polygon() {
        let narrow = MapFixture::new();
        let x = crate::spawn_limit(10.0) * 0.5;
        narrow.polygon(&format!("[(-10,-10), ({x},-10), ({x},10), (-10,10)]"));
        narrow.refuses(&[
            "boundary.ron",
            "field polygon",
            "enclose the entire spawn-limit square",
            "limit_fraction",
        ]);
        // Fixture: the square's corners remain inside, but its top edge bridges this wide notch.
        let half = play_area::WALL_THICKNESS_M * 10.0 / crate::spawn_limit(1.0);
        let limit = crate::spawn_limit(half);
        let (x, z) = (limit * 0.1, limit * 0.9);
        let concave = MapFixture::with_half_extent(half);
        concave.polygon(&format!("[(-{half},-{half}), ({half},-{half}), ({half},{half}), ({x},{half}), ({x},{z}), (-{x},{z}), (-{x},{half}), (-{half},{half})]"));
        concave.refuses(&[
            "boundary.ron",
            "field polygon",
            "enclose the entire spawn-limit square",
        ]);
    }

    /// Scatter's colliders must fit before play. Origins, unrotated bounds, or a base-only
    /// projection each admit one of these overflowing graybox fixtures.
    #[test]
    fn block_footprints_refuse_overflow_after_yaw_and_tilt() {
        // Fixture: identity, yaw and tilt poses against the same generic square boundary.
        for (x, rotation) in [
            (9.5, [0.0, 0.0, 0.0, 1.0]),
            (8.8, [0.0, 0.38268343, 0.0, 0.9238795]),
            (8.0, [0.0, 0.0, -0.38268343, 0.9238795]),
        ] {
            let fixture = MapFixture::new();
            std::fs::write(prototypes_path(&fixture.0), HOUSE_LIBRARY).unwrap();
            let mut instance: serde_json::Value = serde_json::from_str(HOUSE_INSTANCE).unwrap();
            instance["translation"] = serde_json::json!([x, 0, 0]);
            instance["rotation"] = serde_json::json!(rotation);
            fixture.edit_level(|level| level["instances"] = serde_json::json!([instance]));
            fixture.refuses(&[
                "level.json",
                "field instances[house_0001].translation/rotation",
                "move or rotate the instance inside",
            ]);
        }
        // Fixture: touching the boundary is legal; no authored clearance is added by the gate.
        let touching = MapFixture::new();
        std::fs::write(prototypes_path(&touching.0), HOUSE_LIBRARY).unwrap();
        let mut instance: serde_json::Value = serde_json::from_str(HOUSE_INSTANCE).unwrap();
        instance["translation"] = serde_json::json!([9, 0, 0]);
        touching.edit_level(|level| level["instances"] = serde_json::json!([instance]));
        load(&touching.0);
    }

    /// Resolved construction bounds, rather than placement origins or view geometry, enclose
    /// everything scatter can spawn. The fixture takes its size from the construction owner.
    #[test]
    fn construction_footprint_crossing_names_the_instance_and_repair() {
        let fixture = MapFixture::new();
        // Fixture: one shipped construction supplies real admitted bounds, with an arbitrary id.
        let asset = "fir_1";
        let library = format!("{{\"fixture\": Construction(asset: \"{asset}\")}}");
        std::fs::write(prototypes_path(&fixture.0), library).unwrap();
        let dir = fixture.0.join("constructions").join(asset);
        std::fs::create_dir_all(&dir).unwrap();
        for suffix in ["construction", "dressing"] {
            let file = format!("{asset}.{suffix}.ron");
            std::fs::copy(
                shipped_assets()
                    .join("constructions")
                    .join(asset)
                    .join(&file),
                dir.join(file),
            )
            .unwrap();
        }
        let constructions = ConstructionLibrary::load(&fixture.0, [asset]);
        let (min, max) = constructions.get(asset).unwrap().bounds();
        let x = 10.0 - max.x + (max.x - min.x) * 0.25;
        fixture.edit_level(|level| level["instances"] = serde_json::json!([{
            "prototype":"fixture", "id":"construction_fixture", "translation":[x,0,0], "rotation":[0,0,0,1]
        }]));
        fixture.refuses(&[
            "level.json",
            "instances[construction_fixture].translation/rotation",
            "move or rotate the instance inside",
        ]);
    }

    /// A footprint requires a finite normalized orientation. The map edge must refuse these
    /// poses before either scatter or compatibility can consume them.
    #[test]
    fn invalid_placement_rotations_name_the_field_and_repair() {
        // Fixture: a directionless quaternion and one whose norm overflows simulation precision.
        for rotation in [[0.0; 4], [1.0e30, 0.0, 0.0, 1.0]] {
            let fixture = MapFixture::new();
            std::fs::write(prototypes_path(&fixture.0), HOUSE_LIBRARY).unwrap();
            let mut instance: serde_json::Value = serde_json::from_str(HOUSE_INSTANCE).unwrap();
            instance["rotation"] = serde_json::json!(rotation);
            fixture.edit_level(|level| level["instances"] = serde_json::json!([instance]));
            fixture.refuses(&[
                "level.json",
                "field instances[house_0001].rotation",
                "author a finite quaternion with a normalizable length",
            ]);
        }
    }

    /// A handshake must not mint a digest for a polygon whose outward wall enters the region.
    #[test]
    fn wall_overlap_is_refused_by_load_and_digest() {
        let fixture = MapFixture::with_half_extent(1000.0);
        // Fixture: a notch beyond the spawn square, narrower than the actual wall depth.
        let x = play_area::WALL_THICKNESS_M * 0.25;
        fixture.polygon(&format!("[(-1000,-1000), (1000,-1000), (1000,1000), ({x},1000), ({x},980), (-{x},980), (-{x},1000), (-1000,1000)]"));
        fixture.refuses(&[
            "boundary.ron",
            "field polygon edge",
            "widen or reshape the notch",
        ]);
    }

    /// Compatibility must change with boundary or mask bytes, including edits that still pass
    /// admission. Both consumers use the same snapshot and neither may skip its checks.
    #[test]
    fn boundary_and_mask_mutations_change_the_admitted_digest() {
        let fixture = MapFixture::new();
        let root = &fixture.0;
        let base = load(root).digest;
        assert_eq!(content_digest(root), base);
        let path = map_dir(root).join(BOUNDARY_FILE);
        let boundary = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, boundary.replace("ceiling_m: (4", "ceiling_m: (5")).unwrap();
        assert_ne!(
            content_digest(root),
            base,
            "boundary edit changes the compatibility term"
        );
        std::fs::write(&path, boundary).unwrap();
        let masks = map_dir(root).join("m.png");
        let mut image = image::RgbImage::new(4, 4);
        image.put_pixel(0, 0, image::Rgb([1, 0, 0])); // Fixture: a different valid mask sample.
        image.save(masks).unwrap();
        assert_ne!(
            content_digest(root),
            base,
            "mask edit changes the compatibility term"
        );
    }

    /// A handshake must refuse the same broken boundary as map startup, rather than mint a tag
    /// for data the simulation will reject. The file, field and fix survive through both edges.
    #[test]
    fn digest_cannot_bypass_boundary_admission() {
        let fixture = MapFixture::new();
        let path = map_dir(&fixture.0).join(BOUNDARY_FILE);
        let boundary = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, boundary.replace("floor_m: (-3", "floor_m: (-2")).unwrap();
        fixture.refuses(&["boundary.ron", "floor_m", "below height_decode.offset_m"]);
    }

    /// Finite source coordinates can still overflow a derived range. Terrain and boundary
    /// consumers rely on finite metres, so the descriptor gate must refuse before publication.
    #[test]
    fn derived_terrain_ranges_must_remain_finite() {
        // Fixture: representable inputs whose side or offset + scale overflows f32.
        for (text, field) in [
            (manifest_text(f32::MAX, 0.0, 1.0, "+Z"), "world_extent_xz"),
            (
                manifest_text(10.0, f32::MAX, f32::MAX, "+Z"),
                "height_decode",
            ),
        ] {
            let refusal = std::panic::catch_unwind(|| {
                super::parse(&text, EMPTY_LIBRARY, Path::new("fixture/level.json"));
            })
            .expect_err("overflowing ranges must be refused");
            let reason = refusal.downcast_ref::<String>().unwrap();
            for part in ["fixture/level.json", field, "author a finite"] {
                assert!(reason.contains(part), "{reason}");
            }
        }
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
