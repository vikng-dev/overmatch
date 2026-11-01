//! The game's damage-model view: the muzzle law, resolved with the gun registry.
//!
//! Two facts decide every shot's muzzle emission: its muzzle energy ([`MuzzleEnergy`], half the
//! fired round's launched mass times its speed squared, both data) and its gun's
//! [`MuzzleShape`] (the gun file's `muzzle_shape`). The shape picks the layers and where the gas
//! goes; the energy sets every size, life and light through one curve per property, whose value at
//! a reference energy lives once in the taste file `assets/vfx/muzzle.ron` ([`MuzzleLook`]). A
//! length scales with the cube root of energy, a light with its two-thirds power, a gas life with
//! the file's `duration_exponent`, a gas speed with their quotient; flash lives, shares, ratios,
//! counts and colours do not scale. The law, its choices and its derived numbers are the design
//! record's "The muzzle law" (`.agents/docs/design/view-sweep-review.md`).
//!
//! The registry resolves each (gun, round) once at load into a [`MuzzleEmission`], which every
//! round carries ([`crate::rounds::Round::muzzle`]) to the one renderer (`vfx::muzzle`). Units are
//! SI: metres along or across the bore, seconds, lumens, joules.

use std::sync::LazyLock;

use crate::provenance::Sourced;
use serde::Deserialize;

use super::Mechanism;

/// The shipped taste file, embedded: the gun registry resolves synchronously (ADR-0014).
const LOOK_RON: &str = include_str!("../../assets/vfx/muzzle.ron");

/// A length's exponent on energy: similarity, DERIVED (energy is a volume's worth of gas work).
const LENGTH_EXPONENT: f32 = 1.0 / 3.0;
/// A light's exponent on energy: the flash's area, DERIVED from the same similarity.
const AREA_EXPONENT: f32 = 2.0 / 3.0;

/// A gun's muzzle construction, the one per-gun fact of its muzzle emission: where its propellant
/// gas goes. The set the shipped guns need; a gun with another muzzle adds its variant here and
/// its row to the taste file's `shapes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub(crate) enum MuzzleShape {
    /// Baffles turn most of the gas out sideways.
    MuzzleBrake,
    /// A cone cools and spreads the gas before it flares.
    FlashHider,
}

/// A shot's muzzle energy, J: finite and positive by its one constructor.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub(crate) struct MuzzleEnergy(f32);

/// Which input left a shot without a muzzle energy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MuzzleEnergyError {
    Mass(f32),
    Speed(f32),
    Energy { mass: f32, speed: f32 },
}

impl MuzzleEnergy {
    /// Half of `mass_kg` times `speed_mps` squared, or the input that is not finite and positive.
    pub(crate) fn of(mass_kg: f32, speed_mps: f32) -> Result<Self, MuzzleEnergyError> {
        if !(mass_kg.is_finite() && mass_kg > 0.0) {
            return Err(MuzzleEnergyError::Mass(mass_kg));
        }
        if !(speed_mps.is_finite() && speed_mps > 0.0) {
            return Err(MuzzleEnergyError::Speed(speed_mps));
        }
        let joules = (0.5 * f64::from(mass_kg) * f64::from(speed_mps).powi(2)) as f32;
        if !(joules.is_finite() && joules > 0.0) {
            return Err(MuzzleEnergyError::Energy {
                mass: mass_kg,
                speed: speed_mps,
            });
        }
        Ok(Self(joules))
    }

    pub(crate) fn joules(self) -> f32 {
        self.0
    }
}

/// The flash core's sprite: the radial starburst atlas (four frames, one picked per shot) or the
/// single round glow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub(crate) enum CoreSprite {
    Starburst,
    Glow,
}

/// One shot's muzzle emission, every layer sized: what the renderer draws. Carried by every round
/// ([`crate::rounds::Round::muzzle`]) and every shot ([`crate::ballistics::FireShell`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MuzzleEmission {
    pub(crate) core: FlashCore,
    pub(crate) flames: Flames,
    pub(crate) fireball: Option<Fireball>,
    pub(crate) glow: Option<GlowCard>,
    pub(crate) smoke: Option<MuzzleSmoke>,
    pub(crate) light: FlashLight,
    pub(crate) ground_blast: Option<GroundBlast>,
}

/// The camera-facing hot core, born at its largest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlashCore {
    pub(crate) sprite: CoreSprite,
    /// Birth diameter range, the per-shot jitter, m.
    pub(crate) diameter_m: (f32, f32),
    /// Centre's distance down the bore from the muzzle, m.
    pub(crate) standoff_m: f32,
    pub(crate) lifetime_s: f32,
    /// End-of-life diameter over birth diameter.
    pub(crate) end_scale: f32,
}

/// The flame planes: a forward jet down the bore and, for a shape that turns gas sideways, a jet
/// out of each side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Flames {
    pub(crate) forward: FlameJet,
    pub(crate) side_jets: Option<SideJets>,
    /// Plane width over its length.
    pub(crate) width_ratio: f32,
    pub(crate) lifetime_s: f32,
    pub(crate) end_scale: f32,
}

/// One jet's planes, evenly crossed around its axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlameJet {
    /// How many planes; zero states none.
    pub(crate) planes: u8,
    pub(crate) length_m: (f32, f32),
}

/// A jet out of each side of the bore, in the plane of the bore and the side axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SideJets {
    /// Each jet's axis's angle from the bore, rad, in (0, π): above π/2 the jets sweep back.
    pub(crate) angle_rad: f32,
    pub(crate) jet: FlameJet,
}

/// The expanding fireball flipbook, played once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fireball {
    pub(crate) diameter_m: (f32, f32),
    /// End-of-life diameter over birth diameter.
    pub(crate) growth: f32,
    pub(crate) standoff_m: f32,
    pub(crate) lifetime_s: f32,
}

/// A soft fill glow around the core: its diameter is the core's times `scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlowCard {
    pub(crate) scale: f32,
    pub(crate) lifetime_s: f32,
    pub(crate) end_scale: f32,
}

/// The lingering propellant-gas puff.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MuzzleSmoke {
    /// One puff every this many shots, counted over every shot presented: at least one, and for a
    /// belt the shots its cyclic rate fires in the taste file's `puff_interval_s`.
    pub(crate) every_rounds: u32,
    pub(crate) lifetime_s: f32,
    /// Birth and end diameters, m.
    pub(crate) diameter_m: (f32, f32),
    pub(crate) standoff_m: f32,
    pub(crate) rise_mps: f32,
    /// Drift down the bore, m/s.
    pub(crate) push_mps: f32,
    /// Overall opacity, 0..=1.
    pub(crate) opacity: f32,
}

