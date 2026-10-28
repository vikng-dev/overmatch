//! The marks' maps, generated at startup: no art, one analytic profile per kind.
//!
//! A profile answers, for a point `p` of the decal's square (`[-1, 1]²`, +x the shot's direction
//! across the face, +y the texture's v), what the mark is there: an albedo with its alpha, metallic,
//! roughness and a height below the plate (all heights and distances in the square's units). Every
//! layer comes from the distance to one shape, perturbed by value noise so no edge looks drawn:
//!
//! - **hole** (a circle): a near-black core, a lip whose height falls toward the core (metal
//!   pushed in), a bare-steel ring whose outer edge the noise tears, and bare chips past it;
//! - **dent** (a bowl, no core): the bowl's height and its raised rim shade it, a bare spot at the
//!   centre, crazed paint in a ring around it;
//! - **scratch** (a capsule along +x, its radius tapering toward +x): a bare-steel trough streaked
//!   along the gouge, a pushed-up paint lip at its edge, chips beside it.
//!
//! From the profile come four maps: base colour (sRGB, straight alpha), a tangent-space normal map
//! from the height's gradient, metallic-roughness (G roughness, B metallic), and the depth map
//! parallax reads (white the deepest). Each carries a full box-filtered mip chain, so a distant
//! mark does not shimmer. [`VARIANTS`] noise seeds per kind.
//!
//! The tangent-space normal's y follows the mesh bitangent, which mikktspace points along +v for
//! the decal's quad, so a height rising toward +v tilts it to −y: `n ∝ (−∂h/∂x, −∂h/∂y, 1)`.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler, ImageSamplerDescriptor};
use bevy::math::{Vec2, Vec3};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::hull_marks::MarkKind;

/// Texels on a side of every map. ASSUMED: a look to tune; at 256 a mark keeps a texel per screen
/// pixel down to about a metre's viewing distance for an 8.8 cm hole (DERIVED: the hole's quad is
/// 0.29 m, so 256 texels span it at ~880 px per metre).
pub(crate) const SIZE: usize = 256;

/// Noise seeds per kind.
pub(crate) const VARIANTS: usize = 3;

/// One generated set.
pub(crate) struct MarkMaps {
    pub(crate) color: Image,
    pub(crate) normal: Image,
    pub(crate) metal_rough: Image,
    pub(crate) depth: Image,
}

/// The core's radius in the square: a hole's rim, the round's calibre across. The quad is then
/// `1 / HOLE_CORE` calibres on a side.
const HOLE_CORE: f32 = 0.30;
/// A dent's bowl radius in the square, as [`HOLE_CORE`].
const DENT_BOWL: f32 = 0.36;
/// A scratch's half-width at its widest, as [`HOLE_CORE`].
const SCRATCH_WIDTH: f32 = 0.32;

/// The deepest each profile goes, in the square's units: what white means in its depth map.
const HOLE_DEPTH: f32 = 0.12;
const DENT_DEPTH: f32 = 0.05;
const SCRATCH_DEPTH: f32 = 0.035;

/// Fresh-cut steel, linear albedo. ASSUMED: neutral, a touch cool (War Thunder's cutting grey is
/// 0.4, per the research note).
const STEEL: Vec3 = Vec3::new(0.50, 0.51, 0.53);
/// The hole's core, near-black.
const CORE: Vec3 = Vec3::new(0.012, 0.011, 0.010);
/// A paint edge or crack in shadow.
const PAINT_EDGE: Vec3 = Vec3::new(0.035, 0.034, 0.030);

/// The decal's footprint per kind, in calibres: x along the shot (before the stretch), y across it.
pub(crate) fn footprint(kind: MarkKind) -> Vec2 {
    let across = match kind {
        MarkKind::Hole => 1.0 / HOLE_CORE,
        MarkKind::Dent => 1.0 / DENT_BOWL,
        MarkKind::Scratch => 1.0 / SCRATCH_WIDTH,
    };
    Vec2::splat(across)
}

/// The parallax depth scale for a kind: its deepest point relative to the texture's size, which
/// spans two of the square's units.
pub(crate) fn parallax_depth(kind: MarkKind) -> f32 {
    deepest(kind) / 2.0
}

fn deepest(kind: MarkKind) -> f32 {
    match kind {
        MarkKind::Hole => HOLE_DEPTH,
        MarkKind::Dent => DENT_DEPTH,
        MarkKind::Scratch => SCRATCH_DEPTH,
    }
}

/// What a profile says at one point.
#[derive(Clone, Copy)]
struct Texel {
    albedo: Vec3,
    alpha: f32,
    metallic: f32,
    roughness: f32,
    /// Height above the plate; below it is negative.
    height: f32,
}

