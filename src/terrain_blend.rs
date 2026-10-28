//! The ground's surface BLEND: four material packs mixed per fragment by the map's author-painted
//! weight masks, drawn through one [`TerrainMaterial`].
//!
//! VIEW ONLY (ADR-0014). Nothing here touches the height grid, the oracle or the collider — the
//! ONE-SURFACE invariant ([`crate::terrain_grid`]) is about geometry, and this is about what that
//! one surface looks like. The dedicated server never builds the material.
//!
//! # The four packs
//!
//! The BASE pack is [`crate::terrain_grid::TEXTURE_PATH`]'s — the surface everywhere the masks are
//! silent. The three LAYERS are [`BLEND_LAYERS`], and their order is the mask's channel order:
//! R = recesses, G = slopes, B = lowlands (the map manifest's `terrain.masks` block declares the
//! same one). Each layer is a slice of the three 2D-ARRAY textures the encode script cuts
//! ([`LAYER_TEXTURE_PATH`] and its siblings), because texture slots are the binding constraint:
//! nine separate layer maps do not fit beside bevy's PBR bind group, three arrays do.
//!
//! # Per-pack UV scale lives HERE, not on the mesh
//!
//! The mesh bakes ONE UV set, `world_xz / TEXTURE_TILE_M`, cut for the base pack's authored size.
//! Every layer multiplies that by [`BlendLayer::uv_scale`] — its own authored metres are a pack
//! CONTRACT (each pack's `cc.txt`), and mapping a scan onto anything else silently resizes every
//! feature in it. So a pack swap is a constant here, never a mesh regeneration.
//!
//! # The macro tint
//!
//! Every pack repeats at its authored size, and at 8–25 m that grid is visible across a map. The
//! shader multiplies the BLENDED albedo by a low-frequency gain over world XZ
//! ([`MACRO_PERIOD_M`], [`MACRO_AMPLITUDE`]) — one gain for all four packs, after the mix, which
//! is the only point the base pack can be reached at all. It moves brightness, which is what a
//! repeat is read by; the packs, the normals and the ARM are untouched.
//!
//! # Track prints
//!
//! Each pack names its ground type ([`BlendLayer::ground`], [`BASE_GROUND`]), and the ground-type
//! table ([`crate::ground_types`]) says how that ground takes a track print
//! ([`crate::track_prints`], cosmetic terrain marks): its softness and depth cap, and how its albedo
//! and roughness change where a sole lay. The shader blends the four responses by the map's PAINTED
//! weights ([`print_weights`]), not by the AO-biased weights it blends the packs' looks by, so the
//! CPU can reproduce the depth it draws ([`print_depth_m`]): the drawn belt sinks by that depth
//! (`track::view`), and nothing senses the ground per stamp.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::ground_types::{GroundTypes, PrintResponse};
use crate::map::MapManifest;
use crate::terrain_grid::{RowOrder, TEXTURE_TILE_M, TerrainExtent};

/// The ground material: bevy's `StandardMaterial` (which carries the BASE pack's three maps and
/// every lighting parameter) extended with [`TerrainBlend`] (the three layer arrays, the mask, and
/// the blend law's constants).
pub(crate) type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainBlend>;

/// The ground type the BASE pack (`coast_sand_rocks_02`, sand strewn with stones) is: the
/// ground-stamping note's GRAVEL row, a key into [`crate::ground_types::GroundTypes`].
pub(crate) const BASE_GROUND: &str = "gravel";

/// One blended layer's contract: which pack it is, the real-world metres that pack was authored
/// at, and which ground type it is.
pub(crate) struct BlendLayer {
    /// The Poly Haven pack — the folder under `assets/terrain/` whose `cc.txt` carries the
    /// attribution and the authored size below. Names the layer; nothing parses it.
    pub(crate) pack: &'static str,
    /// Metres per texture repeat, as the scan declares (`dimensions`, millimetres). The layer's UV
    /// scale is derived from it and from nothing else.
    pub(crate) authored_m: f32,
    /// The ground type the pack is: a key into [`crate::ground_types::GroundTypes`], which says how
    /// it takes a track print.
    pub(crate) ground: &'static str,
}