/// The transient muzzle light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlashLight {
    pub(crate) peak_lm: f32,
    pub(crate) range_m: f32,
    pub(crate) lifetime_s: f32,
    /// The emitter's own radius, m.
    pub(crate) radius_m: f32,
    pub(crate) standoff_m: f32,
    /// sRGB colour, each channel 0..=1.
    pub(crate) colour_srgb: [f32; 3],
}

/// The muzzle gas's coupling to the ground under the barrel: within `reach_m` of the muzzle the
/// blast lifts this cloud on fully erodible ground, and the ground's own erodibility
/// ([`crate::ground_types::DustResponse`]) is the share of its puffs any ground gives up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GroundBlast {
    /// Muzzle height over the ground at and below which the gas lifts loose ground, m.
    pub(crate) reach_m: f32,
    /// Puffs on fully erodible ground, inclusive range.
    pub(crate) puffs: (u32, u32),
    pub(crate) birth_diameter_m: f32,
    pub(crate) end_diameter_m: (f32, f32),
    /// Birth offset from the ground point under the muzzle, m.
    pub(crate) spread_m: (f32, f32),
    pub(crate) push_mps: (f32, f32),
    pub(crate) rise_mps: f32,
    /// How far each puff's random azimuth is pulled onto the bore's, in (0.5, 1]: above one half
    /// the radial and bore terms cannot cancel.
    pub(crate) bore_bias: f32,
    pub(crate) lifetime_s: (f32, f32),
    pub(crate) opacity: f32,
}

// --- The taste file as authored.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LookFile {
    reference_energy_j: Sourced<f32>,
    duration_exponent: Sourced<f32>,
    core: CoreFile,
    flames: FlamesFile,
    fireball: FireballFile,
    glow: GlowFile,
    smoke: SmokeFile,
    light: LightFile,
    ground_blast: GroundBlastFile,
    shapes: ShapesFile,
    renderer: RendererFile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreFile {
    diameter_m: Sourced<f32>,
    jitter: Sourced<f32>,
    standoff_m: Sourced<f32>,
    lifetime_s: Sourced<f32>,
    end_scale: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FlamesFile {
    length_m: Sourced<f32>,
    width_ratio: Sourced<f32>,
    lifetime_s: Sourced<f32>,
    end_scale: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FireballFile {
    diameter_m: Sourced<f32>,
    growth: Sourced<f32>,
    standoff_m: Sourced<f32>,
    lifetime_s: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GlowFile {
    scale: Sourced<f32>,
    lifetime_s: Sourced<f32>,
    end_scale: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeFile {
    puff_interval_s: Sourced<f32>,
    lifetime_s: Sourced<f32>,
    birth_diameter_m: Sourced<f32>,
    end_diameter_m: Sourced<f32>,
    standoff_m: Sourced<f32>,
    rise_mps: Sourced<f32>,
    push_mps: Sourced<f32>,
    opacity: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LightFile {
    peak_lm: Sourced<f32>,
    range_m: Sourced<f32>,
    lifetime_s: Sourced<f32>,
    radius_m: Sourced<f32>,
    standoff_m: Sourced<f32>,
    colour_srgb: Sourced<(f32, f32, f32)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroundBlastFile {
    reach_m: Sourced<f32>,
    puffs: Sourced<(u32, u32)>,
    birth_diameter_m: Sourced<f32>,
    end_diameter_m: Sourced<(f32, f32)>,
    spread_m: Sourced<(f32, f32)>,
    push_mps: Sourced<(f32, f32)>,
    rise_mps: Sourced<f32>,
    bore_bias: Sourced<f32>,
    lifetime_s: Sourced<(f32, f32)>,
    opacity: Sourced<f32>,
}

/// Sprite appearance shared by all shots. Values are taste, independent of energy.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RendererFile {
    flash_boost: Sourced<f32>,
    core_sharpness: Sourced<f32>,
    flame_sharpness: Sourced<f32>,
    fireball_opacity: Sourced<f32>,
    fireball_boost: Sourced<f32>,
    fireball_sharpness: Sourced<f32>,
    glow_opacity: Sourced<f32>,
    glow_boost: Sourced<f32>,
    glow_sharpness: Sourced<f32>,
    smoke_boost: Sourced<f32>,
    smoke_sharpness: Sourced<f32>,
    dust_sharpness: Sourced<f32>,
}

/// Validated sprite taste; the renderer applies it without a per-gun override.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MuzzleRenderer {
    pub(crate) flash_boost: f32,
    pub(crate) core_sharpness: f32,
    pub(crate) flame_sharpness: f32,
    pub(crate) fireball_opacity: f32,
    pub(crate) fireball_boost: f32,
    pub(crate) fireball_sharpness: f32,
    pub(crate) glow_opacity: f32,
    pub(crate) glow_boost: f32,
    pub(crate) glow_sharpness: f32,
    pub(crate) smoke_boost: f32,
    pub(crate) smoke_sharpness: f32,
    pub(crate) dust_sharpness: f32,
}

/// One row per [`MuzzleShape`]: a field per variant, so a shape without its row does not parse.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapesFile {
    muzzle_brake: ShapeFile,
    flash_hider: ShapeFile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapeFile {
    core: ShapeCoreFile,
    forward: JetFile,
    side_jets: Sourced<Option<SideJetsFile>>,
    fireball: Sourced<bool>,
    glow: Sourced<bool>,
    smoke: Sourced<bool>,
    ground_blast: Sourced<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapeCoreFile {
    sprite: Sourced<CoreSprite>,
    share: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JetFile {
    planes: Sourced<u8>,
    share: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SideJetsFile {
    angle_deg: Sourced<f32>,
    planes: Sourced<u8>,
    share: Sourced<f32>,
}

// --- The validated taste.

/// The taste file, validated: every magnitude at the reference energy, and each shape's statement.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MuzzleLook {
    reference_j: f32,
    duration_exponent: f32,
    jitter: f32,
    core: ReferenceCore,
    flames: ReferenceFlames,
    fireball: ReferenceFireball,
    glow: GlowCard,
    smoke: ReferenceSmoke,
    light: FlashLight,
    ground_blast: GroundBlast,
    brake: Shape,
    hider: Shape,
    pub(crate) renderer: MuzzleRenderer,
}

/// The core at the reference energy, before its shape's sprite and share.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ReferenceCore {
    diameter_m: f32,
    standoff_m: f32,
    lifetime_s: f32,
    end_scale: f32,
}

/// The flames at the reference energy: one jet's whole length before its shape's share.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ReferenceFlames {
    length_m: f32,
    width_ratio: f32,
    lifetime_s: f32,
    end_scale: f32,
}

/// The fireball at the reference energy, before the jitter.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ReferenceFireball {
    diameter_m: f32,
    growth: f32,
    standoff_m: f32,
    lifetime_s: f32,
}

/// The smoke at the reference energy, before its mechanism's ration.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ReferenceSmoke {
    puff_interval_s: f32,
    lifetime_s: f32,
    diameter_m: (f32, f32),
    standoff_m: f32,
    rise_mps: f32,
    push_mps: f32,
    opacity: f32,
}

/// One shape's statement: its core, its jets and which gas layers it draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shape {
    pub(crate) sprite: CoreSprite,
    pub(crate) core_share: f32,
    pub(crate) forward_planes: u8,
    pub(crate) forward_share: f32,
    pub(crate) side_jets: Option<ShapeSideJets>,
    pub(crate) fireball: bool,
    pub(crate) glow: bool,
    pub(crate) smoke: bool,
    pub(crate) ground_blast: bool,
}

/// A shape's side jets: their angle from the bore (rad), planes and share of the flame length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShapeSideJets {
    pub(crate) angle_rad: f32,
    pub(crate) planes: u8,
    pub(crate) share: f32,
}

