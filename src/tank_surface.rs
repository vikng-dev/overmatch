//! The tank surface: every drawn primitive wearing a material that names a visual
//! (`assets/materials/materials.ron`) draws through ONE generic [`TankSurfaceMaterial`] — the
//! material's visual from the shared library (`assets/materials/visuals.ron`), then the vehicle's
//! livery (`assets/<tank>/<tank>.livery.ron`): its paint and camouflage, where the visual takes
//! paint. Design, the material table and the library: `.agents/docs/design/tank-surfaces.md`.
//!
//! VIEW ONLY (ADR-0014). Nothing here is sim state: the dedicated server reads neither data file,
//! never builds the material, and parses a material's visual key as text and nothing more.
//!
//! # Entry points
//!
//! - [`plugin`]: mounted on the windowed roots beside `geometry_lod::view_plugin`. It reads the
//!   library and the selected vehicle's livery once at build ([`SurfaceData`]; a file that does not
//!   load panics, ADR-0011) and dresses each tank's view the moment it lands.
//! - The dress ([`Dress::vehicle`]): on a tank's `WorldInstanceReady`, every primitive whose glTF
//!   material names a visual swaps its `StandardMaterial` for a surface material and takes a
//!   [`MeshTag`] naming its rest frame. A primitive whose material names none (decor, proxies, a
//!   crew witness) keeps its own.
//!
//! # Invariants
//!
//! - ONE MATERIAL PER (VEHICLE, MATERIAL): made at the vehicle's first bind and shared by every
//!   instance of it ([`SurfaceVehicles`]), so a battle of one vehicle type draws as many surface
//!   materials as the glb has materials, as it did before.
//! - THE REST TABLE IS BUILT ONCE PER VEHICLE and never resized: row 0 is the identity, then one row
//!   per node path of the first bind. A later instance finds its rows by node path; an untagged mesh
//!   reads row 0. The shader's rest frame (`assets/shaders/tank_surface.wgsl`) is the mesh-local
//!   frame through every node up to, and not including, the first the tank file articulates
//!   ([`rest_row`]), so plates of one rigid part share it and a moving part carries it.
//! - A RUNG WEARS ITS SOURCE'S MATERIAL AND TAG: `geometry_lod` copies both, so a LOD switch changes
//!   the silhouette and nothing else.
//! - Rows are rigid up to a uniform scale: the shader takes their 3×3 as the normal's rotation and
//!   renormalises. A node with a non-uniform scale is warned about once.

use std::collections::{BTreeMap, HashMap, HashSet};

use bevy::asset::{AssetLoadFailedEvent, RenderAssetUsages};
use bevy::ecs::system::SystemParam;
use bevy::gltf::GltfMaterialName;
use bevy::image::ImageSampler;
use bevy::math::Affine3A;
use bevy::mesh::MeshTag;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldInstanceReady};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

use crate::provenance::Sourced;
use crate::substances::SubstanceRegistry;
use crate::world::{MapEncoding, MapTiling, surface_map, surface_sampler};

/// A tank's drawn surface: bevy's `StandardMaterial` carrying the lighting parameters and, for a
/// flat visual, its look; extended with [`TankSurface`] (the visual's maps, the livery, the rest
/// table).
pub(crate) type TankSurfaceMaterial = ExtendedMaterial<StandardMaterial, TankSurface>;

const SHADER: &str = "shaders/tank_surface.wgsl";
/// The shared visual library, relative to the asset root.
const VISUALS_PATH: &str = "materials/visuals.ron";
/// The selected vehicle's livery, relative to the asset root.
const LIVERY_PATH: &str = crate::assets::tank_file!(".livery.ron");

/// The extension's flags (`FLAG_*` in the shader). The visual ships maps.
const FLAG_MAPS: u32 = 1;
/// The visual takes the livery's paint.
const FLAG_PAINT: u32 = 2;
/// The livery carries camouflage.
const FLAG_CAMO: u32 = 4;
/// The most camouflage colours a pattern carries: its R, G and B coverage.
const CAMO_COLOURS: usize = 3;

// ---------------------------------------------------------------------------------------------
// The data
// ---------------------------------------------------------------------------------------------

/// An sRGB colour as authored, `"#RRGGBB"`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Srgb([u8; 3]);

impl Srgb {
    /// `"#RRGGBB"`, or why not.
    fn parse(text: &str) -> Result<Self, String> {
        let hex = text
            .strip_prefix('#')
            .filter(|hex| hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| format!("{text:?} is not an sRGB colour `#RRGGBB`"))?;
        let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex, checked");
        Ok(Self([channel(0), channel(2), channel(4)]))
    }

    /// Linear RGB, what the shader lights.
    fn linear(self) -> Vec3 {
        let [r, g, b] = self.0;
        let linear = Color::srgb_u8(r, g, b).to_linear();
        Vec3::new(linear.red, linear.green, linear.blue)
    }
}

impl<'de> Deserialize<'de> for Srgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

/// A map keyed by name whose repeated key is refused, never overwritten: a second entry for one
/// key is two answers, and the file cannot say which is meant.
struct UniqueMap<V>(BTreeMap<String, V>);

impl<'de, V: Deserialize<'de>> Deserialize<'de> for UniqueMap<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<V>(std::marker::PhantomData<V>);

        impl<'de, V: Deserialize<'de>> Visitor<'de> for MapVisitor<V> {
            type Value = UniqueMap<V>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of names, each named once")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut out = BTreeMap::new();
                while let Some((key, value)) = access.next_entry::<String, V>()? {
                    if out.contains_key(&key) {
                        return Err(de::Error::custom(format!("{key:?} is declared twice")));
                    }
                    out.insert(key, value);
                }
                Ok(UniqueMap(out))
            }
        }

        deserializer.deserialize_map(MapVisitor(std::marker::PhantomData))
    }
}

