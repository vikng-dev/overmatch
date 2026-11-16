//! The ground's SURFACE as data (`assets/terrain/surface.ron`, embedded): which pack the base and
//! each blended layer is, the metres each was authored at, the ground type each is, and the
//! blend's look. [`TerrainSurface::parse`] admits the file on first read, and the shipped-file
//! test refuses a bad one in CI, so it is refused before play naming the field and the fix.
//!
//! Its readers: [`crate::terrain_grid`] and [`crate::terrain_lod`] cut the mesh UV at the base
//! pack's authored metres; `world::spawn_environment` binds the base pack's maps
//! ([`Pack::map_path`]); [`crate::terrain_blend`] scales each layer's UV and packs the blend law and
//! the macro tint; the ground field and the ground-type table read [`TerrainSurface::grounds`].

use std::sync::LazyLock;

use serde::Deserialize;

use crate::provenance::{Labelled, Provenance};

/// The file, as its refusals name it.
const SURFACE_FILE: &str = "assets/terrain/surface.ron";

static SURFACE: LazyLock<TerrainSurface> = LazyLock::new(|| {
    crate::provenance::with_shipped_policy(|| {
        TerrainSurface::parse(SURFACE_FILE, include_str!("../assets/terrain/surface.ron"))
    })
    .unwrap_or_else(|error| panic!("{error}"))
});

/// The shipped surface.
pub(crate) fn surface() -> &'static TerrainSurface {
    &SURFACE
}

/// The base pack, then the layers in the mask's channel order: a layer's index is its mask channel
/// (R recesses, G slopes, B lowlands) and its slice in the blend arrays.
#[derive(Debug)]
pub(crate) struct TerrainSurface {
    pub(crate) base: Pack,
    pub(crate) layers: [Pack; 3],
    pub(crate) blend: BlendLook,
}

/// One pack's contract.
#[derive(Debug)]
pub(crate) struct Pack {
    /// The folder under `assets/terrain/` whose `cc.txt` carries the attribution and the authored
    /// size.
    pub(crate) pack: String,
    /// Metres per texture repeat, as the scan declares (Poly Haven `dimensions`, mm). A fact, not a
    /// taste: mapping a scan onto any other repeat resizes every feature in it, and DERIVED, the
    /// 15 m base scan fitted into an 8 m repeat draws a 1 m boulder at 53 cm. View only: texturing
    /// never changes the grid, the oracle or the collider.
    pub(crate) authored_m: f32,
    /// The ground type the pack is: a key into `assets/terrain/ground_types.ron`, whose
    /// `GroundTypes::parse` refuses a table lacking it.
    pub(crate) ground: String,
}

/// The blend's tastes, which `terrain_blend.wesl` reads through `TerrainBlendParams`.
#[derive(Debug)]
pub(crate) struct BlendLook {
    /// How far a layer's own ambient occlusion may raise its weight, as a fraction of that weight.
    /// The bias is MULTIPLICATIVE (`w · (1 + this · ao)`), so a zero weight stays zero however
    /// bright the layer's AO is, which is what makes the shader's skip of a zero weight exact.
    pub(crate) ao_bias: f32,
    /// The gap, in biased-weight units, behind the leading weight at which a layer stops
    /// contributing: one level with the leader keeps its full weight. The transition width, and
    /// the only reason a 1024² mask can read as a sharp boundary: the crossing moves with the AO
    /// bias, so the edge follows the material's detail instead of the mask's texel grid.
    pub(crate) window: f32,
    /// World metres per period of the macro tint (`terrain_blend.wesl` places it), above 0.
    pub(crate) macro_period_m: f32,
    /// How far the macro tint moves the blended albedo, as a fraction of it: the gain is
    /// `1 ± this`, so below 1 keeps it positive.
    pub(crate) macro_amplitude: f32,
}

/// A base pack's three maps under the asset root. Each ships as KTX2 (UASTC 4x4, zstd, full mip
/// chain), cut by `scripts/encode-terrain-ktx2.sh` from masters kept outside the repo. The format
/// is the frame budget, not a packaging detail: a PNG gives bevy one mip level, and a 4k map tiled
/// across the whole horizon then misses the texture cache on nearly every fetch (MEASURED 30 fps).
#[derive(Clone, Copy, Debug)]
pub(crate) enum PackMap {
    /// sRGB albedo, `<pack>_diff.ktx2`.
    Albedo,
    /// OpenGL-convention tangent-space normals, `<pack>_nor_gl.ktx2`.
    Normal,
    /// The glTF-ORM pack (R = AO, G = roughness, B = metallic), `<pack>_arm.ktx2`.
    Arm,
}