/// A finite value, nonnegative or (with `positive`) above zero.
fn number(field: &str, Sourced(value): Sourced<f32>, positive: bool) -> Result<f32, String> {
    if !value.is_finite() || value < 0.0 || (positive && value == 0.0) {
        return Err(format!(
            "{field} must be finite and {} (got {value})",
            if positive { "positive" } else { "nonnegative" }
        ));
    }
    Ok(value)
}

/// A share in 0..=1, or (with `positive`) in (0, 1].
fn share(field: &str, Sourced(value): Sourced<f32>, positive: bool) -> Result<f32, String> {
    if !(0.0..=1.0).contains(&value) || (positive && value == 0.0) {
        return Err(format!(
            "{field} must be a share in {} (got {value})",
            if positive { "(0, 1]" } else { "0..=1" }
        ));
    }
    Ok(value)
}

/// A positive range, low end first.
fn span(field: &str, Sourced((lo, hi)): Sourced<(f32, f32)>) -> Result<(f32, f32), String> {
    let lo = number(field, Sourced(lo), true)?;
    let hi = number(field, Sourced(hi), true)?;
    if lo > hi {
        return Err(format!(
            "{field} is ({lo}, {hi}): state the range low end first"
        ));
    }
    Ok((lo, hi))
}

impl ShapeFile {
    fn resolve(self, name: &str) -> Result<Shape, String> {
        let field = |rest: &str| format!("shapes.{name}.{rest}");
        let side_jets = self
            .side_jets
            .0
            .map(|jets| -> Result<ShapeSideJets, String> {
                let angle = jets.angle_deg.0;
                if !(angle > 0.0 && angle < 180.0) {
                    return Err(format!(
                        "{} must lie strictly between 0 and 180 degrees from the bore (got \
                         {angle})",
                        field("side_jets.angle_deg")
                    ));
                }
                if jets.planes.0 == 0 {
                    return Err(format!(
                        "{} is 0 — a side jet draws at least one plane; state `None` for a shape \
                         that turns no gas sideways",
                        field("side_jets.planes")
                    ));
                }
                Ok(ShapeSideJets {
                    angle_rad: angle.to_radians(),
                    planes: jets.planes.0,
                    share: share(&field("side_jets.share"), jets.share, true)?,
                })
            })
            .transpose()?;
        Ok(Shape {
            sprite: self.core.sprite.0,
            core_share: share(&field("core.share"), self.core.share, true)?,
            forward_planes: self.forward.planes.0,
            forward_share: share(&field("forward.share"), self.forward.share, true)?,
            side_jets,
            fireball: self.fireball.0,
            glow: self.glow.0,
            smoke: self.smoke.0,
            ground_blast: self.ground_blast.0,
        })
    }
}

