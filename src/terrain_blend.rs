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

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::map::MapManifest;
use crate::terrain_grid::{RowOrder, TEXTURE_TILE_M, TerrainExtent};

/// The ground material: bevy's `StandardMaterial` (which carries the BASE pack's three maps and
/// every lighting parameter) extended with [`TerrainBlend`] (the three layer arrays, the mask, and
/// the blend law's constants).
pub(crate) type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainBlend>;

/// One blended layer's contract: which pack it is, and the real-world metres that pack was
/// authored at.
pub(crate) struct BlendLayer {
    /// The Poly Haven pack — the folder under `assets/terrain/` whose `cc.txt` carries the
    /// attribution and the authored size below. Names the layer; nothing parses it.
    pub(crate) pack: &'static str,
    /// Metres per texture repeat, as the scan declares (`dimensions`, millimetres). The layer's UV
    /// scale is derived from it and from nothing else.
    pub(crate) authored_m: f32,
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
    },
    // G — slopes: where land slides, exposing sand and soil.
    BlendLayer {
        pack: "coast_sand_05",
        authored_m: 25.0,
    },
    // B — lowlands: swampy, marshy, moist.
    BlendLayer {
        pack: "aerial_mud_1",
        authored_m: 8.0,
    },
];

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

/// THE BLEND LAW, as arithmetic: mask weights and per-pack AO in, the four normalized weights that
/// mix albedo, normal and ARM out. Index 0 is the base pack, `i + 1` is [`BLEND_LAYERS`]`[i]`.
///
/// * the three layers keep their painted weights and the base takes what is left,
///   `1 - saturate(r + g + b)` — the masks are INDEPENDENT and do not sum to 1;
/// * each weight is biased by its own pack's AO ([`AO_BIAS`]) and then cut against the leader
///   ([`BLEND_WINDOW`]);
/// * the four are normalized, so they sum to 1 at every mask value.
///
/// MIRRORED in `assets/shaders/terrain_blend.wgsl`, which is where the law actually runs — per
/// fragment, skipping the layers [`SKIP_WEIGHT`] rules out. This copy exists so the law has an
/// executable specification (the tests below); no shipping code path calls it.
#[cfg(test)]
pub(crate) fn blend_weights(mask: [f32; 3], ao: [f32; 4]) -> [f32; 4] {
    let covered = (mask[0] + mask[1] + mask[2]).clamp(0.0, 1.0);
    let raw = [1.0 - covered, mask[0], mask[1], mask[2]];
    let mut biased = [0.0f32; 4];
    let mut leader = 0.0f32;
    for i in 0..4 {
        biased[i] = raw[i] * (1.0 + AO_BIAS * ao[i]);
        leader = leader.max(biased[i]);
    }
    let mut weights = [0.0f32; 4];
    let mut total = 0.0f32;
    for i in 0..4 {
        weights[i] = biased[i] * (1.0 - (leader - biased[i]) / BLEND_WINDOW).max(0.0);
        total += weights[i];
    }
    // The leader keeps its whole biased weight, and the raw weights always sum to at least 1, so
    // the total is bounded below by 0.25 — there is no zero to divide by.
    for weight in &mut weights {
        *weight /= total;
    }
    weights
}

/// THE MACRO TINT'S ENVELOPE, as arithmetic: a two-octave value-noise field in `0..1.5` in, the
/// gain the shader multiplies the blended albedo by out.
///
/// MIRRORED in `assets/shaders/terrain_blend.wgsl`. Only the ENVELOPE is mirrored, not the hash —
/// `sin`-based hashing does not agree bit-for-bit between a CPU and a GPU, and the properties that
/// matter (a strictly positive gain, bounded by the amplitude, centred on 1) hold for any field in
/// range. This copy exists so those properties have an executable specification; no shipping code
/// path calls it.
#[cfg(test)]
pub(crate) fn macro_gain(field: f32) -> f32 {
    const MACRO_PEAK: f32 = 1.5;
    1.0 + MACRO_AMPLITUDE * (field * (2.0 / MACRO_PEAK) - 1.0)
}