impl BlendLayer {
    /// The multiplier this layer applies to the mesh's UV, which is cut at [`TEXTURE_TILE_M`].
    /// Ratio of authored sizes, so both packs render at life size off one UV set.
    pub(crate) fn uv_scale(&self) -> f32 {
        TEXTURE_TILE_M / self.authored_m
    }
}

/// The blended layers, in the mask's channel order — layer `i` is array slice `i` and mask channel
/// `i`. Changing this order re-cuts the arrays (`scripts/encode-terrain-ktx2.sh`'s `LAYER_PACKS`)
/// and re-paints the masks; the two are one contract.
pub(crate) const BLEND_LAYERS: [BlendLayer; 3] = [
    // R — recesses: where dirt and debris settle.
    BlendLayer {
        pack: "dirt_aerial_03",
        authored_m: 25.0,
        ground: "dirt",
    },
    // G — slopes: where land slides, exposing sand and soil.
    BlendLayer {
        pack: "coast_sand_05",
        authored_m: 25.0,
        ground: "sand",
    },
    // B — lowlands: swampy, marshy, moist.
    BlendLayer {
        pack: "aerial_mud_1",
        authored_m: 8.0,
        ground: "mud",
    },
];

/// Every pack's print response in the shader's slot order: the base, then [`BLEND_LAYERS`].
pub(crate) fn print_responses(table: &GroundTypes) -> [PrintResponse; 4] {
    [
        table.get(BASE_GROUND).print,
        table.get(BLEND_LAYERS[0].ground).print,
        table.get(BLEND_LAYERS[1].ground).print,
        table.get(BLEND_LAYERS[2].ground).print,
    ]
}

/// The material's print lanes for `table`, per slot (`x` the base pack, `yzw` the layers):
/// softness (m per kPa), depth cap (m), albedo gain and roughness gain.
pub(crate) fn print_lanes(table: &GroundTypes) -> [Vec4; 4] {
    let responses = print_responses(table);
    let lane = |of: fn(&PrintResponse) -> f32| Vec4::from_array(responses.map(|p| of(&p)));
    [
        lane(|p| p.softness_mm_per_kpa / 1000.0),
        lane(|p| p.depth_cap_mm / 1000.0),
        lane(PrintResponse::albedo_gain),
        lane(|p| p.roughness_gain),
    ]
}

/// Write `table`'s print lanes into `params`.
pub(crate) fn set_print_lanes(params: &mut TerrainBlendParams, table: &GroundTypes) {
    let [softness, cap, albedo, roughness] = print_lanes(table);
    params.print_softness = softness;
    params.print_cap = cap;
    params.print_albedo = albedo;
    params.print_roughness = roughness;
}

/// The print's ground weights from the mask's painted weights `mask` (R recesses, G slopes,
/// B lowlands), in slot order: the base pack where the mask is silent, each layer by its painted
/// weight, normalised. `terrain_blend.wgsl`'s `print_weights` is this law, line for line.
pub(crate) fn print_weights(mask: Vec3) -> Vec4 {
    let base = 1.0 - (mask.x + mask.y + mask.z).clamp(0.0, 1.0);
    let weights = Vec4::new(base, mask.x, mask.y, mask.z).max(Vec4::ZERO);
    weights / weights.element_sum().max(1e-6)
}

/// The print depth (m) a full press reaches under a ground pressure of `kpa` (kPa), on ground of
/// print weights `weights` ([`print_weights`]), from the material's own lanes: each slot's
/// `softness × pressure`, capped, blended. `terrain_blend.wgsl`'s `print_depth` is this law.
pub(crate) fn print_depth_m(params: &TerrainBlendParams, weights: Vec4, kpa: f32) -> f32 {
    weights.dot(params.print_cap.min(params.print_softness * kpa))
}