impl MuzzleLook {
    /// The shipped taste file, parsed once. Panics on a malformed file: a broken shipped contract is
    /// a build defect, as for the gun registry that reads it.
    pub(crate) fn shipped() -> &'static Self {
        static LOOK: LazyLock<MuzzleLook> = LazyLock::new(|| {
            crate::provenance::shipped("assets/vfx/muzzle.ron", || MuzzleLook::parse(LOOK_RON))
                .unwrap_or_else(|err| panic!("assets/vfx/muzzle.ron is unusable: {err}"))
        });
        &LOOK
    }

    /// The taste file `text`, validated; the error names the field.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        Self::from_file(ron::de::from_str(text).map_err(|err| err.to_string())?)
    }

    /// The parsed taste `file`, validated; the error names the field.
    fn from_file(file: LookFile) -> Result<Self, String> {
        let reference_j = number("reference_energy_j", file.reference_energy_j, true)?;
        let duration_exponent = file.duration_exponent.0;
        // A gas life never shortens with energy, and a gas speed (a length over a life) never
        // slows: the exponent lies between zero and a length's.
        if !(0.0..=LENGTH_EXPONENT).contains(&duration_exponent) {
            return Err(format!(
                "duration_exponent must lie in 0..=1/3, so neither a gas life nor a gas speed \
                 falls as energy grows (got {duration_exponent})"
            ));
        }
        let core = file.core;
        let jitter = number("core.jitter", core.jitter, false)?;
        if jitter >= 1.0 {
            return Err(format!(
                "core.jitter must be below 1, so the smallest jittered size stays positive (got \
                 {jitter})"
            ));
        }
        let flames = file.flames;
        let smoke = file.smoke;
        let light = file.light;
        let (r, g, b) = light.colour_srgb.0;
        let blast = file.ground_blast;
        let (lo, hi) = blast.puffs.0;
        if lo == 0 || lo > hi {
            return Err(format!(
                "ground_blast.puffs is ({lo}, {hi}): state at least one puff, low end first"
            ));
        }
        let bore_bias = blast.bore_bias.0;
        if !(bore_bias > 0.5 && bore_bias <= 1.0) {
            return Err(format!(
                "ground_blast.bore_bias is {bore_bias}: state it in (0.5, 1], where a puff's \
                 radial and bore terms cannot cancel"
            ));
        }
        let birth = number("smoke.birth_diameter_m", smoke.birth_diameter_m, true)?;
        let end = number("smoke.end_diameter_m", smoke.end_diameter_m, true)?;
        Ok(Self {
            renderer: MuzzleRenderer {
                flash_boost: number("renderer.flash_boost", file.renderer.flash_boost, true)?,
                core_sharpness: number(
                    "renderer.core_sharpness",
                    file.renderer.core_sharpness,
                    true,
                )?,
                flame_sharpness: number(
                    "renderer.flame_sharpness",
                    file.renderer.flame_sharpness,
                    true,
                )?,
                fireball_opacity: share(
                    "renderer.fireball_opacity",
                    file.renderer.fireball_opacity,
                    false,
                )?,
                fireball_boost: number(
                    "renderer.fireball_boost",
                    file.renderer.fireball_boost,
                    true,
                )?,
                fireball_sharpness: number(
                    "renderer.fireball_sharpness",
                    file.renderer.fireball_sharpness,
                    true,
                )?,
                glow_opacity: share("renderer.glow_opacity", file.renderer.glow_opacity, false)?,
                glow_boost: number("renderer.glow_boost", file.renderer.glow_boost, true)?,
                glow_sharpness: number(
                    "renderer.glow_sharpness",
                    file.renderer.glow_sharpness,
                    true,
                )?,
                smoke_boost: number("renderer.smoke_boost", file.renderer.smoke_boost, true)?,
                smoke_sharpness: number(
                    "renderer.smoke_sharpness",
                    file.renderer.smoke_sharpness,
                    true,
                )?,
                dust_sharpness: number(
                    "renderer.dust_sharpness",
                    file.renderer.dust_sharpness,
                    true,
                )?,
            },
            reference_j,
            duration_exponent,
            jitter,
            core: ReferenceCore {
                diameter_m: number("core.diameter_m", core.diameter_m, true)?,
                standoff_m: number("core.standoff_m", core.standoff_m, false)?,
                lifetime_s: number("core.lifetime_s", core.lifetime_s, true)?,
                end_scale: number("core.end_scale", core.end_scale, true)?,
            },
            flames: ReferenceFlames {
                length_m: number("flames.length_m", flames.length_m, true)?,
                width_ratio: number("flames.width_ratio", flames.width_ratio, true)?,
                lifetime_s: number("flames.lifetime_s", flames.lifetime_s, true)?,
                end_scale: number("flames.end_scale", flames.end_scale, true)?,
            },
            fireball: ReferenceFireball {
                diameter_m: number("fireball.diameter_m", file.fireball.diameter_m, true)?,
                growth: number("fireball.growth", file.fireball.growth, true)?,
                standoff_m: number("fireball.standoff_m", file.fireball.standoff_m, false)?,
                lifetime_s: number("fireball.lifetime_s", file.fireball.lifetime_s, true)?,
            },
            glow: GlowCard {
                scale: number("glow.scale", file.glow.scale, true)?,
                lifetime_s: number("glow.lifetime_s", file.glow.lifetime_s, true)?,
                end_scale: number("glow.end_scale", file.glow.end_scale, true)?,
            },
            smoke: ReferenceSmoke {
                puff_interval_s: number("smoke.puff_interval_s", smoke.puff_interval_s, true)?,
                lifetime_s: number("smoke.lifetime_s", smoke.lifetime_s, true)?,
                diameter_m: span("smoke.end_diameter_m", Sourced((birth, end)))?,
                standoff_m: number("smoke.standoff_m", smoke.standoff_m, false)?,
                rise_mps: number("smoke.rise_mps", smoke.rise_mps, false)?,
                push_mps: number("smoke.push_mps", smoke.push_mps, false)?,
                opacity: share("smoke.opacity", smoke.opacity, false)?,
            },
            light: FlashLight {
                peak_lm: number("light.peak_lm", light.peak_lm, true)?,
                range_m: number("light.range_m", light.range_m, true)?,
                lifetime_s: number("light.lifetime_s", light.lifetime_s, true)?,
                radius_m: number("light.radius_m", light.radius_m, false)?,
                standoff_m: number("light.standoff_m", light.standoff_m, false)?,
                colour_srgb: [
                    share("light.colour_srgb", Sourced(r), false)?,
                    share("light.colour_srgb", Sourced(g), false)?,
                    share("light.colour_srgb", Sourced(b), false)?,
                ],
            },
            ground_blast: GroundBlast {
                reach_m: number("ground_blast.reach_m", blast.reach_m, true)?,
                puffs: (lo, hi),
                birth_diameter_m: number(
                    "ground_blast.birth_diameter_m",
                    blast.birth_diameter_m,
                    true,
                )?,
                end_diameter_m: span("ground_blast.end_diameter_m", blast.end_diameter_m)?,
                spread_m: span("ground_blast.spread_m", blast.spread_m)?,
                push_mps: span("ground_blast.push_mps", blast.push_mps)?,
                rise_mps: number("ground_blast.rise_mps", blast.rise_mps, false)?,
                bore_bias,
                lifetime_s: span("ground_blast.lifetime_s", blast.lifetime_s)?,
                opacity: share("ground_blast.opacity", blast.opacity, false)?,
            },
            brake: file.shapes.muzzle_brake.resolve("muzzle_brake")?,
            hider: file.shapes.flash_hider.resolve("flash_hider")?,
        })
    }

    /// Shape `shape`'s statement.
    pub(crate) fn shape(&self, shape: MuzzleShape) -> Shape {
        match shape {
            MuzzleShape::MuzzleBrake => self.brake,
            MuzzleShape::FlashHider => self.hider,
        }
    }

    /// The emission of a shot of `energy` from a gun of muzzle `shape` and `mechanism`: every
    /// magnitude its reference value times its law's power of `energy` over the reference, the
    /// layers the shape states, the smoke rationed to the mechanism's cyclic rate.
    pub(crate) fn emission(
        &self,
        shape: MuzzleShape,
        energy: MuzzleEnergy,
        mechanism: Mechanism,
    ) -> Result<MuzzleEmission, String> {
        let ratio = energy.joules() / self.reference_j;
        if !(ratio.is_finite() && ratio > 0.0) {
            return Err("reference_energy_j and shot energy produce an unrepresentable ratio; adjust the reference energy or the shot's mass/speed".into());
        }
        let length = ratio.powf(LENGTH_EXPONENT);
        let area = ratio.powf(AREA_EXPONENT);
        let gas_life = ratio.powf(self.duration_exponent);
        let gas_speed = ratio.powf(LENGTH_EXPONENT - self.duration_exponent);
        let jittered = |size: f32| (size * (1.0 - self.jitter), size * (1.0 + self.jitter));
        let scaled = |(lo, hi): (f32, f32), by: f32| (lo * by, hi * by);
        let shape = self.shape(shape);
        let flame = self.flames.length_m * length;
        let every_rounds = match mechanism {
            Mechanism::Single => 1,
            Mechanism::Automatic { cyclic_rpm } => {
                let shots = (self.smoke.puff_interval_s * cyclic_rpm / 60.0).ceil();
                if !(shots.is_finite() && shots >= 0.0 && shots < u32::MAX as f32) {
                    return Err("smoke.puff_interval_s and cyclic_rpm produce an unrepresentable shot count; reduce the interval or cyclic rate".into());
                }
                (shots as u32).max(1)
            }
        };
        let (core, fireball, smoke, blast) =
            (self.core, self.fireball, self.smoke, self.ground_blast);
        let emission = MuzzleEmission {
            core: FlashCore {
                sprite: shape.sprite,
                diameter_m: jittered(core.diameter_m * length * shape.core_share),
                standoff_m: core.standoff_m * length,
                lifetime_s: core.lifetime_s,
                end_scale: core.end_scale,
            },
            flames: Flames {
                forward: FlameJet {
                    planes: shape.forward_planes,
                    length_m: jittered(flame * shape.forward_share),
                },
                side_jets: shape.side_jets.map(|jets| SideJets {
                    angle_rad: jets.angle_rad,
                    jet: FlameJet {
                        planes: jets.planes,
                        length_m: jittered(flame * jets.share),
                    },
                }),
                width_ratio: self.flames.width_ratio,
                lifetime_s: self.flames.lifetime_s,
                end_scale: self.flames.end_scale,
            },
            fireball: shape.fireball.then(|| Fireball {
                diameter_m: jittered(fireball.diameter_m * length),
                growth: fireball.growth,
                standoff_m: fireball.standoff_m * length,
                lifetime_s: fireball.lifetime_s * gas_life,
            }),
            glow: shape.glow.then_some(self.glow),
            smoke: shape.smoke.then(|| MuzzleSmoke {
                every_rounds,
                lifetime_s: smoke.lifetime_s * gas_life,
                diameter_m: scaled(smoke.diameter_m, length),
                standoff_m: smoke.standoff_m * length,
                rise_mps: smoke.rise_mps * gas_speed,
                push_mps: smoke.push_mps * gas_speed,
                opacity: smoke.opacity,
            }),
            light: FlashLight {
                peak_lm: self.light.peak_lm * area,
                range_m: self.light.range_m * length,
                radius_m: self.light.radius_m * length,
                standoff_m: self.light.standoff_m * length,
                ..self.light
            },
            ground_blast: shape.ground_blast.then(|| GroundBlast {
                reach_m: blast.reach_m * length,
                birth_diameter_m: blast.birth_diameter_m * length,
                end_diameter_m: scaled(blast.end_diameter_m, length),
                spread_m: scaled(blast.spread_m, length),
                push_mps: scaled(blast.push_mps, gas_speed),
                rise_mps: blast.rise_mps * gas_speed,
                lifetime_s: scaled(blast.lifetime_s, gas_life),
                ..blast
            }),
        };
        emission.validate()?;
        Ok(emission)
    }
}