/// One visual of the library, as authored.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) enum Visual {
    /// A texture pack: `<maps>_diff.ktx2`, `<maps>_nor_gl.ktx2` and `<maps>_arm.ktx2`, relative to
    /// the asset root, one repeat spanning `metres_per_repeat`.
    Maps {
        maps: String,
        metres_per_repeat: Sourced<f32>,
        takes_paint: bool,
    },
    /// No maps: a colour and two factors.
    Flat {
        colour: Sourced<Srgb>,
        roughness: Sourced<f32>,
        metallic: Sourced<f32>,
        takes_paint: bool,
    },
}

impl Visual {
    fn takes_paint(&self) -> bool {
        match self {
            Self::Maps { takes_paint, .. } | Self::Flat { takes_paint, .. } => *takes_paint,
        }
    }

    /// The entry's own validity: a named pack, a positive repeat, factors in `0..=1`.
    fn checked(&self, key: &str) -> Result<(), String> {
        let unit = |field: &str, value: f32| {
            (0.0..=1.0)
                .contains(&value)
                .then_some(())
                .ok_or_else(|| format!("visual {key:?}: {field} must be in 0..=1 (got {value})"))
        };
        match self {
            Self::Maps {
                maps,
                metres_per_repeat,
                ..
            } => {
                if maps.trim().is_empty() {
                    return Err(format!("visual {key:?} names no maps"));
                }
                positive(
                    &format!("visual {key:?}: metres_per_repeat"),
                    metres_per_repeat.0,
                )
            }
            Self::Flat {
                roughness,
                metallic,
                ..
            } => unit("roughness", roughness.0).and(unit("metallic", metallic.0)),
        }
    }
}

/// `value` is finite and positive, or why not.
fn positive(what: &str, value: f32) -> Result<(), String> {
    (value.is_finite() && value > 0.0)
        .then_some(())
        .ok_or_else(|| format!("{what} must be finite and positive (got {value})"))
}

/// The shared visual library (`assets/materials/visuals.ron`).
#[derive(Debug)]
pub(crate) struct VisualLibrary(BTreeMap<String, Visual>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryFile {
    visuals: UniqueMap<Visual>,
}

impl VisualLibrary {
    /// Parse the library: every visual valid, no key twice.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let file: LibraryFile = ron::de::from_str(text).map_err(|err| err.to_string())?;
        for (key, visual) in &file.visuals.0 {
            visual.checked(key)?;
        }
        Ok(Self(file.visuals.0))
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Visual> {
        self.0.get(key)
    }
}

/// A vehicle's livery (`assets/<tank>/<tank>.livery.ron`): the paint, and optionally camouflage.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct Livery {
    paint: Paint,
    camouflage: Option<Camouflage>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct Paint {
    colour: Sourced<Srgb>,
    /// 0 dead flat .. 1 mirror gloss; the paint's perceptual roughness is `1 − sheen`.
    sheen: Sourced<f32>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct Camouflage {
    /// `assets/materials/camouflage/<pattern>.ktx2`.
    pattern: String,
    metres_per_repeat: Sourced<f32>,
    /// The pattern's R, G and B coverage in turn.
    colours: Vec<Sourced<Srgb>>,
}

impl Livery {
    /// Parse a livery: a sheen in `0..=1`, a named pattern, a positive repeat, one to three
    /// colours.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let livery: Self = ron::de::from_str(text).map_err(|err| err.to_string())?;
        let sheen = livery.paint.sheen.0;
        if !(0.0..=1.0).contains(&sheen) {
            return Err(format!("paint sheen must be in 0..=1 (got {sheen})"));
        }
        if let Some(camouflage) = &livery.camouflage {
            if camouflage.pattern.trim().is_empty() {
                return Err("camouflage names no pattern".into());
            }
            positive(
                "camouflage metres_per_repeat",
                camouflage.metres_per_repeat.0,
            )?;
            let count = camouflage.colours.len();
            if !(1..=CAMO_COLOURS).contains(&count) {
                return Err(format!(
                    "camouflage names {count} colours — a pattern carries one to {CAMO_COLOURS}, \
                     its R, G and B coverage"
                ));
            }
        }
        Ok(livery)
    }

    /// The camouflage pattern's path, relative to the asset root.
    fn pattern_path(&self) -> Option<String> {
        self.camouflage
            .as_ref()
            .map(|camouflage| format!("materials/camouflage/{}.ktx2", camouflage.pattern))
    }
}

/// The library and the selected vehicle's livery, read once at [`plugin`] build.
#[derive(Resource, Debug)]
pub(crate) struct SurfaceData {
    library: VisualLibrary,
    livery: Livery,
}

impl SurfaceData {
    /// The shipped files under the asset root. Panics on a file that does not load: both are
    /// required, in-repo data (ADR-0011), and a tank drawn without them is a build defect.
    fn shipped() -> Self {
        let read = |path: &str| {
            let full = crate::assets::asset_root().join(path);
            std::fs::read_to_string(&full).unwrap_or_else(|err| {
                panic!("tank surface: {} is unreadable: {err}", full.display())
            })
        };
        let library = VisualLibrary::parse(&read(VISUALS_PATH))
            .unwrap_or_else(|err| panic!("tank surface: {VISUALS_PATH} is unusable: {err}"));
        let livery = Livery::parse(&read(LIVERY_PATH))
            .unwrap_or_else(|err| panic!("tank surface: {LIVERY_PATH} is unusable: {err}"));
        Self { library, livery }
    }
}

// ---------------------------------------------------------------------------------------------
// The material
// ---------------------------------------------------------------------------------------------

/// The livery and the visual's scale, one block of [`TankSurface`].
#[derive(ShaderType, Reflect, Debug, Clone, Default)]
pub(crate) struct TankSurfaceParams {
    /// rgb: the paint, linear. a: its perceptual roughness, `1 − sheen`.
    pub paint: Vec4,
    /// The camouflage colours, linear rgb, for the pattern's R, G and B coverage; a: 1 where the
    /// livery names that colour.
    pub camo_r: Vec4,
    pub camo_g: Vec4,
    pub camo_b: Vec4,
    /// x: the visual's repeats per metre. y: the pattern's repeats per metre. zw: unused.
    pub scale: Vec4,
    /// `FLAG_MAPS`, `FLAG_PAINT`, `FLAG_CAMO`.
    pub flags: u32,
}

/// The surface half of [`TankSurfaceMaterial`]. Bindings start at 100, bevy's convention for a
/// material extension. One sampler for every map and the pattern: each sampler a material declares
/// counts against the 16 a stage gets (MEASURED: wgpu's default `max_samplers_per_shader_stage`).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct TankSurface {
    #[uniform(100)]
    pub params: TankSurfaceParams,
    /// The visual's albedo where the surface shows it, else [`SurfaceVehicles`]'s stand-in. Bevy
    /// binds the image's own sampler beside it, so this binding always holds an image carrying the
    /// repeating surface sampler: left empty, bevy's fallback would clamp every map and the pattern.
    #[texture(101)]
    #[sampler(102)]
    pub visual_albedo: Option<Handle<Image>>,
    #[texture(103)]
    pub visual_normal: Option<Handle<Image>>,
    #[texture(104)]
    pub visual_arm: Option<Handle<Image>>,
    #[texture(105)]
    pub camo_pattern: Option<Handle<Image>>,
    /// The vehicle's rest table, one rigid 3×4 row per node path ([`RestRow`]), indexed by the
    /// primitive's [`MeshTag`].
    #[storage(106, read_only)]
    pub rest_frames: Handle<ShaderBuffer>,
}