/// Albedo of every layer, as one KTX2 2D array (UASTC 4x4 + zstd, full mip chain, 2048² per
/// slice) — slice order is [`BLEND_LAYERS`]. Cut by `scripts/encode-terrain-ktx2.sh blend`; the
/// attribution for each slice is its pack's `cc.txt`.
pub(crate) const LAYER_TEXTURE_PATH: &str = "terrain/blend/blend_diff.ktx2";
/// OpenGL-convention tangent-space normals of every layer — see [`LAYER_TEXTURE_PATH`].
pub(crate) const LAYER_NORMAL_PATH: &str = "terrain/blend/blend_nor_gl.ktx2";
/// glTF-ORM `arm` pack of every layer (R = AO, G = roughness, B = metallic) — see
/// [`LAYER_TEXTURE_PATH`].
pub(crate) const LAYER_ARM_PATH: &str = "terrain/blend/blend_arm.ktx2";

/// How far a layer's own ambient occlusion may push its weight, as a fraction of that weight. The
/// bias is MULTIPLICATIVE (`w · (1 + this · ao)`), so a zero weight stays zero however bright the
/// layer's AO is — which is what makes [`SKIP_WEIGHT`] exact.
const AO_BIAS: f32 = 0.5;

/// The window, in biased-weight units, below the winning weight that still contributes: a layer
/// `BLEND_WINDOW` behind the leader reaches zero, one level with it keeps its full weight. This is
/// the transition width, and the only reason a 1024² mask can read as a sharp boundary — the
/// crossing point moves with the AO bias above, so the edge follows the material's own detail
/// instead of the mask's texel grid.
const BLEND_WINDOW: f32 = 0.4;

/// Mask weight at or below which a layer is not sampled at all. HALF A MASK QUANTISATION STEP
/// (the mask is 8-bit, so a stored weight is a multiple of 1/255): the skip therefore fires
/// exactly on the weights the mask stores as zero and on no other, and the ~6 texture reads it
/// leaves are not an approximation of the 13 it avoids — they are the same result.
const SKIP_WEIGHT: f32 = 0.5 / 255.0;

/// World metres per period of the macro tint. Well above every pack's authored size
/// ([`BLEND_LAYERS`], 8–25 m), so the tint varies across a repeat instead of with it.
const MACRO_PERIOD_M: f32 = 60.0;

/// How far the macro tint may push the blended albedo, as a fraction of it: the ground ranges
/// between `1 - this` and `1 + this` of its packs' own colour. Strictly below 1, which is what
/// makes the gain positive at every field value.
const MACRO_AMPLITUDE: f32 = 0.12;

/// The gain the shader applies is `1 ± MACRO_AMPLITUDE`, so an amplitude at or above 1 would zero
/// or invert the ground's albedo. Checked where it cannot be edited past — at compile time.
const _: () = assert!(0.0 < MACRO_AMPLITUDE && MACRO_AMPLITUDE < 1.0);

/// The tint must vary ACROSS a repeat rather than beat against it, so its period clears twice the
/// coarsest pack ([`TEXTURE_TILE_M`] is the base pack's, [`BLEND_LAYERS`] the rest).
const _: () = assert!(MACRO_PERIOD_M > 2.0 * TEXTURE_TILE_M);

