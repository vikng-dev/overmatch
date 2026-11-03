//! The GROUND FIELD: which ground type is under each point of the map, sim side. The map's painted
//! blend weights (`terrain.masks` in its `level.json`, R recesses, G slopes, B lowlands) are
//! decoded once at the data edge, synchronously at startup beside the height grid
//! ([`crate::terrain_grid::decode_height_grid`], ADR-0014: never the view's loaded texture), and
//! held with the ground-type table's row for each of the four packs ([`crate::terrain_blend`]'s
//! slot order: the base pack, then [`crate::terrain_blend::BLEND_LAYERS`]).
//!
//! A point's ground ([`GroundField::ground_at`]) is the four packs' rows blended by the print's
//! painted weights there ([`crate::terrain_blend::print_weights`] of the mask read bilinearly and
//! clamped, as the material's sampler reads it), so the sim reads the same weights the prints and
//! the muzzle dust are drawn with. A world no map paints (the flat slab and its authored course, a
//! synthetic grid) is the base pack's ground everywhere, as the blend is where the mask is silent.
//!
//! The field's resolution is the mask's own, 1024² over the shipped 1 500 m square: 1.46 m a
//! texel (DERIVED, 1500 / 1024). `Resolution`: the mask is the finest statement of the ground the
//! map has, and the patch a player can see is the blend's edge at that texel scale (the material
//! sharpens the transition inside one texel, `terrain_blend::BLEND_WINDOW`); a coarser field would
//! let a player see ground the tracks do not feel, and a finer one would interpolate data the map
//! does not state. Cost: three bytes a texel, 3 MiB on the shipped map (DERIVED, 1024² × 3), and
//! one decode at startup, logged MEASURED.

use std::path::Path;
use std::sync::Arc;

use bevy::math::{UVec2, Vec2, Vec3, Vec4};
use bevy::prelude::Resource;

use crate::ground_types::GroundTypes;
use crate::terrain_blend::{BASE_GROUND, BLEND_LAYERS};
use crate::terrain_grid::RowOrder;

/// Area-weighted constitutive facts. A painted boundary mixes capacities, not law names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContactTraction {
    pub cohesion_pa: f32,
    pub tan_phi: f32,
    pub shear_m: f32,
    pub rigid_share: f32,
}

impl ContactTraction {
    pub const RIGID: Self = Self {
        cohesion_pa: 0.0,
        tan_phi: 0.0,
        shear_m: 0.0,
        rigid_share: 1.0,
    };
}

/// What the ground under a track contact brings to the track law, blended over the packs at the
/// contact's point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContactGround {
    pub traction: ContactTraction,
    /// The friction coefficient the ground brings to the contact, one side's
    /// ([`crate::ground_types::GroundType::friction`]).
    pub friction: f32,
    /// The ground's motion resistance coefficient on a moving track
    /// ([`crate::ground_types::GroundType::rolling_resistance`]).
    pub rolling_resistance: f32,
}

/// Which ground is under each point of the world, sim side: the four packs' grounds and, on a
/// painted map, the weights that blend them. Immutable after startup and shared by `Arc`, so the
/// track field's copy ([`crate::track::oracle::BlockField::with_ground`]) is free.
#[derive(Resource, Clone)]
pub struct GroundField {
    /// Each pack's ground, in the shader's slot order: the base pack, then the layers.
    slots: [ContactGround; 4],
    paint: Paint,
}

/// Whether a map paints this world's ground.
#[derive(Clone)]
enum Paint {
    /// No map paints this world: the base pack's ground is the whole of it.
    Unpainted,
    /// The map's mask: row-major RGB weights, row 0 at `z = -half` (the map's declared row order
    /// folded in at decode), stretched once over the world square of side `world_size_m`.
    Painted {
        size: UVec2,
        world_size_m: f32,
        texels: Arc<[[u8; 3]]>,
    },
}