impl Texel {
    const CLEAR: Self = Self {
        albedo: Vec3::ZERO,
        alpha: 0.0,
        metallic: 0.0,
        roughness: 0.8,
        height: 0.0,
    };

    fn steel(grain: f32, alpha: f32) -> Self {
        Self {
            albedo: STEEL * (0.88 + 0.3 * (grain - 0.5)),
            alpha,
            metallic: 0.85,
            roughness: 0.36 + 0.16 * (grain - 0.5),
            height: 0.004 * (grain - 0.5),
        }
    }

    fn paint_edge(alpha: f32, height: f32) -> Self {
        Self {
            albedo: PAINT_EDGE,
            alpha,
            metallic: 0.0,
            roughness: 0.8,
            height,
        }
    }
}

/// Generate one kind's maps for one noise seed.
pub(crate) fn generate(kind: MarkKind, variant: usize) -> MarkMaps {
    let seed = 0x5EED_0000 ^ ((kind as u32) << 8) ^ variant as u32;
    let profile: fn(Vec2, u32) -> Texel = match kind {
        MarkKind::Hole => hole,
        MarkKind::Dent => dent,
        MarkKind::Scratch => scratch,
    };
    let texels: Vec<Texel> = (0..SIZE * SIZE)
        .map(|index| profile(square_point(index % SIZE, index / SIZE), seed))
        .collect();
    encode(&texels, deepest(kind))
}

/// The centre of texel (`column`, `row`) in the square.
fn square_point(column: usize, row: usize) -> Vec2 {
    Vec2::new(column as f32 + 0.5, row as f32 + 0.5) / SIZE as f32 * 2.0 - Vec2::ONE
}

fn hole(p: Vec2, seed: u32) -> Texel {
    let r = p.length();
    let core = HOLE_CORE * (1.0 + 0.10 * (fbm(p * 6.0, seed) - 0.5));
    let d = r - core;
    let lip = 0.085;
    let ring = HOLE_CORE * (1.45 + 0.55 * fbm(p * 4.0 + Vec2::splat(17.0), seed ^ 0xA1));
    let grain = fbm(p * 22.0, seed ^ 0x5C);
    if d < 0.0 {
        // The bore's wall catches a little light just inside the rim.
        let wall = smoothstep(-0.05, 0.0, d);
        return Texel {
            albedo: CORE.lerp(STEEL * 0.12, wall * 0.5),
            alpha: 1.0,
            metallic: 0.3,
            roughness: 0.85,
            height: -HOLE_DEPTH,
        };
    }
    if d < lip {
        let t = d / lip;
        return Texel {
            // Darkened by the passage at the rim, bright steel where the ring begins.
            albedo: (STEEL * 0.5).lerp(STEEL, t) * (0.92 + 0.16 * (grain - 0.5)),
            alpha: 1.0,
            metallic: 0.9,
            roughness: 0.32 + 0.1 * grain,
            height: -0.045 * (1.0 - t) * (1.0 - t),
        };
    }
    if r < ring {
        return Texel::steel(grain, 1.0 - smoothstep(ring - 0.012, ring, r));
    }
    chipped_paint(p, r - ring, seed)
}

fn dent(p: Vec2, seed: u32) -> Texel {
    let rho = p.length() / DENT_BOWL * (1.0 + 0.12 * (fbm(p * 5.0, seed) - 0.5));
    let bowl = if rho < 1.0 {
        let s = 1.0 - rho * rho;
        -DENT_DEPTH * s * s
    } else {
        0.0
    };
    let rim = 0.010 * (-((rho - 1.06) / 0.10).powi(2)).exp();
    let height = bowl + rim;
    let grain = fbm(p * 22.0, seed ^ 0x5C);
    let bare = 0.38 + 0.25 * (2.0 * fbm(p * 7.0 + Vec2::splat(3.0), seed ^ 0x11) - 1.0);
    if rho < bare {
        let mut texel = Texel::steel(grain, 1.0);
        texel.height += height;
        return texel;
    }
    let flake = fbm(p * 12.0, seed ^ 0x33);
    if (0.35..1.15).contains(&rho) && flake > 0.70 {
        let mut texel = Texel::steel(grain, 1.0);
        texel.height += height;
        return texel;
    }
    let ring_out = 1.5 + 0.3 * (fbm(p * 3.0, seed ^ 0x44) - 0.5);
    if rho > 0.3 && rho < ring_out {
        let craze = crack(p * 9.0, seed ^ 0x22, 0.90).max(crack(p * 17.0, seed ^ 0x23, 0.93));
        let fade = 1.0 - smoothstep(ring_out - 0.25, ring_out, rho);
        if craze * fade > 0.05 {
            return Texel::paint_edge(0.85 * craze * fade, height);
        }
    }
    if rho < 1.12 {
        // The bowl over the paint: a translucent grey the bowl's normal shades.
        return Texel {
            albedo: Vec3::splat(0.10),
            alpha: 0.45 * (1.0 - smoothstep(0.95, 1.12, rho)),
            metallic: 0.0,
            roughness: 0.7,
            height,
        };
    }
    Texel {
        height,
        ..Texel::CLEAR
    }
}