impl MuzzleEmission {
    /// Refuse unrepresentable scaled fields and the renderer's derived sizes at the load edge.
    /// The error names the taste field; the registry adds the gun and ammunition row.
    fn validate(&self) -> Result<(), String> {
        let check = |field, value, positive| number(field, Sourced(value), positive).map(|_| ());
        let range = |field, (lo, hi), positive| -> Result<(), String> {
            check(field, lo, positive)?;
            check(field, hi, positive)
        };
        range("core.diameter_m", self.core.diameter_m, true)?;
        check("core.standoff_m", self.core.standoff_m, false)?;
        check(
            "core.end_scale",
            self.core.diameter_m.1 * self.core.end_scale,
            true,
        )?;
        for jet in
            std::iter::once(self.flames.forward).chain(self.flames.side_jets.map(|side| side.jet))
        {
            range("flames.length_m", jet.length_m, true)?;
            check(
                "flames.width_ratio",
                jet.length_m.1 * self.flames.width_ratio,
                true,
            )?;
            check(
                "flames.end_scale",
                jet.length_m.1 * self.flames.end_scale,
                true,
            )?;
            // A plane's end width: the renderer scales its width by both factors at once.
            check(
                "flames.width_ratio × flames.end_scale",
                jet.length_m.1 * self.flames.end_scale * self.flames.width_ratio,
                true,
            )?;
        }
        if let Some(fireball) = self.fireball {
            range("fireball.diameter_m", fireball.diameter_m, true)?;
            check(
                "fireball.growth",
                fireball.diameter_m.1 * fireball.growth,
                true,
            )?;
            check("fireball.standoff_m", fireball.standoff_m, false)?;
            check("fireball.lifetime_s", fireball.lifetime_s, true)?;
        }
        if let Some(glow) = self.glow {
            let size = self.core.diameter_m.1 * glow.scale;
            check("glow.scale", size, true)?;
            check("glow.end_scale", size * glow.end_scale, true)?;
        }
        if let Some(smoke) = self.smoke {
            range("smoke.diameter_m", smoke.diameter_m, true)?;
            check("smoke.standoff_m", smoke.standoff_m, false)?;
            check("smoke.lifetime_s", smoke.lifetime_s, true)?;
            check("smoke.rise_mps", smoke.rise_mps, false)?;
            check("smoke.push_mps", smoke.push_mps, false)?;
        }
        check("light.peak_lm", self.light.peak_lm, true)?;
        check("light.range_m", self.light.range_m, true)?;
        check("light.radius_m", self.light.radius_m, false)?;
        check("light.standoff_m", self.light.standoff_m, false)?;
        if let Some(blast) = self.ground_blast {
            check("ground_blast.reach_m", blast.reach_m, true)?;
            check(
                "ground_blast.birth_diameter_m",
                blast.birth_diameter_m,
                true,
            )?;
            range("ground_blast.end_diameter_m", blast.end_diameter_m, true)?;
            range("ground_blast.spread_m", blast.spread_m, true)?;
            range("ground_blast.push_mps", blast.push_mps, true)?;
            check("ground_blast.rise_mps", blast.rise_mps, false)?;
            range("ground_blast.lifetime_s", blast.lifetime_s, true)?;
        }
        Ok(())
    }
}