#[derive(Debug)]
pub(crate) enum GroundMaskError {
    Decode(image::ImageError),
    Colour(image::ColorType),
    Resolution {
        actual: [u32; 2],
        declared: [u32; 2],
    },
}

impl std::fmt::Display for GroundMaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "terrain mask decode failed: {error}"),
            Self::Colour(colour) => write!(
                f,
                "terrain masks must be 8-bit RGB, not {colour:?}; export the declared RGB PNG"
            ),
            Self::Resolution { actual, declared } => write!(
                f,
                "terrain masks are {actual:?}, not declared {declared:?}; fix the mask or manifest resolution"
            ),
        }
    }
}

impl std::error::Error for GroundMaskError {}

impl GroundField {
    /// The four packs' grounds from `table` ([`GroundTypes::parse`] requires every pack's).
    fn slots(table: &GroundTypes) -> [ContactGround; 4] {
        [
            table.get(BASE_GROUND).contact(),
            table.get(BLEND_LAYERS[0].ground).contact(),
            table.get(BLEND_LAYERS[1].ground).contact(),
            table.get(BLEND_LAYERS[2].ground).contact(),
        ]
    }

    /// A world no map paints: the base pack's ground everywhere.
    pub(crate) fn unpainted(table: &GroundTypes) -> Self {
        Self {
            slots: Self::slots(table),
            paint: Paint::Unpainted,
        }
    }

    /// FIXTURE: a world that is `ground` everywhere.
    #[cfg(test)]
    pub(crate) fn uniform(ground: ContactGround) -> Self {
        Self {
            slots: [ground; 4],
            paint: Paint::Unpainted,
        }
    }

    /// FIXTURE: decoded texels in world-row order, quantized like the shipped RGB PNG.
    #[cfg(test)]
    pub(crate) fn fixture_mask(size: UVec2, world_size_m: f32, values: Vec<Vec3>) -> Self {
        Self {
            slots: Self::slots(&GroundTypes::load_shipped()),
            paint: Paint::Painted {
                size,
                world_size_m,
                texels: values
                    .into_iter()
                    .map(|v| v.to_array().map(|x| (x * 255.0).round() as u8))
                    .collect::<Vec<_>>()
                    .into(),
            },
        }
    }

    /// The map's painted ground from its mask's PNG `bytes`: 8-bit RGB at `resolution`, its rows
    /// running toward `rows`, stretched over the square of side `world_size_m`. Pure, so the
    /// tests decode the shipped mask through it.
    pub(crate) fn painted(
        bytes: &[u8],
        resolution: [u32; 2],
        rows: RowOrder,
        world_size_m: f32,
        table: &GroundTypes,
    ) -> Result<Self, GroundMaskError> {
        let image = image::load_from_memory(bytes).map_err(GroundMaskError::Decode)?;
        let color = image.color();
        let image::DynamicImage::ImageRgb8(mask) = image else {
            return Err(GroundMaskError::Colour(color));
        };
        let (width, height) = mask.dimensions();
        if [width, height] != resolution {
            return Err(GroundMaskError::Resolution {
                actual: [width, height],
                declared: resolution,
            });
        }
        let rows_in_file: Vec<[u8; 3]> = mask.pixels().map(|pixel| pixel.0).collect();
        let texels: Vec<[u8; 3]> = match rows {
            RowOrder::TowardPositiveZ => rows_in_file,
            RowOrder::TowardNegativeZ => rows_in_file
                .chunks_exact(width as usize)
                .rev()
                .flatten()
                .copied()
                .collect(),
        };
        Ok(Self {
            slots: Self::slots(table),
            paint: Paint::Painted {
                size: UVec2::new(width, height),
                world_size_m,
                texels: texels.into(),
            },
        })
    }