fn scratch(p: Vec2, seed: u32) -> Texel {
    // The widest reach past the axis is the noisy half-width, the lip and the chips: under 0.47,
    // so the gouge starts that far inside the square's edge.
    let (start, end) = (-0.52_f32, 0.80_f32);
    let t = ((p.x - start) / (end - start)).clamp(0.0, 1.0);
    let axis = Vec2::new(start + t * (end - start), 0.0);
    // The gouge tapers toward its tail, the way the round left the face.
    let width = SCRATCH_WIDTH * (1.0 - 0.8 * t.powf(0.8));
    let edge = fbm(Vec2::new(p.x * 3.0, p.y * 3.0), seed) - 0.5;
    let dist = (p - axis).length();
    let d = dist - width * (1.0 + 0.25 * edge);
    let streak = fbm(Vec2::new(p.x * 1.5, p.y * 30.0), seed ^ 0x4);
    if d < 0.0 {
        let q = (dist / width).min(1.0);
        let depth = SCRATCH_DEPTH * (1.0 - 0.7 * t);
        return Texel {
            albedo: STEEL * (0.9 + 0.35 * (streak - 0.5)),
            alpha: smoothstep(0.0, 0.01, -d),
            metallic: 0.88,
            roughness: 0.34 + 0.15 * streak,
            height: -depth * (1.0 - q * q),
        };
    }
    let lip = 0.035;
    if d < lip {
        let s = 1.0 - d / lip;
        return Texel::paint_edge(0.55 * s, 0.004 * s);
    }
    if d < 0.10 && fbm(p * 18.0, seed ^ 0x77) > 0.72 {
        return Texel::steel(streak, 1.0);
    }
    Texel::CLEAR
}

/// Past a hole's bare ring: the dark edge of the paint the ring tore, then scattered bare chips.
fn chipped_paint(p: Vec2, past: f32, seed: u32) -> Texel {
    let edge = 0.03;
    if past < edge {
        return Texel::paint_edge(0.6 * (1.0 - past / edge), 0.003);
    }
    if past < 0.2 && fbm(p * 13.0, seed ^ 0x77) > 0.68 + past {
        return Texel::steel(fbm(p * 22.0, seed ^ 0x5C), 1.0);
    }
    Texel::CLEAR
}

