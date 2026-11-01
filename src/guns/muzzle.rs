//! The game's damage-model view: a gun's authored muzzle emission, resolved with the gun registry.
//!
//! Every gun file states its flash, fireball, glow, smoke, light and ground blast with provenance;
//! an absent layer is an authored `None`. The renderer (`vfx::muzzle`) consumes one record for every
//! gun: no calibre, weapon kind or viewer distance selects a recipe or a size. Values are finite and
//! in their rendering domains before a shot exists. Units are SI; diameters and lengths in metres
//! along or across the bore, times in seconds, light in lumens.

use crate::provenance::Sourced;
use serde::Deserialize;

/// The flash core's sprite: the radial starburst atlas (four frames, one picked per shot) or the
/// single round glow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub(crate) enum CoreSprite {
    Starburst,
    Glow,
}

/// The gun's muzzle emission, carried by every round it fires ([`crate::rounds::Round::muzzle`])
/// and every shot ([`crate::ballistics::FireShell`]) to the one view consumer.
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

/// Bore-aligned flame planes, evenly crossed around the bore.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Flames {
    /// How many planes; zero states none.
    pub(crate) count: u8,
    pub(crate) length_m: (f32, f32),
    /// Plane width over its length.
    pub(crate) width_ratio: f32,
    pub(crate) lifetime_s: f32,
    pub(crate) end_scale: f32,
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
    /// One puff every this many shots, counted over every shot presented.
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
    /// Peak multiplier when the shot carries a tracer.
    pub(crate) tracer_gain: f32,
}