    /// The shipped map's painted ground, read from the masks `manifest` declares. The masks are a
    /// required in-repo asset, so one that is missing or does not decode as declared aborts naming
    /// the file (ADR-0011).
    pub(crate) fn load_painted(
        manifest: &crate::map::MapManifest,
        root: &Path,
        table: &GroundTypes,
    ) -> Self {
        let started = std::time::Instant::now();
        let path = root.join(manifest.masks_asset());
        let bytes = std::fs::read(&path).unwrap_or_else(|err| {
            panic!(
                "ground field: {} declares terrain masks {}: {err}",
                crate::map::level_path(root).display(),
                path.display(),
            )
        });
        let field = Self::painted(
            &bytes,
            manifest.masks_resolution,
            manifest.rows,
            manifest.extent.world_size_m,
            table,
        )
        .unwrap_or_else(|err| panic!("ground field: {}: {err}", path.display()));
        bevy::log::info!(
            "ground field: masks {}x{} decoded for the sim in {:.1} ms ({} KiB)",
            manifest.masks_resolution[0],
            manifest.masks_resolution[1],
            started.elapsed().as_secs_f64() * 1000.0,
            manifest.masks_resolution[0] as usize * manifest.masks_resolution[1] as usize * 3
                / 1024,
        );
        field
    }

    /// Upload bytes in the file's row order for the shader's manifest mapping. The view
    /// uploads the already decoded mask; it does not read or decode the PNG a second time.
    pub(crate) fn mask_rgba(&self, rows: RowOrder) -> (UVec2, Vec<u8>) {
        match &self.paint {
            Paint::Unpainted => (UVec2::ONE, vec![0, 0, 0, 255]),
            Paint::Painted { size, texels, .. } => {
                let mut rgba = Vec::with_capacity(texels.len() * 4);
                for file_row in 0..size.y {
                    let world_row = match rows {
                        RowOrder::TowardPositiveZ => file_row,
                        RowOrder::TowardNegativeZ => size.y - 1 - file_row,
                    };
                    let start = (world_row * size.x) as usize;
                    for [r, g, b] in &texels[start..start + size.x as usize] {
                        rgba.extend_from_slice(&[*r, *g, *b, 255]);
                    }
                }
                (*size, rgba)
            }
        }
    }

    /// The pack weights at world XZ `xz`, in slot order: the print's painted weights
    /// ([`crate::terrain_blend::print_weights`]) of the mask read bilinearly and clamped at the
    /// texel centres, as the material's sampler reads it. All base pack on an unpainted world.
    pub(crate) fn weights_at(&self, xz: Vec2) -> Vec4 {
        let (size, world_size_m, texels) = match &self.paint {
            Paint::Unpainted => return Vec4::X,
            Paint::Painted {
                size,
                world_size_m,
                texels,
            } => (size, world_size_m, texels),
        };
        let texel = (xz / *world_size_m + 0.5) * size.as_vec2() - 0.5;
        let low = texel.floor();
        let t = texel - low;
        let at = |i: f32, j: f32| {
            let i = (i as i32).clamp(0, size.x as i32 - 1) as usize;
            let j = (j as i32).clamp(0, size.y as i32 - 1) as usize;
            Vec3::from_array(texels[j * size.x as usize + i].map(f32::from)) / 255.0
        };
        let top = at(low.x, low.y).lerp(at(low.x + 1.0, low.y), t.x);
        let bottom = at(low.x, low.y + 1.0).lerp(at(low.x + 1.0, low.y + 1.0), t.x);
        crate::terrain_blend::print_weights(top.lerp(bottom, t.y))
    }