/// The uniform block `terrain_blend.wgsl` reads — the whole CPU→shader contract, and the single
/// home of every number the blend law uses.
#[derive(ShaderType, Reflect, Debug, Clone, Default)]
pub(crate) struct TerrainBlendParams {
    /// `xyz`: each layer's [`BlendLayer::uv_scale`] over the mesh's own UV. `w`: the mesh's UV per
    /// world metre (`1 / `[`TEXTURE_TILE_M`]), which moves the UV with a parallax offset.
    pub layer_uv_scale: Vec4,
    /// World XZ to mask UV: `uv = world.xz * xy + zw`. Carries the map's extent AND the row order
    /// its mask image was exported in (a `-Z` image has a negative `y`), so the mask lands on the
    /// terrain the author painted it for.
    pub mask_map: Vec4,
    /// `x`: [`AO_BIAS`], `y`: [`BLEND_WINDOW`], `z`: [`SKIP_WEIGHT`], `w`: unused.
    pub blend_law: Vec4,
    /// `x`: periods per metre (`1 / `[`MACRO_PERIOD_M`]), `y`: [`MACRO_AMPLITUDE`], `zw`: unused.
    /// World-space, so it rides no UV set and no pack's authored size.
    pub macro_tint: Vec4,
    /// Per slot (`x` the base pack, `yzw` the layers), from the ground type the pack names: print
    /// depth per kPa of ground pressure, m ([`PrintResponse::softness_mm_per_kpa`]).
    pub print_softness: Vec4,
    /// Per slot: the deepest print, m ([`PrintResponse::depth_cap_mm`]).
    pub print_cap: Vec4,
    /// Per slot: the albedo multiplier where a sole lay ([`PrintResponse::albedo_gain`]).
    pub print_albedo: Vec4,
    /// Per slot: [`PrintResponse::roughness_gain`].
    pub print_roughness: Vec4,
    /// The prints' near store: `xy` its window's low corner in world XZ (m), `z` its side (m), `w`
    /// its texel (m) — [`crate::track_prints::near_params`], rewritten when the window moves.
    pub print_near: Vec4,
    /// World XZ to the prints' world-layer UV: `uv = world.xz * xy + zw`.
    pub print_world: Vec4,
    /// `x` the ground pressure (kPa) a full G channel of the near store stands for, `y` its window
    /// fade (m), `zw` the pixels per texel at which its normal is gone and whole
    /// ([`crate::track_prints::law_params`]).
    pub print_law: Vec4,
    /// How the prints' relief is drawn ([`crate::track_prints::relief_params`]): `x` the level
    /// (0 lighting only, 1 parallax, 2 parallax with the sun's shadow), `y` the distance (m) relief
    /// reaches, `z` the distance over which it fades to lighting only (m).
    pub print_relief: Vec4,
}

/// The terrain blend's half of [`TerrainMaterial`]: the three layer arrays, the map's weight mask,
/// the track prints' two stores, and [`TerrainBlendParams`]. The base pack's maps ride the
/// `StandardMaterial` half.
///
/// Bindings start at 100 — bevy's convention for a material extension, keeping them clear of the
/// `StandardMaterial` slots in the same bind group.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct TerrainBlend {
    #[uniform(100)]
    pub params: TerrainBlendParams,
    #[texture(101, dimension = "2d_array")]
    #[sampler(102)]
    pub layer_albedo: Handle<Image>,
    #[texture(103, dimension = "2d_array")]
    #[sampler(104)]
    pub layer_normal: Handle<Image>,
    #[texture(105, dimension = "2d_array")]
    #[sampler(106)]
    pub layer_arm: Handle<Image>,
    /// The map's own weight masks, STRETCHED once over its world square — R = recesses,
    /// G = slopes, B = lowlands, linear weights that do not sum to 1.
    #[texture(107)]
    #[sampler(108)]
    pub masks: Handle<Image>,
    /// The track prints' near store ([`crate::track_prints`]): R the press, G the footprint, in a
    /// window that wraps. NO SAMPLER OF ITS OWN: the shader reads it through the layer albedo's
    /// repeating sampler, because every sampler the material declares counts against the 16 a
    /// stage that Metal and DX12 resource-binding tier 1 allow.
    #[texture(109)]
    pub prints_near: Handle<Image>,
    /// The track prints' world layer: trodden coverage over the map's square, mipmapped. Read
    /// through the mask's clamping sampler, for the same reason.
    #[texture(110)]
    pub prints_world: Handle<Image>,
}

impl MaterialExtension for TerrainBlend {
    fn fragment_shader() -> ShaderRef {
        "shaders/terrain_blend.wgsl".into()
    }
}

/// World XZ to mask UV for a map, as [`TerrainBlendParams::mask_map`] packs it. The mask shares
/// the heightmap's world extent and row order; unlike the heightmap it is never row-reversed at
/// load, so the row order is folded in HERE instead.
pub(crate) fn mask_map(extent: TerrainExtent, rows: RowOrder) -> Vec4 {
    let per_metre = 1.0 / extent.world_size_m;
    let toward_z = match rows {
        RowOrder::TowardPositiveZ => per_metre,
        RowOrder::TowardNegativeZ => -per_metre,
    };
    Vec4::new(per_metre, toward_z, 0.5, 0.5)
}