impl MaterialExtension for TankSurface {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

/// One row of the rest table: the rows of the 3×4 transform from a primitive's mesh-local frame to
/// its part's rest frame.
#[derive(ShaderType, Clone, Copy, Debug, PartialEq)]
pub(crate) struct RestRow {
    x: Vec4,
    y: Vec4,
    z: Vec4,
}

impl RestRow {
    const IDENTITY: Self = Self {
        x: Vec4::X,
        y: Vec4::Y,
        z: Vec4::Z,
    };

    fn of(rest: Affine3A) -> Self {
        let m = rest.matrix3;
        let t = rest.translation;
        Self {
            x: m.row(0).extend(t.x),
            y: m.row(1).extend(t.y),
            z: m.row(2).extend(t.z),
        }
    }
}

/// A visual's three maps, loaded once.
#[derive(Clone)]
struct VisualMaps {
    albedo: Handle<Image>,
    normal: Handle<Image>,
    arm: Handle<Image>,
}

/// The surface material for the glb material `original`, of visual `visual`, under `livery`.
/// `stand_in` is [`SurfaceVehicles`]'s: binding 101 holds it where no albedo is drawn.
fn surface_material(
    original: &StandardMaterial,
    visual: &Visual,
    maps: Option<VisualMaps>,
    livery: &Livery,
    pattern: Option<Handle<Image>>,
    stand_in: &Handle<Image>,
    rest_frames: Handle<ShaderBuffer>,
) -> TankSurfaceMaterial {
    // The glb material keeps its pipeline-shaping state (culling, sidedness, alpha mode); its maps
    // and factors give way to the visual's.
    let mut base = original.clone();
    base.base_color_texture = None;
    base.normal_map_texture = None;
    base.metallic_roughness_texture = None;
    base.occlusion_texture = None;
    base.emissive_texture = None;
    base.uv_transform = default();
    let mut flags = 0;
    let mut visual_scale = 0.0;
    match visual {
        Visual::Maps {
            metres_per_repeat, ..
        } => {
            // Factors of 1 pass the maps through.
            base.base_color = Color::WHITE;
            base.perceptual_roughness = 1.0;
            base.metallic = 1.0;
            visual_scale = 1.0 / metres_per_repeat.0;
            flags |= FLAG_MAPS;
        }
        Visual::Flat {
            colour,
            roughness,
            metallic,
            ..
        } => {
            let [r, g, b] = colour.0.0;
            base.base_color = Color::srgb_u8(r, g, b);
            base.perceptual_roughness = roughness.0;
            base.metallic = metallic.0;
        }
    }
    let paint = &livery.paint;
    let mut params = TankSurfaceParams {
        paint: paint.colour.0.linear().extend(1.0 - paint.sheen.0),
        scale: Vec4::new(visual_scale, 0.0, 0.0, 0.0),
        ..default()
    };
    if visual.takes_paint() {
        flags |= FLAG_PAINT;
    }
    let mut camo_pattern = None;
    if let (Some(camouflage), Some(pattern)) = (&livery.camouflage, pattern)
        && visual.takes_paint()
    {
        let colour = |k: usize| {
            camouflage
                .colours
                .get(k)
                .map_or(Vec4::ZERO, |colour| colour.0.linear().extend(1.0))
        };
        params.camo_r = colour(0);
        params.camo_g = colour(1);
        params.camo_b = colour(2);
        params.scale.y = 1.0 / camouflage.metres_per_repeat.0;
        flags |= FLAG_CAMO;
        camo_pattern = Some(pattern);
    }
    params.flags = flags;
    TankSurfaceMaterial {
        base,
        extension: TankSurface {
            params,
            visual_albedo: Some(maps.as_ref().map_or(stand_in, |maps| &maps.albedo).clone()),
            visual_normal: maps.as_ref().map(|maps| maps.normal.clone()),
            visual_arm: maps.map(|maps| maps.arm),
            camo_pattern,
            rest_frames,
        },
    }
}

// ---------------------------------------------------------------------------------------------
// The dress
// ---------------------------------------------------------------------------------------------

/// One vehicle's surface state, built at its first bind.
struct Vehicle {
    rest_frames: Handle<ShaderBuffer>,
    /// Node path → row of the rest table.
    rows: HashMap<String, u32>,
    /// glb material → its surface material.
    materials: HashMap<AssetId<StandardMaterial>, Handle<TankSurfaceMaterial>>,
}

/// Every vehicle dressed so far, by its view scene, and the maps loaded for them.
#[derive(Resource)]
pub(crate) struct SurfaceVehicles {
    vehicles: HashMap<AssetId<WorldAsset>, Vehicle>,
    maps: HashMap<String, VisualMaps>,
    pattern: Option<Handle<Image>>,
    /// Every map and pattern loaded, held by id for [`report_failed_surface_map`].
    required: HashSet<AssetId<Image>>,
    /// A 1×1 white image carrying the surface sampler ([`surface_sampler`]): what binding 101
    /// holds where the surface draws no albedo, so the sampler beside it still repeats.
    stand_in: Handle<Image>,
}

impl FromWorld for SurfaceVehicles {
    fn from_world(world: &mut World) -> Self {
        let mut stand_in = Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[255; 4],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        stand_in.sampler = ImageSampler::Descriptor(surface_sampler(MapTiling::Repeat));
        Self {
            vehicles: HashMap::new(),
            maps: HashMap::new(),
            pattern: None,
            required: HashSet::new(),
            stand_in: world.resource_mut::<Assets<Image>>().add(stand_in),
        }
    }
}

/// The rest frame of `primitive` under view root `root`: its own transform, then each ancestor's
/// up to, and not including, the first `articulated` one or the root — the frame of the rigid part
/// it rides. Authored transforms only: an articulated node, the one thing a pose seeds or animates,
/// is never read.
fn rest_row(
    primitive: Entity,
    root: Entity,
    articulated: &dyn Fn(&str) -> bool,
    parents: &Query<&ChildOf>,
    names: &Query<&Name>,
    transforms: &Query<&Transform>,
) -> Affine3A {
    let local = |entity| {
        transforms
            .get(entity)
            .map_or(Affine3A::IDENTITY, Transform::compute_affine)
    };
    let mut rest = local(primitive);
    let mut node = parents.get(primitive).ok().map(ChildOf::parent);
    while let Some(entity) = node
        && entity != root
    {
        if names
            .get(entity)
            .is_ok_and(|name| articulated(name.as_str()))
        {
            break;
        }
        rest = local(entity) * rest;
        node = parents.get(entity).ok().map(ChildOf::parent);
    }
    rest
}

/// The names of `primitive`'s ancestors below `root`, root-most first, joined by `/`: the key a
/// later instance of the same vehicle finds its rows by.
fn node_path(
    primitive: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    names: &Query<&Name>,
) -> String {
    let mut path: Vec<&str> = parents
        .iter_ancestors(primitive)
        .take_while(|ancestor| *ancestor != root)
        .map(|ancestor| names.get(ancestor).map_or("", Name::as_str))
        .collect();
    path.reverse();
    path.join("/")
}

/// The view tree a dress reads.
#[derive(SystemParam)]
pub(crate) struct ViewTree<'w, 's> {
    roots: Query<'w, 's, &'static WorldAssetRoot, With<crate::tank::Tank>>,
    children: Query<'w, 's, &'static Children>,
    parents: Query<'w, 's, &'static ChildOf>,
    names: Query<'w, 's, &'static Name>,
    transforms: Query<'w, 's, &'static Transform>,
    primitives: Query<
        'w,
        's,
        (
            &'static MeshMaterial3d<StandardMaterial>,
            &'static GltfMaterialName,
        ),
    >,
}

/// Everything a dress writes or reads beside the view tree.
#[derive(SystemParam)]
pub(crate) struct Dress<'w, 's> {
    tree: ViewTree<'w, 's>,
    data: Res<'w, SurfaceData>,
    blueprint: Option<Res<'w, crate::bake::TankBlueprint>>,
    vehicles: ResMut<'w, SurfaceVehicles>,
    standard: Res<'w, Assets<StandardMaterial>>,
    surfaces: ResMut<'w, Assets<TankSurfaceMaterial>>,
    buffers: ResMut<'w, Assets<ShaderBuffer>>,
    server: Res<'w, AssetServer>,
    commands: Commands<'w, 's>,
}

impl Dress<'_, '_> {
    /// Dress the tank view rooted at `root` (module doc). Not a tank, or no scene: nothing.
    pub(crate) fn vehicle(&mut self, root: Entity) {
        let Ok(scene) = self.tree.roots.get(root) else {
            return;
        };
        let scene = scene.0.id();
        let registry = SubstanceRegistry::shipped_static();
        let spec = self.blueprint.as_deref().map(|blueprint| &*blueprint.spec);
        let articulated: HashSet<&str> = spec
            .map(|spec| spec.articulated_nodes().collect())
            .unwrap_or_default();
        let articulated = |name: &str| articulated.contains(name);

        // Every primitive whose material names a visual, with the visual.
        let worn: Vec<(Entity, Handle<StandardMaterial>, Visual)> = self
            .tree
            .children
            .iter_descendants(root)
            .filter_map(|entity| {
                let (material, name) = self.tree.primitives.get(entity).ok()?;
                let name = name.0.as_str();
                let visual = registry.visual(name)?;
                let visual = self.data.library.get(visual).unwrap_or_else(|| {
                    panic!(
                        "tank surface: material {name:?} names visual {visual:?}, which \
                         {VISUALS_PATH} does not carry"
                    )
                });
                Some((entity, material.0.clone(), visual.clone()))
            })
            .collect();
        if worn.is_empty() {
            return;
        }

        if !self.vehicles.vehicles.contains_key(&scene) {
            let mut rows = HashMap::new();
            let mut table = vec![RestRow::IDENTITY];
            for (entity, ..) in &worn {
                let path = node_path(*entity, root, &self.tree.parents, &self.tree.names);
                if rows.contains_key(&path) {
                    continue;
                }
                let rest = rest_row(
                    *entity,
                    root,
                    &articulated,
                    &self.tree.parents,
                    &self.tree.names,
                    &self.tree.transforms,
                );
                let lengths = [
                    rest.matrix3.x_axis,
                    rest.matrix3.y_axis,
                    rest.matrix3.z_axis,
                ]
                .map(|axis| axis.length());
                if lengths
                    .iter()
                    .any(|length| (length - lengths[0]).abs() > 1e-3)
                {
                    warn_once!(
                        "tank surface: node {path:?} carries a non-uniform scale; its surface's \
                         normals lean"
                    );
                }
                rows.insert(path, table.len() as u32);
                table.push(RestRow::of(rest));
            }
            let rest_frames = self.buffers.add(ShaderBuffer::from(table));
            self.vehicles.vehicles.insert(
                scene,
                Vehicle {
                    rest_frames,
                    rows,
                    materials: HashMap::new(),
                },
            );
        }

        for (entity, original, visual) in worn {
            let handle = self.material(scene, &original, &visual);
            let path = node_path(entity, root, &self.tree.parents, &self.tree.names);
            let vehicle = &self.vehicles.vehicles[&scene];
            let row = vehicle.rows.get(&path).copied().unwrap_or(0);
            self.commands
                .entity(entity)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert((MeshMaterial3d(handle), MeshTag(row)));
        }
    }