/// The muzzle gas's coupling to the ground under the barrel: within `reach_m` of the muzzle the
/// blast lifts this cloud.
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct File {
    core: CoreFile,
    flames: FlamesFile,
    fireball: Sourced<Option<FireballFile>>,
    glow: Sourced<Option<GlowFile>>,
    smoke: Sourced<Option<SmokeFile>>,
    light: LightFile,
    ground_blast: Sourced<Option<GroundBlastFile>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreFile {
    sprite: Sourced<CoreSprite>,
    diameter_m: Sourced<(f32, f32)>,
    standoff_m: Sourced<f32>,
    lifetime_s: Sourced<f32>,
    end_scale: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FlamesFile {
    count: Sourced<u8>,
    length_m: Sourced<(f32, f32)>,
    width_ratio: Sourced<f32>,
    lifetime_s: Sourced<f32>,
    end_scale: Sourced<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FireballFile {
    diameter_m: Sourced<(f32, f32)>,
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
    every_rounds: Sourced<u32>,
    lifetime_s: Sourced<f32>,
    diameter_m: Sourced<(f32, f32)>,
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
    tracer_gain: Sourced<f32>,
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

/// A finite value, nonnegative or (with `positive`) above zero.
fn number(field: &str, Sourced(value): Sourced<f32>, positive: bool) -> Result<f32, String> {
    if !value.is_finite() || value < 0.0 || (positive && value == 0.0) {
        return Err(format!(
            "muzzle.{field} must be finite and {} (got {value}) — author a valid emission value",
            if positive { "positive" } else { "nonnegative" }
        ));
    }
    Ok(value)
}

/// A share in 0..=1.
fn share(field: &str, Sourced(value): Sourced<f32>) -> Result<f32, String> {
    if !(0.0..=1.0).contains(&value) {
        return Err(format!(
            "muzzle.{field} must be a share in 0..=1 (got {value}) — author a valid emission value"
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
            "muzzle.{field} is ({lo}, {hi}): state the range low end first"
        ));
    }
    Ok((lo, hi))
}

impl File {
    pub(super) fn resolve(self) -> Result<MuzzleEmission, String> {
        let core = FlashCore {
            sprite: self.core.sprite.0,
            diameter_m: span("core.diameter_m", self.core.diameter_m)?,
            standoff_m: number("core.standoff_m", self.core.standoff_m, false)?,
            lifetime_s: number("core.lifetime_s", self.core.lifetime_s, true)?,
            end_scale: number("core.end_scale", self.core.end_scale, true)?,
        };
        let flames = Flames {
            count: self.flames.count.0,
            length_m: span("flames.length_m", self.flames.length_m)?,
            width_ratio: number("flames.width_ratio", self.flames.width_ratio, true)?,
            lifetime_s: number("flames.lifetime_s", self.flames.lifetime_s, true)?,
            end_scale: number("flames.end_scale", self.flames.end_scale, true)?,
        };
        let fireball = self
            .fireball
            .0
            .map(|file| -> Result<Fireball, String> {
                Ok(Fireball {
                    diameter_m: span("fireball.diameter_m", file.diameter_m)?,
                    growth: number("fireball.growth", file.growth, true)?,
                    standoff_m: number("fireball.standoff_m", file.standoff_m, false)?,
                    lifetime_s: number("fireball.lifetime_s", file.lifetime_s, true)?,
                })
            })
            .transpose()?;
        let glow = self
            .glow
            .0
            .map(|file| -> Result<GlowCard, String> {
                Ok(GlowCard {
                    scale: number("glow.scale", file.scale, true)?,
                    lifetime_s: number("glow.lifetime_s", file.lifetime_s, true)?,
                    end_scale: number("glow.end_scale", file.end_scale, true)?,
                })
            })
            .transpose()?;
        let smoke = self
            .smoke
            .0
            .map(|file| -> Result<MuzzleSmoke, String> {
                let every_rounds = file.every_rounds.0;
                if every_rounds == 0 {
                    return Err(
                        "muzzle.smoke.every_rounds is 0 — a puff comes every 1 or more \
                                shots; author `None` for a smokeless gun"
                            .into(),
                    );
                }
                Ok(MuzzleSmoke {
                    every_rounds,
                    lifetime_s: number("smoke.lifetime_s", file.lifetime_s, true)?,
                    diameter_m: span("smoke.diameter_m", file.diameter_m)?,
                    standoff_m: number("smoke.standoff_m", file.standoff_m, false)?,
                    rise_mps: number("smoke.rise_mps", file.rise_mps, false)?,
                    push_mps: number("smoke.push_mps", file.push_mps, false)?,
                    opacity: share("smoke.opacity", file.opacity)?,
                })
            })
            .transpose()?;
        let (r, g, b) = self.light.colour_srgb.0;
        let light = FlashLight {
            peak_lm: number("light.peak_lm", self.light.peak_lm, true)?,
            range_m: number("light.range_m", self.light.range_m, true)?,
            lifetime_s: number("light.lifetime_s", self.light.lifetime_s, true)?,
            radius_m: number("light.radius_m", self.light.radius_m, false)?,
            standoff_m: number("light.standoff_m", self.light.standoff_m, false)?,
            colour_srgb: [
                share("light.colour_srgb", Sourced(r))?,
                share("light.colour_srgb", Sourced(g))?,
                share("light.colour_srgb", Sourced(b))?,
            ],
            tracer_gain: number("light.tracer_gain", self.light.tracer_gain, true)?,
        };
        let ground_blast = self
            .ground_blast
            .0
            .map(|file| -> Result<GroundBlast, String> {
                let (lo, hi) = file.puffs.0;
                if lo == 0 || lo > hi {
                    return Err(format!(
                        "muzzle.ground_blast.puffs is ({lo}, {hi}): state at least one puff, low \
                         end first; author `None` for a gun whose gas lifts no ground"
                    ));
                }
                let bore_bias = file.bore_bias.0;
                if !(bore_bias > 0.5 && bore_bias <= 1.0) {
                    return Err(format!(
                        "muzzle.ground_blast.bore_bias is {bore_bias}: state it in (0.5, 1], where \
                         a puff's radial and bore terms cannot cancel"
                    ));
                }
                Ok(GroundBlast {
                    reach_m: number("ground_blast.reach_m", file.reach_m, true)?,
                    puffs: (lo, hi),
                    birth_diameter_m: number(
                        "ground_blast.birth_diameter_m",
                        file.birth_diameter_m,
                        true,
                    )?,
                    end_diameter_m: span("ground_blast.end_diameter_m", file.end_diameter_m)?,
                    spread_m: span("ground_blast.spread_m", file.spread_m)?,
                    push_mps: span("ground_blast.push_mps", file.push_mps)?,
                    rise_mps: number("ground_blast.rise_mps", file.rise_mps, false)?,
                    bore_bias,
                    lifetime_s: span("ground_blast.lifetime_s", file.lifetime_s)?,
                    opacity: share("ground_blast.opacity", file.opacity)?,
                })
            })
            .transpose()?;
        Ok(MuzzleEmission {
            core,
            flames,
            fireball,
            glow,
            smoke,
            light,
            ground_blast,
        })
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
                count: 2,
                length_m: (4.3, 6.4),
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
                tracer_gain: 1.0,
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

    /// FIXTURE DATA: a small gun's rationed dressing: no fireball, glow or ground blast.
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
                count: 1,
                length_m: (0.45, 0.8),
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
                tracer_gain: 1.5,
            },
            ground_blast: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer divides by lifetimes, draws ranges low end first and normalises the dust
    /// azimuth only because its bias exceeds one half. A file breaking any of those, or omitting a
    /// layer's presence or provenance, must refuse while the gun registry loads.
    #[test]
    fn muzzle_layers_and_their_domains_are_required() {
        let text = |ground: &str, smoke_every: &str| {
            format!(
                r#"(
                core: (sprite: (Glow, Unknown("f")), diameter_m: ((0.3, 0.5), Unknown("f")),
                    standoff_m: (0.1, Unknown("f")), lifetime_s: (0.03, Unknown("f")),
                    end_scale: (1.0, Unknown("f"))),
                flames: (count: (1, Unknown("f")), length_m: ((0.4, 0.8), Unknown("f")),
                    width_ratio: (0.5, Unknown("f")), lifetime_s: (0.03, Unknown("f")),
                    end_scale: (1.0, Unknown("f"))),
                fireball: (None, Unknown("f")),
                glow: (None, Unknown("f")),
                smoke: (Some((every_rounds: ({smoke_every}, Unknown("f")),
                    lifetime_s: (0.7, Unknown("f")), diameter_m: ((0.3, 1.0), Unknown("f")),
                    standoff_m: (0.4, Unknown("f")), rise_mps: (0.4, Unknown("f")),
                    push_mps: (0.7, Unknown("f")), opacity: (0.45, Unknown("f")))), Unknown("f")),
                light: (peak_lm: (1.0e6, Unknown("f")), range_m: (16.0, Unknown("f")),
                    lifetime_s: (0.05, Unknown("f")), radius_m: (0.1, Unknown("f")),
                    standoff_m: (0.3, Unknown("f")), colour_srgb: ((1.0, 0.7, 0.4), Unknown("f")),
                    tracer_gain: (1.0, Unknown("f"))),
                {ground}
            )"#
            )
        };
        let blast = |bias: &str| {
            format!(
                r#"ground_blast: (Some((reach_m: (2.5, Unknown("f")), puffs: ((5, 8), Unknown("f")),
                    birth_diameter_m: (1.8, Unknown("f")), end_diameter_m: ((3.7, 5.0), Unknown("f")),
                    spread_m: ((0.4, 3.0), Unknown("f")), push_mps: ((1.8, 4.0), Unknown("f")),
                    rise_mps: (0.35, Unknown("f")), bore_bias: ({bias}, Unknown("f")),
                    lifetime_s: ((2.0, 2.5), Unknown("f")), opacity: (0.5, Unknown("f")))), Unknown("f")),"#
            )
        };
        let resolve = |text: &str| ron::from_str::<File>(text).map(File::resolve);
        assert!(matches!(
            resolve(&text(r#"ground_blast: (None, Unknown("f")),"#, "4")),
            Ok(Ok(_))
        ));
        assert!(matches!(resolve(&text(&blast("0.65"), "4")), Ok(Ok(_))));
        // An omitted presence does not parse.
        assert!(resolve(&text("", "4")).is_err());
        // A bias at one half lets the two unit terms cancel; zero cadence emits never.
        for bad in [text(&blast("0.5"), "4"), text(&blast("0.65"), "0")] {
            assert!(matches!(resolve(&bad), Ok(Err(_))), "{bad}");
        }
        for (field, value) in [
            ((0.5_f32, 0.3_f32), "low end first"),
            ((0.0, 1.0), "positive"),
        ] {
            let err = span("x.range", Sourced(field)).unwrap_err();
            assert!(err.contains(value), "{err}");
        }
    }
}