/// The material's parameters for the map being drawn: per-layer UV scales from the pack contracts,
/// the mask mapping from the manifest, the blend law's constants, and each pack's ground type's
/// print response from `ground`, with the prints' stores as they start.
pub(crate) fn params(manifest: &MapManifest, ground: &GroundTypes) -> TerrainBlendParams {
    let mut params = TerrainBlendParams {
        layer_uv_scale: Vec4::new(
            BLEND_LAYERS[0].uv_scale(),
            BLEND_LAYERS[1].uv_scale(),
            BLEND_LAYERS[2].uv_scale(),
            1.0 / TEXTURE_TILE_M,
        ),
        mask_map: mask_map(manifest.extent, manifest.rows),
        blend_law: Vec4::new(AO_BIAS, BLEND_WINDOW, SKIP_WEIGHT, 0.0),
        macro_tint: Vec4::new(1.0 / MACRO_PERIOD_M, MACRO_AMPLITUDE, 0.0, 0.0),
        print_near: crate::track_prints::near_params(Vec2::ZERO),
        print_world: crate::track_prints::world_params(manifest.extent.world_size_m),
        print_law: crate::track_prints::law_params(),
        print_relief: crate::track_prints::relief_params(Default::default(), Default::default()),
        ..default()
    };
    set_print_lanes(&mut params, ground);
    params
}