    /// The vehicle's surface material for glb material `original`, made on first sight.
    fn material(
        &mut self,
        scene: AssetId<WorldAsset>,
        original: &Handle<StandardMaterial>,
        visual: &Visual,
    ) -> Handle<TankSurfaceMaterial> {
        if let Some(handle) = self.vehicles.vehicles[&scene].materials.get(&original.id()) {
            return handle.clone();
        }
        let maps = match visual {
            Visual::Maps { maps, .. } => Some(self.maps(maps)),
            Visual::Flat { .. } => None,
        };
        let pattern = self.pattern();
        let base = self.standard.get(original).cloned().unwrap_or_default();
        let rest_frames = self.vehicles.vehicles[&scene].rest_frames.clone();
        let made = surface_material(
            &base,
            visual,
            maps,
            &self.data.livery,
            pattern,
            &self.vehicles.stand_in,
            rest_frames,
        );
        let handle = self.surfaces.add(made);
        self.vehicles
            .vehicles
            .get_mut(&scene)
            .expect("made above")
            .materials
            .insert(original.id(), handle.clone());
        handle
    }

    /// A pack's three maps, loaded on first use.
    fn maps(&mut self, maps: &str) -> VisualMaps {
        if let Some(loaded) = self.vehicles.maps.get(maps) {
            return loaded.clone();
        }
        let load = |map: &str, encoding| {
            surface_map(
                &self.server,
                &format!("{maps}_{map}.ktx2"),
                encoding,
                MapTiling::Repeat,
            )
        };
        let loaded = VisualMaps {
            albedo: load("diff", MapEncoding::Srgb),
            normal: load("nor_gl", MapEncoding::Linear),
            arm: load("arm", MapEncoding::Linear),
        };
        let required = &mut self.vehicles.required;
        required.extend([loaded.albedo.id(), loaded.normal.id(), loaded.arm.id()]);
        self.vehicles.maps.insert(maps.to_owned(), loaded.clone());
        loaded
    }