impl Pack {
    /// `map`'s path under the asset root: `terrain/<pack>/<pack>_<suffix>.ktx2`.
    pub(crate) fn map_path(&self, map: PackMap) -> String {
        let suffix = match map {
            PackMap::Albedo => "diff",
            PackMap::Normal => "nor_gl",
            PackMap::Arm => "arm",
        };
        format!("terrain/{0}/{0}_{suffix}.ktx2", self.pack)
    }
}

impl TerrainSurface {
    /// Every pack's ground in the shader's slot order: the base, then the layers.
    pub(crate) fn grounds(&self) -> [&str; 4] {
        [
            self.base.ground.as_str(),
            self.layers[0].ground.as_str(),
            self.layers[1].ground.as_str(),
            self.layers[2].ground.as_str(),
        ]
    }

    /// The multiplier `layer` applies to the mesh's UV, which repeats at the base pack's metres:
    /// the ratio of authored sizes, so every pack renders at life size off one UV set.
    pub(crate) fn uv_scale(&self, layer: &Pack) -> f32 {
        self.base.authored_m / layer.authored_m
    }

    /// The surface `text` holds, or why `file` is refused: a malformed file, a field missing or
    /// unknown, a pack that is not a plain folder name, an authored size that is not a positive
    /// sourced fact, a layer count other than the mask's three channels, or a blend taste outside
    /// its law's domain.
    pub(crate) fn parse(file: &str, text: &str) -> Result<Self, String> {
        let raw: RawSurface =
            ron::de::from_str(text).map_err(|error| format!("{file}: {error}"))?;
        let base = raw.base.admit(file, "base")?;
        let layers: [Pack; 3] = raw
            .layers
            .into_iter()
            .enumerate()
            .map(|(index, layer)| layer.admit(file, &format!("layers[{index}]")))
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|layers: Vec<Pack>| {
                format!(
                    "{file}: layers: {} declared; declare exactly 3, one per mask channel \
                     (R recesses, G slopes, B lowlands)",
                    layers.len()
                )
            })?;
        let taste = |field: &str, value: &Labelled<f32>, admits: bool, domain: &str| {
            if !value.value.is_finite() || !admits {
                return Err(format!(
                    "{file}: blend.{field}: {} is outside {domain}; author a value inside it",
                    value.value
                ));
            }
            match value.label {
                Provenance::Taste(_) | Provenance::Fixture(_) => Ok(value.value),
                _ => Err(format!(
                    "{file}: blend.{field}: label it Taste, naming its effect family"
                )),
            }
        };
        let look = raw.blend;
        let blend = BlendLook {
            ao_bias: taste(
                "ao_bias",
                &look.ao_bias,
                look.ao_bias.value >= 0.0,
                "0 or more",
            )?,
            window: taste("window", &look.window, look.window.value > 0.0, "above 0")?,
            macro_period_m: taste(
                "macro_period_m",
                &look.macro_period_m,
                look.macro_period_m.value > 0.0,
                "above 0 m",
            )?,
            macro_amplitude: taste(
                "macro_amplitude",
                &look.macro_amplitude,
                (0.0..1.0).contains(&look.macro_amplitude.value),
                "0..1, where the gain 1 ± amplitude stays positive",
            )?,
        };
        Ok(Self {
            base,
            layers,
            blend,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSurface {
    base: RawPack,
    layers: Vec<RawPack>,
    blend: RawBlend,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPack {
    pack: String,
    authored_m: Labelled<f32>,
    ground: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlend {
    ao_bias: Labelled<f32>,
    window: Labelled<f32>,
    macro_period_m: Labelled<f32>,
    macro_amplitude: Labelled<f32>,
}

impl RawPack {
    fn admit(self, file: &str, at: &str) -> Result<Pack, String> {
        if self.pack.is_empty()
            || !self
                .pack
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(format!(
                "{file}: {at}.pack: {:?} is not a pack folder; name the folder under assets/terrain/ \
                 (letters, digits and underscores)",
                self.pack
            ));
        }
        let authored_m = self.authored_m.value;
        if !authored_m.is_finite() || authored_m <= 0.0 {
            return Err(format!(
                "{file}: {at}.authored_m: {authored_m} is not a size; author the scan's metres per repeat"
            ));
        }
        if !matches!(
            self.authored_m.label,
            Provenance::Measured(_) | Provenance::Derived(_) | Provenance::Fixture(_)
        ) {
            return Err(format!(
                "{file}: {at}.authored_m: label it Measured or Derived, naming the scan's declared dimensions"
            ));
        }
        if self.ground.is_empty() {
            return Err(format!(
                "{file}: {at}.ground: name the pack's ground type from assets/terrain/ground_types.ron"
            ));
        }
        Ok(Pack {
            pack: self.pack,
            authored_m,
            ground: self.ground,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE SHIPPED SURFACE IS ADMITTED, and the blend arrays are cut in its layer order. The shader
    /// reads array slice `i` as mask channel `i`, whose scale and ground are layer `i` here, so a
    /// script cutting another order draws every layer at another's size over another's ground.
    #[test]
    fn the_shipped_surface_is_admitted_and_cut_in_its_layer_order() {
        let surface = surface();
        let script = include_str!("../scripts/encode-terrain-ktx2.sh");
        let cut = script
            .lines()
            .find_map(|line| line.strip_prefix("LAYER_PACKS=\""))
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("the encode script declares LAYER_PACKS=\"…\"");
        let layers: Vec<&str> = surface.layers.iter().map(|l| l.pack.as_str()).collect();
        assert_eq!(cut.split_whitespace().collect::<Vec<_>>(), layers);
        for pack in std::iter::once(&surface.base).chain(&surface.layers) {
            let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/terrain")
                .join(&pack.pack);
            assert!(
                folder.join("cc.txt").is_file(),
                "{}: pack {:?} has no cc.txt carrying its attribution and authored size",
                SURFACE_FILE,
                pack.pack
            );
        }
    }

    /// Each refusal names the field and the fix. FIXTURE DATA: a minimal valid surface, each case
    /// breaking one field.
    #[test]
    fn a_surface_outside_its_domain_is_refused_naming_the_field() {
        let pack = |name: &str, metres: &str| {
            format!(r#"(pack: "{name}", authored_m: ({metres}, Fixture("f")), ground: "g")"#)
        };
        let surface = |base: &str, layers: &str, period: &str, amplitude: &str| {
            format!(
                r#"(base: {base}, layers: [{layers}], blend: (ao_bias: (0.5, Fixture("f")),
                window: (0.4, Fixture("f")), macro_period_m: ({period}, Fixture("f")),
                macro_amplitude: ({amplitude}, Fixture("f"))))"#
            )
        };
        let three = [pack("a", "10.0"), pack("b", "10.0"), pack("c", "10.0")].join(", ");
        let valid = surface(&pack("base", "15.0"), &three, "60.0", "0.1");
        TerrainSurface::parse("f.ron", &valid).expect("the fixture is admitted");
        for (text, field) in [
            (
                surface(&pack("../x", "15.0"), &three, "60.0", "0.1"),
                "base.pack",
            ),
            (
                surface(&pack("base", "0.0"), &three, "60.0", "0.1"),
                "base.authored_m",
            ),
            (
                surface(&pack("base", "15.0"), &pack("a", "10.0"), "60.0", "0.1"),
                "layers",
            ),
            (
                surface(&pack("base", "15.0"), &three, "0.0", "0.1"),
                "blend.macro_period_m",
            ),
            (
                surface(&pack("base", "15.0"), &three, "60.0", "1.0"),
                "blend.macro_amplitude",
            ),
        ] {
            let refusal = TerrainSurface::parse("f.ron", &text).expect_err(field);
            assert!(
                refusal.starts_with(&format!("f.ron: {field}")),
                "{field}: {refusal}"
            );
        }
        let measured = valid.replace(
            r#"window: (0.4, Fixture("f"))"#,
            r#"window: (0.4, Measured("m"))"#,
        );
        let refusal = TerrainSurface::parse("f.ron", &measured).expect_err("a measured taste");
        assert!(
            refusal.contains("blend.window: label it Taste"),
            "{refusal}"
        );
    }
}