    /// The ground under world XZ `xz`: each pack's ground weighted by [`Self::weights_at`].
    pub fn ground_at(&self, xz: Vec2) -> ContactGround {
        let weights = self.weights_at(xz);
        let blend = |of: fn(&ContactGround) -> f32| {
            weights.dot(Vec4::from_array(self.slots.each_ref().map(of)))
        };
        let soil_share = blend(|g| 1.0 - g.traction.rigid_share);
        let rigid_share = blend(|g| g.traction.rigid_share);
        ContactGround {
            traction: ContactTraction {
                cohesion_pa: blend(|g| g.traction.cohesion_pa),
                tan_phi: blend(|g| g.traction.tan_phi),
                shear_m: if soil_share > 0.0 {
                    blend(|g| g.traction.shear_m * (1.0 - g.traction.rigid_share)) / soil_share
                } else {
                    0.0
                },
                rigid_share,
            },
            friction: if rigid_share > 0.0 {
                // Geometric-mean pairs mix capacities: average sqrt(mu), then square.
                (blend(|g| g.traction.rigid_share * g.friction.sqrt()) / rigid_share).powi(2)
            } else {
                blend(|ground| ground.friction)
            },
            rolling_resistance: blend(|ground| ground.rolling_resistance),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A painted fixture mask of `side`² texels over a `world` m square, each texel `paint(i, j)`
    /// (row `j` toward +Z). FIXTURE DATA.
    fn fixture(side: u32, world: f32, paint: impl Fn(u32, u32) -> [u8; 3]) -> GroundField {
        let mut image = image::RgbImage::new(side, side);
        for (i, j, pixel) in image.enumerate_pixels_mut() {
            pixel.0 = paint(i, j);
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encodes");
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        GroundField::painted(&png, [side, side], RowOrder::TowardPositiveZ, world, &table)
            .expect("decodes")
    }

    /// A CONTACT OVER A PACK'S PATCH READS THAT PACK'S GROUND, and the unpainted ground reads the
    /// base pack's: the track law's grip and rolling resistance are those of the ground the map
    /// paints under the contact. Every channel is checked on its own patch, so a channel read from
    /// the wrong slot fails, and a field that ignored its paint reads the base pack everywhere.
    /// FIXTURE DATA: a 4² mask over 40 m, one 2×2 quadrant per channel, one quadrant silent; the
    /// grounds are the shipped table's rows, compared with themselves (no shipped value is a pass
    /// condition).
    #[test]
    fn a_contact_over_a_packs_patch_reads_that_packs_ground() {
        let field = fixture(4, 40.0, |i, j| match (i < 2, j < 2) {
            (true, true) => [0, 0, 0],
            (false, true) => [255, 0, 0],
            (true, false) => [0, 255, 0],
            (false, false) => [0, 0, 255],
        });
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        let row = |name: &str| table.get(name).contact();
        for (xz, ground) in [
            (Vec2::new(-15.0, -15.0), row(BASE_GROUND)),
            (Vec2::new(15.0, -15.0), row(BLEND_LAYERS[0].ground)),
            (Vec2::new(-15.0, 15.0), row(BLEND_LAYERS[1].ground)),
            (Vec2::new(15.0, 15.0), row(BLEND_LAYERS[2].ground)),
        ] {
            let read = field.ground_at(xz);
            assert_eq!(
                read, ground,
                "at {xz}: the whole constitutive row must follow its pack"
            );
        }
        let flat = GroundField::unpainted(&table);
        assert_eq!(flat.ground_at(Vec2::new(15.0, 15.0)), row(BASE_GROUND));
    }

    /// Painted boundaries mix pressure capacities. A mutant interpolating angles or raw
    /// rigid coefficients instead of strengths moves traction away from the visual mixture.
    #[test]
    fn painted_boundaries_mix_soil_strength_and_rigid_pair_capacities() {
        // FIXTURE: two cohesive soils and two rigid surfaces under a mixed mask.
        let rigid = |friction| ContactGround {
            friction,
            rolling_resistance: 0.0,
            traction: ContactTraction::RIGID,
        };
        let soil = |c, tangent, k| ContactGround {
            friction: 0.0,
            rolling_resistance: 0.0,
            traction: ContactTraction {
                cohesion_pa: c,
                tan_phi: tangent,
                shear_m: k,
                rigid_share: 0.0,
            },
        };
        let mut field = fixture(2, 1.0, |_, _| [0, 0, 0]);
        field.slots = [
            rigid(0.2),
            rigid(0.8),
            soil(1000.0, 0.5, 0.01),
            soil(3000.0, 0.7, 0.03),
        ];
        // Capacity expectations read weights from their owner; the test varies only constitutive mixing.
        field.paint = GroundField::fixture_mask(UVec2::ONE, 1.0, vec![Vec3::splat(0.2)]).paint;
        let actual_weights = field.weights_at(Vec2::ZERO);
        assert!(actual_weights.is_finite());
        let mixed = field.ground_at(Vec2::ZERO);
        let area = 2.0;
        let load = 10_000.0;
        let track = 0.6_f32;
        let capacity = |g: ContactGround| {
            area * g.traction.cohesion_pa
                + load * (g.traction.tan_phi + g.traction.rigid_share * (track * g.friction).sqrt())
        };
        let expected: f32 = field
            .slots
            .iter()
            .zip(actual_weights.to_array())
            .map(|(&g, w)| capacity(g) * w)
            .sum();
        assert!((capacity(mixed) - expected).abs() < 1e-3);
    }

    /// THE SIM READS THE MASK WHERE THE VIEW DRAWS IT: the field's rows run the way the map
    /// declares, so a `-Z` export lands its first row at `+Z` as the material's mask mapping
    /// (`terrain_blend::mask_map`) puts it. A field that skipped the row fold reads the mirrored
    /// patch; a mask at another resolution than the map declares is refused, naming the size.
    /// FIXTURE DATA: a 4² mask over 40 m, its first row fully recesses-painted.
    #[test]
    fn the_field_folds_the_declared_row_order() {
        let side = 4;
        let mut image = image::RgbImage::new(side, side);
        for (_, j, pixel) in image.enumerate_pixels_mut() {
            pixel.0 = if j == 0 { [255, 0, 0] } else { [0, 0, 0] };
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encodes");
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        for (rows, painted_z) in [
            (RowOrder::TowardPositiveZ, -15.0),
            (RowOrder::TowardNegativeZ, 15.0),
        ] {
            let field = GroundField::painted(&png, [side, side], rows, 40.0, &table).unwrap();
            let (size, rgba) = field.mask_rgba(rows);
            assert_eq!(size, UVec2::splat(side));
            assert_eq!(rgba.len(), (side * side) as usize * 4);
            for (index, texel) in rgba.as_chunks::<4>().0.iter().enumerate() {
                assert_eq!(
                    texel,
                    if index < side as usize {
                        &[255, 0, 0, 255]
                    } else {
                        &[0, 0, 0, 255]
                    }
                );
            }
            assert_eq!(
                field.weights_at(Vec2::new(0.0, painted_z)),
                Vec4::Y,
                "{rows:?}"
            );
            assert_eq!(
                field.weights_at(Vec2::new(0.0, -painted_z)),
                Vec4::X,
                "{rows:?}"
            );
        }
        let refused = GroundField::painted(&png, [8, 8], RowOrder::TowardPositiveZ, 40.0, &table);
        assert!(
            refused.is_err_and(|why| matches!(
                why,
                GroundMaskError::Resolution { actual: [4, 4], .. }
            ))
        );
    }

    /// The server can decode the shipped mask at its manifest resolution; channel placement
    /// and row direction are proved independently by the quadrant and row-order fixtures.
    #[test]
    fn the_shipped_mask_decodes_into_the_field() {
        let manifest = crate::map::tests::shipped_manifest();
        let table = GroundTypes::load(&GroundTypes::shipped_path()).expect("the shipped table");
        let started = std::time::Instant::now();
        let field =
            GroundField::load_painted(&manifest, &crate::map::tests::shipped_assets(), &table);
        println!(
            "ground field: the shipped masks read and decoded in {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        assert!(
            matches!(field.paint, Paint::Painted { size, .. } if size.to_array() == manifest.masks_resolution)
        );
    }
}