#[cfg(test)]
impl MuzzleEmission {
    /// FIXTURE DATA: a large gun's full dressing, every layer present. No shipped gun reads it.
    pub(crate) const fn fixture_cannon() -> Self {
        Self {
            core: FlashCore {
                sprite: CoreSprite::Starburst,
                diameter_m: (3.5, 4.6),
                standoff_m: 1.0,
                lifetime_s: 0.026,
                end_scale: 0.85,
            },
            flames: Flames {
                forward: FlameJet {
                    planes: 2,
                    length_m: (2.3, 3.4),
                },
                side_jets: Some(SideJets {
                    angle_rad: 1.75,
                    jet: FlameJet {
                        planes: 2,
                        length_m: (3.6, 4.9),
                    },
                }),
                width_ratio: 0.55,
                lifetime_s: 0.045,
                end_scale: 0.9,
            },
            fireball: Some(Fireball {
                diameter_m: (8.0, 9.5),
                growth: 1.12,
                standoff_m: 1.8,
                lifetime_s: 0.2,
            }),
            glow: Some(GlowCard {
                scale: 1.5,
                lifetime_s: 0.075,
                end_scale: 0.9,
            }),
            smoke: Some(MuzzleSmoke {
                every_rounds: 1,
                lifetime_s: 1.5,
                diameter_m: (2.2, 4.2),
                standoff_m: 1.6,
                rise_mps: 0.55,
                push_mps: 1.3,
                opacity: 0.92,
            }),
            light: FlashLight {
                peak_lm: 8.0e6,
                range_m: 35.0,
                lifetime_s: 0.055,
                radius_m: 0.4,
                standoff_m: 1.2,
                colour_srgb: [1.0, 0.72, 0.42],
            },
            ground_blast: Some(GroundBlast {
                reach_m: 2.5,
                puffs: (5, 8),
                birth_diameter_m: 1.8,
                end_diameter_m: (3.75, 5.0),
                spread_m: (0.4, 3.0),
                push_mps: (1.8, 4.0),
                rise_mps: 0.35,
                bore_bias: 0.65,
                lifetime_s: (2.0, 2.5),
                opacity: 0.5,
            }),
        }
    }