/// A crack network: ridges of `p`'s noise sharper than `sharpness` (0–1), as a 0–1 mask.
fn crack(p: Vec2, seed: u32, sharpness: f32) -> f32 {
    let ridge = 1.0 - (2.0 * fbm(p, seed) - 1.0).abs();
    smoothstep(sharpness, 1.0, ridge)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A lattice point's value, in `[0, 1)`.
fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    let z = (x as u32 as u64) | ((y as u32 as u64) << 32);
    let z = super::super::mix64(z ^ u64::from(seed).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// Bilinear value noise with a smooth fade, in `[0, 1]`.
fn value_noise(p: Vec2, seed: u32) -> f32 {
    let cell = p.floor();
    let f = p - cell;
    let (x, y) = (cell.x as i32, cell.y as i32);
    let fade = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let a = lattice(x, y, seed);
    let b = lattice(x + 1, y, seed);
    let c = lattice(x, y + 1, seed);
    let d = lattice(x + 1, y + 1, seed);
    let top = a + (b - a) * fade.x;
    let bottom = c + (d - c) * fade.x;
    top + (bottom - top) * fade.y
}

/// Four octaves of [`value_noise`], each turned a little off the last's lattice, in `[0, 1]`.
fn fbm(p: Vec2, seed: u32) -> f32 {
    let turn = Vec2::from_angle(0.6);
    let mut point = p;
    let mut amplitude = 0.5;
    let mut sum = 0.0;
    let mut total = 0.0;
    for octave in 0..4 {
        sum += amplitude * value_noise(point, seed.wrapping_add(octave * 0x9E37));
        total += amplitude;
        point = turn.rotate(point) * 2.03;
        amplitude *= 0.5;
    }
    sum / total
}

/// The four maps from a profile's texels.
fn encode(texels: &[Texel], deepest: f32) -> MarkMaps {
    let mut color = Vec::with_capacity(texels.len() * 4);
    let mut metal_rough = Vec::with_capacity(texels.len() * 4);
    let mut depth = Vec::with_capacity(texels.len());
    for texel in texels {
        let albedo = texel.albedo.to_array().map(srgb_byte);
        color.extend_from_slice(&[albedo[0], albedo[1], albedo[2], unit_byte(texel.alpha)]);
        metal_rough.extend_from_slice(&[
            255,
            unit_byte(texel.roughness),
            unit_byte(texel.metallic),
            255,
        ]);
        depth.push(unit_byte(-texel.height / deepest));
    }
    let mut normal = Vec::with_capacity(texels.len() * 4);
    // Central differences in the square's units: two texels apart.
    let span = 2.0 * 2.0 / SIZE as f32;
    let height = |column: usize, row: usize| texels[row * SIZE + column].height;
    for row in 0..SIZE {
        for column in 0..SIZE {
            let dx = (height((column + 1).min(SIZE - 1), row)
                - height(column.saturating_sub(1), row))
                / span;
            let dy = (height(column, (row + 1).min(SIZE - 1))
                - height(column, row.saturating_sub(1)))
                / span;
            let n = Vec3::new(-dx, -dy, 1.0).normalize();
            normal.extend_from_slice(&[
                unit_byte(n.x * 0.5 + 0.5),
                unit_byte(n.y * 0.5 + 0.5),
                unit_byte(n.z * 0.5 + 0.5),
                255,
            ]);
        }
    }
    MarkMaps {
        color: image(color, 4, TextureFormat::Rgba8UnormSrgb),
        normal: image(normal, 4, TextureFormat::Rgba8Unorm),
        metal_rough: image(metal_rough, 4, TextureFormat::Rgba8Unorm),
        depth: image(depth, 1, TextureFormat::R8Unorm),
    }
}

fn unit_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A linear channel as an sRGB-encoded byte.
fn srgb_byte(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    unit_byte(encoded)
}

/// A square map with its full mip chain, each level the 2×2 box average of the one above.
fn image(level0: Vec<u8>, channels: usize, format: TextureFormat) -> Image {
    let mut data = level0.clone();
    let mut level = level0;
    let mut side = SIZE;
    let mut levels = 1;
    while side > 1 {
        let half = side / 2;
        let mut next = Vec::with_capacity(half * half * channels);
        for row in 0..half {
            for column in 0..half {
                for channel in 0..channels {
                    let at =
                        |c: usize, r: usize| u32::from(level[(r * side + c) * channels + channel]);
                    let sum = at(2 * column, 2 * row)
                        + at(2 * column + 1, 2 * row)
                        + at(2 * column, 2 * row + 1)
                        + at(2 * column + 1, 2 * row + 1);
                    next.push(((sum + 2) / 4) as u8);
                }
            }
        }
        data.extend_from_slice(&next);
        level = next;
        side = half;
        levels += 1;
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: SIZE as u32,
            height: SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        anisotropy_clamp: 8,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each kind's maps carry the whole mip chain the descriptor declares, and each profile draws
    /// something and leaves its border clear: the decal's quad edge must never show (bevy's forward
    /// decal docs ask for transparent padding against stretching at steep views). A mutant that
    /// miscounts the mip levels uploads a short buffer; one that lets a profile reach the border
    /// draws the quad's outline on the hull.
    #[test]
    fn every_kind_draws_inside_a_clear_border() {
        for kind in MarkKind::ALL {
            let maps = generate(kind, 0);
            let color = maps.color.data.as_ref().unwrap();
            let levels = maps.color.texture_descriptor.mip_level_count;
            let texels: usize = (0..levels).map(|level| (SIZE >> level).max(1).pow(2)).sum();
            assert_eq!(color.len(), texels * 4, "{kind:?}");
            let alpha = |column: usize, row: usize| color[(row * SIZE + column) * 4 + 3];
            let drawn = (0..SIZE * SIZE)
                .filter(|i| alpha(i % SIZE, i / SIZE) > 128)
                .count();
            assert!(drawn > SIZE * SIZE / 50, "{kind:?} draws {drawn} texels");
            for i in 0..SIZE {
                for (column, row) in [(i, 0), (i, SIZE - 1), (0, i), (SIZE - 1, i)] {
                    assert_eq!(alpha(column, row), 0, "{kind:?} at ({column}, {row})");
                }
            }
        }
    }
}