    /// The livery's camouflage pattern, loaded on first use; `None` without camouflage.
    fn pattern(&mut self) -> Option<Handle<Image>> {
        if self.vehicles.pattern.is_none()
            && let Some(path) = self.data.livery.pattern_path()
        {
            let pattern = surface_map(&self.server, &path, MapEncoding::Linear, MapTiling::Repeat);
            self.vehicles.required.insert(pattern.id());
            self.vehicles.pattern = Some(pattern);
        }
        self.vehicles.pattern.clone()
    }
}

/// Dress a tank's view as it lands.
fn dress_on_ready(ready: On<WorldInstanceReady>, mut dress: Dress) {
    dress.vehicle(ready.entity);
}

/// Surface a failed surface-map load (ADR-0011, the stance `world::report_failed_terrain_map`
/// takes). A map that fails to decode leaves the material unprepared, and bevy then draws no
/// armour at all while every collider stands. Required, in-repo asset ⇒ panic.
fn report_failed_surface_map(
    mut failures: MessageReader<AssetLoadFailedEvent<Image>>,
    vehicles: Res<SurfaceVehicles>,
) {
    for failure in failures.read() {
        if vehicles.required.contains(&failure.id) {
            let (path, err) = (&failure.path, &failure.error);
            error!("required tank surface map {path} failed to load: {err}");
            panic!("required tank surface map {path} failed to load: {err}");
        }
    }
}

/// Mounted on the windowed roots beside `geometry_lod::view_plugin`: the material's render half is
/// behind the render app, and the dress only ever runs where a view scene lands.
pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(MaterialPlugin::<TankSurfaceMaterial>::default())
        .insert_resource(SurfaceData::shipped())
        .init_resource::<SurfaceVehicles>()
        .add_observer(dress_on_ready)
        .add_systems(Update, report_failed_surface_map);
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn shipped() -> SurfaceData {
        SurfaceData::shipped()
    }