/// The uniform block `terrain_blend.wgsl` reads — the whole CPU→shader contract, and the single
/// home of every number the blend law uses.
#[derive(ShaderType, Reflect, Debug, Clone, Default)]
pub(crate) struct TerrainBlendParams {
    /// `xyz`: each layer's [`BlendLayer::uv_scale`] over the mesh's own UV. `w`: unused.
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
}

/// The terrain blend's half of [`TerrainMaterial`]: the three layer arrays, the map's weight mask,
/// and [`TerrainBlendParams`]. The base pack's maps ride the `StandardMaterial` half.
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
/// the mask mapping from the manifest, the blend law's constants.
pub(crate) fn params(manifest: &MapManifest) -> TerrainBlendParams {
    TerrainBlendParams {
        layer_uv_scale: Vec4::new(
            BLEND_LAYERS[0].uv_scale(),
            BLEND_LAYERS[1].uv_scale(),
            BLEND_LAYERS[2].uv_scale(),
            0.0,
        ),
        mask_map: mask_map(manifest.extent, manifest.rows),
        blend_law: Vec4::new(AO_BIAS, BLEND_WINDOW, SKIP_WEIGHT, 0.0),
        macro_tint: Vec4::new(1.0 / MACRO_PERIOD_M, MACRO_AMPLITUDE, 0.0, 0.0),
    }
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

    /// Every distinct 8-bit mask value on every channel, plus the corners the mask can reach that
    /// a coarse sweep would step over.
    fn mask_sweep() -> Vec<[f32; 3]> {
        let steps: Vec<f32> = (0..=255).map(|v| v as f32 / 255.0).collect();
        let mut sweep = Vec::new();
        for &r in &steps {
            for &g in [0.0, 0.25, 0.5, 1.0].iter() {
                for &b in [0.0, 0.5, 1.0].iter() {
                    sweep.push([r, g, b]);
                }
            }
        }
        sweep.push([0.0, 0.0, 0.0]);
        sweep.push([1.0, 1.0, 1.0]);
        sweep
    }

    /// The one property every consumer of the law depends on: albedo, normal and ARM are mixed by
    /// these four numbers, so anything but a partition of unity brightens or darkens the ground as
    /// a function of the mask. Swept across the whole 8-bit range including the all-zero corner
    /// (base only) and the all-saturated one (no base at all).
    #[test]
    fn the_four_weights_sum_to_one_at_every_mask_value() {
        for mask in mask_sweep() {
            for ao in [[0.0; 4], [1.0; 4], [0.15, 0.9, 0.4, 0.7], [0.5; 4]] {
                let weights = blend_weights(mask, ao);
                let total: f32 = weights.iter().sum();
                assert!(
                    (total - 1.0).abs() < 1e-5,
                    "mask {mask:?} ao {ao:?} weighs {weights:?}, summing to {total}",
                );
                assert!(
                    weights.iter().all(|w| (0.0..=1.0).contains(w)),
                    "mask {mask:?} ao {ao:?} weighs {weights:?} — outside 0..1",
                );
            }
        }
    }

    /// THE SKIP'S LICENCE. The shader does not sample a layer whose mask weight is at or below
    /// `SKIP_WEIGHT`, and the mask is 8-bit, so the layers it drops are exactly the ones stored as
    /// zero. A zero weight must therefore come out zero for EVERY AO the skipped sample could have
    /// returned — otherwise the skip would be an approximation instead of an identity.
    #[test]
    fn a_zero_mask_weight_stays_zero_whatever_its_ao_would_have_been() {
        for channel in 0..3 {
            for other in [0.0f32, 0.05, 0.4, 1.0] {
                let mut mask = [other; 3];
                mask[channel] = 0.0;
                for ao_of_skipped in [0.0f32, 0.25, 0.5, 1.0] {
                    let mut ao = [0.3f32; 4];
                    ao[channel + 1] = ao_of_skipped;
                    let weights = blend_weights(mask, ao);
                    assert_eq!(
                        weights[channel + 1],
                        0.0,
                        "mask {mask:?} paints channel {channel} zero, ao {ao:?} weighs \
                         {weights:?}",
                    );
                }
            }
        }
    }

    /// A mask value below `SKIP_WEIGHT` cannot exist: it is half of the smallest step an 8-bit
    /// mask can store, so the threshold separates stored-zero from every stored non-zero.
    #[test]
    fn the_skip_threshold_sits_between_zero_and_the_masks_smallest_step() {
        let step = 1.0 / 255.0;
        assert!(
            0.0 < SKIP_WEIGHT && SKIP_WEIGHT < step,
            "SKIP_WEIGHT {SKIP_WEIGHT} must fall strictly between 0 and one mask step {step}",
        );
    }

    /// THE TINT MULTIPLIES ALBEDO, so a gain that reached zero would paint black ground and a
    /// negative one would invert it. Swept across the field's whole range including both ends: the
    /// gain stays strictly positive and inside the amplitude it is authored with.
    #[test]
    fn the_macro_gain_is_positive_and_bounded_by_its_amplitude() {
        for step in 0..=1500 {
            let field = step as f32 / 1000.0;
            let gain = macro_gain(field);
            assert!(
                gain > 0.0,
                "field {field} gains {gain} — albedo may not be zeroed or inverted",
            );
            assert!(
                (1.0 - MACRO_AMPLITUDE..=1.0 + MACRO_AMPLITUDE).contains(&gain),
                "field {field} gains {gain}, outside 1 ± {MACRO_AMPLITUDE}",
            );
        }
    }

    /// The tint VARIES the ground, it does not lighten or darken it: the field's midpoint gains
    /// exactly 1, and the two ends are the same distance either side. A gain that was not centred
    /// would shift the whole map's albedo away from the packs the author picked.
    #[test]
    fn the_macro_gain_is_centred_on_the_packs_own_colour() {
        assert_eq!(macro_gain(0.75), 1.0, "the field's midpoint must not tint");
        let (dark, light) = (macro_gain(0.0), macro_gain(1.5));
        assert!(
            ((1.0 - dark) - (light - 1.0)).abs() < 1e-6,
            "the field's ends gain {dark} and {light} — not symmetric about 1",
        );
    }

    /// The tint's period must sit clear of every LAYER's authored size too — the compile-time
    /// check above only covers the base pack, and a pack swap moves these.
    #[test]
    fn the_macro_period_is_longer_than_every_packs_repeat() {
        for layer in &BLEND_LAYERS {
            assert!(
                MACRO_PERIOD_M > 2.0 * layer.authored_m,
                "{} repeats every {} m, which the {MACRO_PERIOD_M} m tint must span",
                layer.pack,
                layer.authored_m,
            );
        }
    }

    /// Each layer renders at LIFE SIZE off the one UV set the mesh bakes: the mesh's UV repeats
    /// every `TEXTURE_TILE_M`, so a pack authored at `authored_m` needs exactly that ratio. The
    /// authored metres are each pack's `cc.txt` contract, restated here as the numbers the shader
    /// is handed.
    #[test]
    fn every_layers_uv_scale_is_the_ratio_of_authored_sizes() {
        for (index, (pack, authored_m)) in [
            ("dirt_aerial_03", 25.0f32),
            ("coast_sand_05", 25.0),
            ("aerial_mud_1", 8.0),
        ]
        .into_iter()
        .enumerate()
        {
            let layer = &BLEND_LAYERS[index];
            assert_eq!(
                layer.pack, pack,
                "layer {index} is the mask's channel {index}"
            );
            assert_eq!(layer.authored_m, authored_m, "{pack} authored metres");
            assert_eq!(
                layer.uv_scale(),
                TEXTURE_TILE_M / authored_m,
                "{pack} must repeat every {authored_m} m off a {TEXTURE_TILE_M} m UV",
            );
        }
    }

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