/// The layer roster for the boot log: which pack each array slice is, the metres its contract
/// pins, and the UV scale that follows. The one runtime statement of what the ground is made of.
pub(crate) fn layer_census() -> String {
    BLEND_LAYERS
        .iter()
        .enumerate()
        .map(|(slice, layer)| {
            format!(
                "{slice}={} every {} m (uv x{:.3})",
                layer.pack,
                layer.authored_m,
                layer.uv_scale(),
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(MaterialPlugin::<TerrainMaterial>::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE BINDING THIS SLICE STANDS ON. The three layer maps ship as KTX2 2D ARRAYS, one slice
    /// per pack, because nine separate bindings do not fit beside bevy's PBR bind group. This runs
    /// the shipped bytes through bevy's OWN loader and pins what the material declares: three
    /// slices, 2048² each, a full mip chain, and a `D2Array` texture view — which is what makes
    /// `#[texture(_, dimension = "2d_array")]` legal. A pack re-cut without `-tex_type 2darray`
    /// loads as a plain 2D texture and the bind group would fail at pipeline creation, on a
    /// machine with a window, at run time.
    ///
    /// Pinned against `CompressedImageFormats::BC` for the same reason the base pack's test is:
    /// the transcode target follows the caller's flags, not the test machine's GPU.
    #[test]
    fn the_shipped_layer_maps_are_three_slice_mipmapped_ktx2_arrays() {
        use bevy::image::{CompressedImageFormats, ktx2_buffer_to_image};
        use bevy::render::render_resource::TextureViewDimension;
        // 2048² ⇒ 12 levels down to 1×1. A map that lost its chain thrashes the texture cache at
        // every grazing angle.
        const FULL_MIP_CHAIN: u32 = 12;
        for (path, is_srgb, what) in [
            (LAYER_TEXTURE_PATH, true, "albedo"),
            (LAYER_NORMAL_PATH, false, "normal"),
            (LAYER_ARM_PATH, false, "arm"),
        ] {
            let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(path);
            let bytes = std::fs::read(&full)
                .unwrap_or_else(|err| panic!("layer map missing at {}: {err}", full.display()));
            assert!(
                bytes.len() > 4096,
                "{path} is {} bytes — a Git LFS POINTER, not the map",
                bytes.len(),
            );
            let image = ktx2_buffer_to_image(&bytes, CompressedImageFormats::BC, is_srgb)
                .unwrap_or_else(|err| panic!("{path} failed to transcode: {err:?}"));
            let size = image.texture_descriptor.size;
            assert_eq!(
                (size.width, size.height),
                (2048, 2048),
                "every slice of the {what} array is 2k",
            );
            assert_eq!(
                size.depth_or_array_layers,
                BLEND_LAYERS.len() as u32,
                "the {what} array carries one slice per blended layer",
            );
            assert_eq!(
                image.texture_descriptor.mip_level_count, FULL_MIP_CHAIN,
                "the {what} array carries {} mip levels, not a full chain",
                image.texture_descriptor.mip_level_count,
            );
            assert_eq!(
                image
                    .texture_view_descriptor
                    .as_ref()
                    .and_then(|view| view.dimension),
                Some(TextureViewDimension::D2Array),
                "the {what} array must present as texture_2d_array, or the material's \
                 dimension = \"2d_array\" binding cannot be satisfied",
            );
        }
    }

    /// EACH PACK NAMES A GROUND TYPE THE TABLE HAS, and its print lanes are that type's: the base
    /// pack in slot `x`, the layers in mask-channel order. A pack naming a missing type aborts the
    /// ground's build at load, and a lane read from the wrong slot would print one pack's depth
    /// under another's look.
    #[test]
    fn every_pack_takes_its_named_ground_types_print() {
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        let [softness, cap, albedo, roughness] = print_lanes(&table);
        let names = [
            BASE_GROUND,
            BLEND_LAYERS[0].ground,
            BLEND_LAYERS[1].ground,
            BLEND_LAYERS[2].ground,
        ];
        for (slot, name) in names.into_iter().enumerate() {
            let print = table.get(name).print;
            assert_eq!(softness[slot], print.softness_mm_per_kpa / 1000.0, "{name}");
            assert_eq!(cap[slot], print.depth_cap_mm / 1000.0, "{name}");
            assert_eq!(albedo[slot], print.albedo_gain(), "{name}");
            assert_eq!(roughness[slot], print.roughness_gain, "{name}");
        }
    }

    /// THE PRINT'S GROUND WEIGHTS ARE A PARTITION OF THE PAINTED MASK: they sum to one, a silent
    /// mask is all base pack, a fully painted channel is all its layer, and an over-painted texel
    /// shares between its layers. A sum other than one would scale every depth the belt sinks by.
    #[test]
    fn the_print_weights_partition_the_painted_mask() {
        for mask in [
            Vec3::ZERO,
            Vec3::new(0.2, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.3, 0.9, 0.6),
        ] {
            let weights = print_weights(mask);
            assert!(
                (weights.element_sum() - 1.0).abs() < 1e-6,
                "{mask}: {weights}"
            );
            assert!(weights.min_element() >= 0.0, "{mask}: {weights}");
        }
        assert_eq!(print_weights(Vec3::ZERO), Vec4::X, "silent: the base pack");
        assert_eq!(print_weights(Vec3::Y), Vec4::Z, "the slopes' layer alone");
    }

    /// The mask is stretched ONCE over the map's square, and its rows run the way the manifest
    /// declares. Both corners of both row orders, so a sign flip cannot pass.
    #[test]
    fn the_mask_mapping_puts_the_worlds_corners_on_the_images_corners() {
        let extent = TerrainExtent {
            world_size_m: 1500.0,
            height_offset_m: 0.0,
            height_span_m: 50.0,
        };
        let half = extent.half_extent();
        for (rows, first_row_z) in [
            (RowOrder::TowardPositiveZ, -half),
            (RowOrder::TowardNegativeZ, half),
        ] {
            let map = mask_map(extent, rows);
            let uv = |x: f32, z: f32| Vec2::new(x * map.x + map.z, z * map.y + map.w);
            assert_eq!(
                uv(-half, first_row_z),
                Vec2::ZERO,
                "{rows:?}: the image's first texel is the -X corner of its first row",
            );
            assert_eq!(
                uv(half, -first_row_z),
                Vec2::ONE,
                "{rows:?}: the image's last texel is the +X corner of its last row",
            );
            assert_eq!(uv(0.0, 0.0), Vec2::splat(0.5), "{rows:?}: the world centre");
        }
    }
}