    /// EVERY WORN MATERIAL HAS A LOOK. The dress panics on a material whose visual the library does
    /// not carry, so every visual key the shipped registry names must be in the shipped library: a
    /// renamed visual or a new material with a typo'd key fails here, not on the first tank drawn.
    #[test]
    fn the_shipped_library_carries_every_visual_a_material_names() {
        let data = shipped();
        let registry = SubstanceRegistry::shipped_static();
        for name in registry.keys() {
            if let Some(visual) = registry.visual(name) {
                assert!(
                    data.library.get(visual).is_some(),
                    "material {name} names visual {visual}, which the library does not carry"
                );
            }
        }
    }

    /// THE SHIPPED MAPS ARE WHAT THE MATERIAL BINDS. Each pack's three maps and the selected
    /// livery's pattern are real KTX2 (not Git LFS pointers) that bevy's own loader transcodes,
    /// 1024² with a full mip chain: a map that fails to load leaves the armour undrawn while its
    /// collider stands, and one without mips shimmers at every range. Pinned against
    /// `CompressedImageFormats::BC`, as the terrain's maps are.
    #[test]
    fn the_shipped_surface_maps_are_mipmapped_1k_ktx2() {
        use bevy::image::{CompressedImageFormats, ktx2_buffer_to_image};
        // 1024² ⇒ 11 levels down to 1×1.
        const FULL_MIP_CHAIN: u32 = 11;
        let data = shipped();
        let mut files: Vec<(String, bool)> = Vec::new();
        for visual in data.library.0.values() {
            if let Visual::Maps { maps, .. } = visual {
                files.push((format!("{maps}_diff.ktx2"), true));
                files.push((format!("{maps}_nor_gl.ktx2"), false));
                files.push((format!("{maps}_arm.ktx2"), false));
            }
        }
        files.extend(data.livery.pattern_path().map(|path| (path, false)));
        assert!(files.len() > 3, "the library ships packs");
        for (path, is_srgb) in files {
            let full = crate::assets::asset_root().join(&path);
            let bytes = std::fs::read(&full)
                .unwrap_or_else(|err| panic!("surface map missing at {}: {err}", full.display()));
            assert!(
                bytes.len() > 4096,
                "{path} is {} bytes — a Git LFS POINTER, not the map",
                bytes.len(),
            );
            let image = ktx2_buffer_to_image(&bytes, CompressedImageFormats::BC, is_srgb)
                .unwrap_or_else(|err| panic!("{path} failed to transcode: {err:?}"));
            let size = image.texture_descriptor.size;
            assert_eq!((size.width, size.height), (1024, 1024), "{path} is 1k");
            assert_eq!(
                image.texture_descriptor.mip_level_count, FULL_MIP_CHAIN,
                "{path} carries a full mip chain",
            );
        }
    }

    /// A COLOUR IS `#RRGGBB` AND LIGHTS LINEAR. The livery's colours are authored as sRGB and the
    /// shader mixes them in linear light: a parser that read the channels in the wrong order, or a
    /// conversion that skipped the sRGB curve, would recolour every tank.
    #[test]
    fn an_srgb_colour_parses_and_converts_to_linear() {
        assert_eq!(Srgb::parse("#9A8952"), Ok(Srgb([0x9A, 0x89, 0x52])));
        for refused in ["9A8952", "#9A895", "#9A89520", "#GG8952", ""] {
            assert!(Srgb::parse(refused).is_err(), "{refused:?}");
        }
        let linear = Srgb([255, 128, 0]).linear();
        assert_eq!(linear.x, 1.0);
        assert_eq!(linear.z, 0.0);
        // sRGB 128 is about 0.216 in linear light, not 0.5.
        assert!((linear.y - 0.216).abs() < 0.002, "{linear}");
    }

    /// The library refuses what the shader cannot draw: a pack with no maps or no positive repeat,
    /// a flat factor outside `0..=1`, a key declared twice, a value without its provenance.
    #[test]
    fn the_library_refuses_an_entry_it_cannot_draw() {
        let pack = |name: &str, repeat: &str| {
            format!(
                r#""{name}": Maps(maps: "x/y", metres_per_repeat: ({repeat}, Tuned("fixture")), takes_paint: true),"#
            )
        };
        let library = |entries: &str| VisualLibrary::parse(&format!("(visuals: {{ {entries} }})"));
        assert!(library(&pack("steel", "1.0")).is_ok());
        for refused in [
            pack("steel", "0.0"),
            pack("steel", "-1.0"),
            pack("steel", "inf"),
            pack("steel", "1.0") + &pack("steel", "2.0"),
            r#""steel": Maps(maps: " ", metres_per_repeat: (1.0, Tuned("fixture")), takes_paint: true),"#.into(),
            r#""steel": Maps(maps: "x/y", metres_per_repeat: 1.0, takes_paint: true),"#.into(),
            r##""glass": Flat(colour: ("#000000", Tuned("fixture")), roughness: (1.5, Tuned("fixture")), metallic: (0.0, Tuned("fixture")), takes_paint: false),"##.into(),
        ] {
            assert!(library(&refused).is_err(), "{refused}");
        }
    }