    /// FIXTURE DATA: a small gun's rationed dressing: one forward plane and no side jet, fireball,
    /// glow or ground blast.
    pub(crate) const fn fixture_rifle() -> Self {
        Self {
            core: FlashCore {
                sprite: CoreSprite::Glow,
                diameter_m: (0.3, 0.55),
                standoff_m: 0.15,
                lifetime_s: 0.03,
                end_scale: 1.2,
            },
            flames: Flames {
                forward: FlameJet {
                    planes: 1,
                    length_m: (0.45, 0.8),
                },
                side_jets: None,
                width_ratio: 0.55,
                lifetime_s: 0.03,
                end_scale: 1.1,
            },
            fireball: None,
            glow: None,
            smoke: Some(MuzzleSmoke {
                every_rounds: 4,
                lifetime_s: 0.7,
                diameter_m: (0.3, 1.0),
                standoff_m: 0.4,
                rise_mps: 0.4,
                push_mps: 0.7,
                opacity: 0.45,
            }),
            light: FlashLight {
                peak_lm: 1.2e6,
                range_m: 16.0,
                lifetime_s: 0.05,
                radius_m: 0.1,
                standoff_m: 0.3,
                colour_srgb: [1.0, 0.72, 0.42],
            },
            ground_blast: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPES: [MuzzleShape; 2] = [MuzzleShape::MuzzleBrake, MuzzleShape::FlashHider];

    /// Every magnitude of `emission` that the law lets grow with energy, by name: what a mutant
    /// that inverts, drops or caps a curve changes.
    fn magnitudes(emission: &MuzzleEmission) -> Vec<(&'static str, f32)> {
        let mut out = vec![
            ("core.diameter_m", emission.core.diameter_m.1),
            ("core.standoff_m", emission.core.standoff_m),
            ("core.lifetime_s", emission.core.lifetime_s),
            (
                "flames.forward.length_m",
                emission.flames.forward.length_m.1,
            ),
            ("flames.lifetime_s", emission.flames.lifetime_s),
            ("light.peak_lm", emission.light.peak_lm),
            ("light.range_m", emission.light.range_m),
            ("light.radius_m", emission.light.radius_m),
            ("light.lifetime_s", emission.light.lifetime_s),
        ];
        if let Some(jets) = emission.flames.side_jets {
            out.push(("flames.side_jets.length_m", jets.jet.length_m.1));
        }
        if let Some(fireball) = emission.fireball {
            out.extend([
                ("fireball.diameter_m", fireball.diameter_m.1),
                ("fireball.lifetime_s", fireball.lifetime_s),
            ]);
        }
        if let Some(glow) = emission.glow {
            out.push(("glow.lifetime_s", glow.lifetime_s));
        }
        if let Some(smoke) = emission.smoke {
            out.extend([
                ("smoke.lifetime_s", smoke.lifetime_s),
                ("smoke.end_diameter_m", smoke.diameter_m.1),
                ("smoke.rise_mps", smoke.rise_mps),
                ("smoke.push_mps", smoke.push_mps),
            ]);
        }
        if let Some(blast) = emission.ground_blast {
            out.extend([
                ("ground_blast.reach_m", blast.reach_m),
                ("ground_blast.end_diameter_m", blast.end_diameter_m.1),
                ("ground_blast.lifetime_s", blast.lifetime_s.1),
                ("ground_blast.push_mps", blast.push_mps.1),
            ]);
        }
        out
    }

    /// The game's damage-model view, a class invariant over the shipped curves: for every shape
    /// and mechanism, a larger muzzle energy never draws a smaller flash, a shorter flame, a
    /// dimmer or shorter-reaching light or a shorter-lived or smaller smoke, and the largest
    /// sizes and the light strictly grow. The consistency the owner asked of the muzzle relies on
    /// it; a mutant that inverts a law's exponent, reads a fixed size or saturates a curve fails.
    /// FIXTURE DATA: an energy ladder over twelve decades, no gun's.
    #[test]
    fn a_larger_muzzle_energy_never_draws_a_smaller_flash_dimmer_light_or_shorter_smoke() {
        let look = MuzzleLook::shipped();
        let ladder: Vec<MuzzleEnergy> = (0..=12)
            .map(|step| {
                MuzzleEnergy::of(2.0, 10f32.powf(0.5 * step as f32)).expect("a positive fixture")
            })
            .collect();
        for shape in SHAPES {
            for mechanism in [
                Mechanism::Single,
                Mechanism::Automatic { cyclic_rpm: 750.0 },
            ] {
                for pair in ladder.windows(2) {
                    let small = magnitudes(&look.emission(shape, pair[0], mechanism).unwrap());
                    let large = magnitudes(&look.emission(shape, pair[1], mechanism).unwrap());
                    assert_eq!(
                        small.len(),
                        large.len(),
                        "{shape:?}: one shape, one set of layers"
                    );
                    for ((name, lo), (_, hi)) in small.iter().zip(&large) {
                        assert!(
                            hi >= lo,
                            "{shape:?} {name}: {hi} at {} J is below {lo} at {} J",
                            pair[1].joules(),
                            pair[0].joules()
                        );
                    }
                    for name in [
                        "core.diameter_m",
                        "flames.forward.length_m",
                        "light.peak_lm",
                    ] {
                        let value = |set: &[(&str, f32)]| {
                            set.iter()
                                .find(|(n, _)| *n == name)
                                .expect("always present")
                                .1
                        };
                        assert!(value(&large) > value(&small), "{shape:?} {name} must grow");
                    }
                }
            }
        }
    }

    /// The game's damage-model view: each independent shape flag survives parsing, resolution
    /// and energy scaling. The renderer relies on exactly the requested layers, even when taste
    /// choices differ from the shipped combinations. FIXTURE DATA: every Boolean combination.
    #[test]
    fn each_shape_states_its_layers_and_energy_never_adds_one() {
        for mask in 0..16 {
            let flags = std::array::from_fn::<_, 4, _>(|bit| mask & (1 << bit) != 0);
            let mut file: LookFile =
                crate::provenance::from_shipped_str("assets/vfx/muzzle.ron", LOOK_RON).unwrap();
            let row = &mut file.shapes.muzzle_brake;
            row.fireball = Sourced(flags[0]);
            row.glow = Sourced(flags[1]);
            row.smoke = Sourced(flags[2]);
            row.ground_blast = Sourced(flags[3]);
            let statement = file.shapes.muzzle_brake.resolve("fixture").unwrap();
            assert_eq!(
                [
                    statement.fireball,
                    statement.glow,
                    statement.smoke,
                    statement.ground_blast
                ],
                flags
            );
            let mut look = MuzzleLook::shipped().clone();
            look.brake = statement;
            look.hider = statement;
            for shape in SHAPES {
                for joules in [3.0e3_f32, 3.0e6] {
                    let energy = MuzzleEnergy::of(2.0, joules.sqrt()).unwrap();
                    let emission = look.emission(shape, energy, Mechanism::Single).unwrap();
                    assert_eq!(
                        [
                            emission.fireball.is_some(),
                            emission.glow.is_some(),
                            emission.smoke.is_some(),
                            emission.ground_blast.is_some()
                        ],
                        flags
                    );
                    assert_eq!(emission.core.sprite, statement.sprite);
                    assert_eq!(
                        emission.flames.side_jets.is_some(),
                        statement.side_jets.is_some()
                    );
                }
            }
        }
    }

    /// The game's damage-model view: each muzzle shape draws its own row of the taste file. A gun's
    /// `muzzle_shape` relies on it; a mutant that answers one shape with the other's row, or
    /// resolves both from one row, fails. FIXTURE DATA: two rows that differ in every statement.
    #[test]
    fn each_shape_resolves_its_own_row() {
        let mut file: LookFile =
            crate::provenance::from_shipped_str("assets/vfx/muzzle.ron", LOOK_RON).unwrap();
        let brake = &mut file.shapes.muzzle_brake;
        brake.core.sprite = Sourced(CoreSprite::Starburst);
        brake.core.share = Sourced(1.0);
        brake.forward.planes = Sourced(2);
        brake.forward.share = Sourced(0.4);
        brake.side_jets = Sourced(Some(SideJetsFile {
            angle_deg: Sourced(120.0),
            planes: Sourced(2),
            share: Sourced(0.7),
        }));
        for flag in [
            &mut brake.fireball,
            &mut brake.glow,
            &mut brake.smoke,
            &mut brake.ground_blast,
        ] {
            *flag = Sourced(true);
        }
        let hider = &mut file.shapes.flash_hider;
        hider.core.sprite = Sourced(CoreSprite::Glow);
        hider.core.share = Sourced(0.5);
        hider.forward.planes = Sourced(3);
        hider.forward.share = Sourced(0.9);
        hider.side_jets = Sourced(None);
        for flag in [
            &mut hider.fireball,
            &mut hider.glow,
            &mut hider.smoke,
            &mut hider.ground_blast,
        ] {
            *flag = Sourced(false);
        }
        let look = MuzzleLook::from_file(file).unwrap();
        assert_eq!(
            look.shape(MuzzleShape::MuzzleBrake),
            Shape {
                sprite: CoreSprite::Starburst,
                core_share: 1.0,
                forward_planes: 2,
                forward_share: 0.4,
                side_jets: Some(ShapeSideJets {
                    angle_rad: 120f32.to_radians(),
                    planes: 2,
                    share: 0.7,
                }),
                fireball: true,
                glow: true,
                smoke: true,
                ground_blast: true,
            }
        );
        assert_eq!(
            look.shape(MuzzleShape::FlashHider),
            Shape {
                sprite: CoreSprite::Glow,
                core_share: 0.5,
                forward_planes: 3,
                forward_share: 0.9,
                side_jets: None,
                fireball: false,
                glow: false,
                smoke: false,
                ground_blast: false,
            }
        );
    }

    /// A belt's smoke is rationed to the shots its cyclic rate fires in the puff interval, and a
    /// single-shot gun puffs every round: the renderer's overdraw bound at a belt's rate. A
    /// mutant that rations by energy or puffs every belt round fails.
    #[test]
    fn a_belt_rations_its_smoke_to_the_puff_interval() {
        let look = MuzzleLook::shipped();
        let energy = MuzzleEnergy::of(0.01, 800.0).expect("a positive fixture");
        let every = |mechanism| {
            look.emission(MuzzleShape::FlashHider, energy, mechanism)
                .unwrap()
                .smoke
                .expect("the hider states smoke")
                .every_rounds
        };
        assert_eq!(every(Mechanism::Single), 1);
        let rpm = 750.0;
        let expected = (look.smoke.puff_interval_s * rpm / 60.0).ceil() as u32;
        assert!(expected > 1, "the fixture belt outruns the interval");
        assert_eq!(every(Mechanism::Automatic { cyclic_rpm: rpm }), expected);
    }

    /// The energy's inputs are refused, naming which, rather than drawing a zero or NaN flash: the
    /// gun registry turns each into a load error naming the round's or the gun's file.
    #[test]
    fn a_shot_without_a_mass_or_a_speed_has_no_energy() {
        assert_eq!(
            MuzzleEnergy::of(0.0, 700.0),
            Err(MuzzleEnergyError::Mass(0.0))
        );
        assert!(matches!(
            MuzzleEnergy::of(f32::NAN, 700.0),
            Err(MuzzleEnergyError::Mass(_))
        ));
        assert_eq!(
            MuzzleEnergy::of(1.0, -1.0),
            Err(MuzzleEnergyError::Speed(-1.0))
        );
        assert_eq!(
            MuzzleEnergy::of(2.0, 10.0).map(MuzzleEnergy::joules),
            Ok(100.0)
        );
    }

    /// The game's damage-model view: outside representable muzzle scales must be refused at
    /// load, before the renderer can receive infinite dimensions. FIXTURE DATA: tiny reference.
    #[test]
    fn scaling_outside_the_emissions_domain_is_refused() {
        let look = MuzzleLook::parse(&LOOK_RON.replace(
            "reference_energy_j: (1.0e6,",
            "reference_energy_j: (1.0e-40,",
        ))
        .unwrap();
        let energy = MuzzleEnergy::of(2.0, 1000.0).unwrap();
        let error = look
            .emission(MuzzleShape::MuzzleBrake, energy, Mechanism::Single)
            .unwrap_err();
        assert!(error.contains("reference_energy_j"));
        // FIXTURE DATA: a representable final energy must not be lost to intermediate rounding.
        assert!(MuzzleEnergy::of(f32::from_bits(1), 1e20).unwrap().joules() > 0.0);
        for (mass, speed) in [(1e30, 1e5), (1e-30, 1e-10)] {
            assert!(matches!(
                MuzzleEnergy::of(mass, speed),
                Err(MuzzleEnergyError::Energy { .. })
            ));
        }
        let look = MuzzleLook::shipped();
        assert!(
            look.emission(
                MuzzleShape::FlashHider,
                energy,
                Mechanism::Automatic {
                    cyclic_rpm: f32::MAX
                }
            )
            .unwrap_err()
            .contains("cyclic_rpm")
        );
        // FIXTURE DATA: each factor of a flame plane's end width is representable on its own at
        // the reference energy; their product is not.
        let mut wide = look.clone();
        wide.flames.width_ratio = 1e20;
        wide.flames.end_scale = 1e20;
        let reference = MuzzleEnergy::of(2.0, wide.reference_j.sqrt()).unwrap();
        assert!(
            wide.emission(MuzzleShape::MuzzleBrake, reference, Mechanism::Single)
                .unwrap_err()
                .contains("flames.width_ratio × flames.end_scale")
        );
        let mut look = look.clone();
        look.fireball.growth = f32::MAX;
        assert!(
            look.emission(MuzzleShape::MuzzleBrake, energy, Mechanism::Single)
                .unwrap_err()
                .contains("fireball.growth")
        );
    }

    /// The game's muzzle rendering: every sprite taste input is required and validated at load.
    /// The owner must be able to author these independently of gun identity and energy.
    #[test]
    fn sprite_taste_is_required_and_refuses_invalid_values() {
        for field in [
            "flash_boost",
            "core_sharpness",
            "flame_sharpness",
            "fireball_opacity",
            "fireball_boost",
            "fireball_sharpness",
            "glow_opacity",
            "glow_boost",
            "glow_sharpness",
            "smoke_boost",
            "smoke_sharpness",
            "dust_sharpness",
        ] {
            let key = format!("{field}:");
            let start = LOOK_RON.find(&key).expect("the required sprite field") + key.len();
            let start = start + LOOK_RON[start..].find('(').expect("a sourced value") + 1;
            let end = start
                + LOOK_RON[start..]
                    .find(',')
                    .expect("a value before provenance");
            let mut changed = LOOK_RON.to_owned();
            changed.replace_range(start..end, "-1.0");
            assert!(
                MuzzleLook::parse(&changed)
                    .unwrap_err()
                    .contains(&format!("renderer.{field}"))
            );
            let missing = LOOK_RON.replace(&key, &format!("missing_{field}:"));
            assert!(MuzzleLook::parse(&missing).is_err());
        }
    }

    /// The taste file refuses, naming the field, a value the law cannot draw: a duration exponent
    /// that would shorten a gas life or slow a gas speed as energy grows, a jitter that would
    /// take a size to zero, and a side jet along the bore. The shipped file loads.
    #[test]
    fn the_taste_file_refuses_what_the_law_cannot_draw() {
        assert!(
            crate::provenance::shipped("assets/vfx/muzzle.ron", || MuzzleLook::parse(LOOK_RON))
                .is_ok()
        );
        for (from, to, field) in [
            (
                "duration_exponent: (0.1667,",
                "duration_exponent: (-0.1,",
                "duration_exponent",
            ),
            (
                "duration_exponent: (0.1667,",
                "duration_exponent: (0.5,",
                "duration_exponent",
            ),
            ("jitter: (0.15,", "jitter: (1.0,", "core.jitter"),
            (
                "angle_deg: (100.0,",
                "angle_deg: (0.0,",
                "side_jets.angle_deg",
            ),
            (
                "reference_energy_j: (1.0e6,",
                "reference_energy_j: (0.0,",
                "reference_energy_j",
            ),
        ] {
            assert!(LOOK_RON.contains(from), "the shipped file states {from}");
            let err = MuzzleLook::parse(&LOOK_RON.replacen(from, to, 1)).unwrap_err();
            assert!(err.contains(field), "{field}: {err}");
        }
        // A shape without its row, and a value without provenance, do not parse.
        let without_hider = LOOK_RON.replacen("flash_hider:", "flash_hider_gone:", 1);
        assert!(MuzzleLook::parse(&without_hider).is_err());
        let unsourced = LOOK_RON.replacen(
            r#"lifetime_s: (0.03, Taste("muzzle: TASTE: two frames at 60 Hz, every gun")),"#,
            "lifetime_s: 0.03,",
            1,
        );
        assert_ne!(unsourced, LOOK_RON);
        assert!(MuzzleLook::parse(&unsourced).is_err());
    }
}