    /// A livery carries one to three camouflage colours (the pattern's R, G and B), a sheen in
    /// `0..=1` and a positive repeat; anything else is refused at load rather than drawn wrong.
    #[test]
    fn a_livery_refuses_what_its_pattern_cannot_carry() {
        let colour = r##"("#808080", Tuned("fixture"))"##;
        let livery = |sheen: &str, repeat: &str, count: usize| {
            let colours = vec![colour; count].join(", ");
            Livery::parse(&format!(
                r##"(paint: (colour: ("#9A8952", Tuned("fixture")), sheen: ({sheen}, Tuned("fixture"))),
                    camouflage: Some((pattern: "p", metres_per_repeat: ({repeat}, Tuned("fixture")),
                    colours: [{colours}])))"##
            ))
        };
        assert!(livery("0.1", "4.0", 2).is_ok());
        assert!(livery("0.1", "4.0", 3).is_ok());
        for (sheen, repeat, count) in [
            ("0.1", "4.0", 0),
            ("0.1", "4.0", 4),
            ("1.5", "4.0", 2),
            ("0.1", "0.0", 2),
        ] {
            assert!(
                livery(sheen, repeat, count).is_err(),
                "{sheen} {repeat} {count}"
            );
        }
        let plain = Livery::parse(
            r##"(paint: (colour: ("#9A8952", Tuned("fixture")), sheen: (0.1, Tuned("fixture"))), camouflage: None)"##,
        )
        .expect("a livery without camouflage parses");
        assert_eq!(plain.pattern_path(), None);
    }

    /// The app the dress runs in: the asset stores it writes, the shipped data, no render app.
    fn dress_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<StandardMaterial>()
            .init_asset::<TankSurfaceMaterial>()
            .init_asset::<ShaderBuffer>()
            .init_asset::<WorldAsset>()
            .insert_resource(shipped())
            .init_resource::<SurfaceVehicles>();
        app
    }

    /// EVERY SURFACE READS THROUGH A REPEATING SAMPLER. Bevy binds binding 101's image's own sampler
    /// at 102, the one every map and the pattern read through; with 101 empty it binds its fallback's,
    /// which clamps, and a flat visual's camouflage smears its edge texels past the first repeat. So
    /// a surface whose visual has no maps binds the stand-in at 101, and the stand-in carries the
    /// repeating surface sampler. A mutant leaving 101 empty for a flat visual, or a stand-in that
    /// clamps, fails here.
    #[test]
    fn a_flat_visual_binds_the_stand_in_and_its_repeating_sampler() {
        let mut app = dress_app();
        let world = app.world_mut();
        let stand_in = world.resource::<SurfaceVehicles>().stand_in.clone();
        let image = world
            .resource::<Assets<Image>>()
            .get(&stand_in)
            .expect("the stand-in is in the store");
        assert_eq!(
            image.sampler,
            ImageSampler::Descriptor(surface_sampler(MapTiling::Repeat))
        );
        let flat = Visual::Flat {
            colour: Sourced(Srgb([0x20, 0x20, 0x20])),
            roughness: Sourced(0.5),
            metallic: Sourced(0.0),
            takes_paint: true,
        };
        let livery = Livery::parse(
            r##"(paint: (colour: ("#9A8952", Tuned("fixture")), sheen: (0.1, Tuned("fixture"))),
                camouflage: Some((pattern: "p", metres_per_repeat: (4.0, Tuned("fixture")),
                colours: [("#4B573E", Tuned("fixture"))])))"##,
        )
        .expect("the fixture livery parses");
        let made = surface_material(
            &StandardMaterial::default(),
            &flat,
            None,
            &livery,
            Some(Handle::default()),
            &stand_in,
            Handle::default(),
        );
        assert_ne!(
            made.extension.params.flags & FLAG_CAMO,
            0,
            "camouflage drawn"
        );
        assert_eq!(made.extension.visual_albedo, Some(stand_in));
    }

    /// One vehicle's view as a scene lands it: a root with two nodes, the outer moved, the inner
    /// holding two primitives wearing `worn` (a material naming a visual) and `decor` (one naming
    /// none). The root, the worn primitive and the decor primitive.
    fn spawn_view(
        app: &mut App,
        scene: &Handle<WorldAsset>,
        worn: &Handle<StandardMaterial>,
        decor: &Handle<StandardMaterial>,
    ) -> (Entity, Entity, Entity) {
        let registry = SubstanceRegistry::shipped_static();
        let worn_name = registry
            .keys()
            .into_iter()
            .find(|name| registry.visual(name).is_some())
            .expect("fixture data: a material naming a visual");
        let world = app.world_mut();
        let root = world
            .spawn((crate::tank::Tank, WorldAssetRoot(scene.clone())))
            .id();
        let outer = world
            .spawn((
                Name::new("Outer"),
                Transform::from_xyz(0.0, 1.0, 0.0),
                ChildOf(root),
            ))
            .id();
        let inner = world
            .spawn((
                Name::new("Inner"),
                Transform::from_xyz(0.0, 0.0, 2.0),
                ChildOf(outer),
            ))
            .id();
        let primitive = |world: &mut World, material: &Handle<StandardMaterial>, name: &str| {
            world
                .spawn((
                    Mesh3d(Handle::default()),
                    MeshMaterial3d(material.clone()),
                    GltfMaterialName(name.to_owned()),
                    Transform::IDENTITY,
                    ChildOf(inner),
                ))
                .id()
        };
        let worn = primitive(world, worn, worn_name);
        let decor = primitive(world, decor, "not a material");
        (root, worn, decor)
    }

    /// THE DRESS SWAPS WHAT A MATERIAL NAMES AND NOTHING ELSE, ONCE PER VEHICLE. A primitive whose
    /// material names a visual leaves its standard material for a surface material and is tagged
    /// with its rest row, the node chain's translation; one whose material names none keeps its
    /// own. A second instance of the vehicle shares the first's material and rest table and finds
    /// the same row by node path. A mutant that dressed decor, tagged row 0, rebuilt the table per
    /// instance or minted a material per instance fails here.
    #[test]
    fn the_dress_swaps_worn_primitives_and_shares_one_material_per_vehicle() {
        let mut app = dress_app();
        let scene: Handle<WorldAsset> = app
            .world_mut()
            .resource_mut::<Assets<WorldAsset>>()
            .add(WorldAsset::new(World::new()));
        let (worn_material, decor_material) = {
            let mut standard = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
            (
                standard.add(StandardMaterial::default()),
                standard.add(StandardMaterial::default()),
            )
        };
        let first = spawn_view(&mut app, &scene, &worn_material, &decor_material);
        let second = spawn_view(&mut app, &scene, &worn_material, &decor_material);
        for (root, ..) in [first, second] {
            app.world_mut()
                .run_system_once(move |mut dress: Dress| dress.vehicle(root))
                .expect("the dress runs");
        }

        let world = app.world();
        let surface = |entity: Entity| world.get::<MeshMaterial3d<TankSurfaceMaterial>>(entity);
        for (_, worn, decor) in [first, second] {
            assert!(
                surface(worn).is_some(),
                "the worn primitive wears the surface"
            );
            assert!(
                world
                    .get::<MeshMaterial3d<StandardMaterial>>(worn)
                    .is_none()
            );
            assert_eq!(
                world.get::<MeshTag>(worn),
                Some(&MeshTag(1)),
                "its rest row"
            );
            assert!(surface(decor).is_none(), "decor keeps its own material");
            assert!(world.get::<MeshTag>(decor).is_none());
        }
        assert_eq!(
            surface(first.1).map(|m| m.id()),
            surface(second.1).map(|m| m.id()),
            "one material per vehicle and material"
        );
        assert_eq!(world.resource::<Assets<TankSurfaceMaterial>>().len(), 1);
        assert_eq!(
            world.resource::<Assets<ShaderBuffer>>().len(),
            1,
            "one rest table"
        );
        let vehicles = world.resource::<SurfaceVehicles>();
        let vehicle = &vehicles.vehicles[&scene.id()];
        assert_eq!(vehicle.rows.get("Outer/Inner"), Some(&1));
        let rows = [
            RestRow::IDENTITY,
            RestRow::of(Affine3A::from_translation(Vec3::new(0.0, 1.0, 2.0))),
        ];
        let mut expected = bevy::render::render_resource::encase::StorageBuffer::new(Vec::new());
        expected.write(&rows.to_vec()).expect("the rows encode");
        let table = world
            .resource::<Assets<ShaderBuffer>>()
            .get(&vehicle.rest_frames)
            .expect("the table");
        assert_eq!(
            table.data.as_deref(),
            Some(expected.into_inner().as_slice()),
            "row 0 the identity, row 1 the node chain's translation"
        );
    }

    /// THE REST FRAME STOPS BELOW THE FIRST MOVING NODE. A part the tank file articulates (a
    /// turret, a barrel, a road-wheel station) is posed and animated, so its own transform is never
    /// read: the frame of a primitive under it is the articulated node's own, which is what keeps a
    /// pattern on a turning turret from swimming. A mutant reading through the articulated node, or
    /// stopping early at an unarticulated one, fails here.
    #[test]
    fn the_rest_frame_stops_below_the_first_articulated_node() {
        let mut world = World::new();
        let root = world.spawn(Transform::from_xyz(100.0, 0.0, 0.0)).id();
        let hull = world
            .spawn((
                Name::new("Hull"),
                Transform::from_xyz(0.0, 1.0, 0.0),
                ChildOf(root),
            ))
            .id();
        let turret = world
            .spawn((
                Name::new("Turret"),
                Transform::from_xyz(0.0, 2.0, 0.0).with_rotation(Quat::from_rotation_y(1.0)),
                ChildOf(hull),
            ))
            .id();
        let plate = world
            .spawn((
                Name::new("Plate"),
                Transform::from_xyz(0.0, 0.0, 3.0),
                ChildOf(turret),
            ))
            .id();
        let primitive = world.spawn((Transform::IDENTITY, ChildOf(plate))).id();
        let hull_plate = world
            .spawn((
                Name::new("Glacis"),
                Transform::from_xyz(0.0, 0.0, -2.0),
                ChildOf(hull),
            ))
            .id();
        let hull_primitive = world.spawn((Transform::IDENTITY, ChildOf(hull_plate))).id();
        let rows = world
            .run_system_once(
                move |parents: Query<&ChildOf>,
                      names: Query<&Name>,
                      transforms: Query<&Transform>| {
                    let articulated = |name: &str| name == "Turret";
                    [primitive, hull_primitive].map(|entity| {
                        rest_row(entity, root, &articulated, &parents, &names, &transforms)
                    })
                },
            )
            .expect("the rows resolve");
        assert_eq!(
            rows[0],
            Affine3A::from_translation(Vec3::new(0.0, 0.0, 3.0))
        );
        assert_eq!(
            rows[1],
            Affine3A::from_translation(Vec3::new(0.0, 1.0, -2.0))
        );
    }
}
