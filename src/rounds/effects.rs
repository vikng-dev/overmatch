//! The game's damage-model view: authored flight emissions, resolved with the round registry.
//!
//! Every round states the presence of each emission and of its drawn body and tracer streak, with
//! provenance. Numeric fields are finite and in their rendering domains before a shot exists;
//! renderers consume these records without guessing from calibre, ammunition names or viewer
//! distance. Units are SI, colours linear HDR.
//!
//! A drawn body names a round model ([`BodyModel`]) by its scene path; the model's frame and
//! dimensions are its `<stem>.model.ron` beside the glb ([`MODEL_FILES`]). The world view draws the
//! model unscaled, so a round names a model of its own flying diameter, refused at load otherwise.

use crate::provenance::Sourced;
use serde::Deserialize;

/// The contact read a shell is born with: the closed shape its round's data states, and the one
/// physical scalar every magnitude of that read follows, never a viewer's distance (ADR-0023).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ImpactStyle {
    pub(crate) preset: ImpactPreset,
    /// J: half the flying body's mass times its muzzle speed squared.
    pub(crate) energy_j: f32,
}

impl ImpactStyle {
    /// A shot of `mass_kg` leaving at `speed_mps`.
    pub(crate) fn of(preset: ImpactPreset, mass_kg: f32, speed_mps: f32) -> Self {
        Self {
            preset,
            energy_j: 0.5 * mass_kg * speed_mps * speed_mps,
        }
    }

    /// FIXTURE DATA: `preset` at 1 MJ, for a test that raises a read and flies no round.
    #[cfg(test)]
    pub(crate) const fn fixture(preset: ImpactPreset) -> Self {
        Self {
            preset,
            energy_j: 1.0e6,
        }
    }
}

/// Authored contact dressing, independent of flying diameter. These are provisional art recipes;
/// material feedback still determines whether the surface emits sparks or powder.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub(crate) enum ImpactPreset {
    Compact,
    Layered,
    /// A layered round whose burst functions at contact, its blast energy in J. Never authored: the
    /// registry derives it from the round's fuze and burst ([`crate::rounds`]), so the presentation
    /// cannot disagree with the round's data.
    #[serde(skip_deserializing)]
    Bursting {
        blast_j: f32,
    },
}

/// The round's flight emissions, present on the shell from complete spawn on every role. View
/// plugins alone consume them. An absent emission is an authored construction fact.
#[derive(bevy::prelude::Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlightEffects {
    pub(crate) impact: ImpactPreset,
    pub(crate) ember: Option<EmberEmission>,
    pub(crate) smoke: Option<SmokeEmission>,
    /// The model the round's body is drawn as; `None` flies undrawn.
    pub(crate) body: Option<&'static BodyModel>,
    /// The streak drawn behind the round when its belt marks it a tracer
    /// (`ballistics::TracerRound`); `None` draws no streak for any belt.
    pub(crate) streak: Option<StreakEmission>,
}

impl FlightEffects {
    /// Explicitly untraced, smokeless, undrawn fixture data; never a missing-data default.
    #[cfg(test)]
    pub(crate) const NONE: Self = Self {
        impact: ImpactPreset::Compact,
        ember: None,
        smoke: None,
        body: None,
        streak: None,
    };

    /// FIXTURE DATA for renderer and transport tests. No shipped round reads this constructor.
    #[cfg(test)]
    pub(crate) fn fixture_traced() -> Self {
        Self {
            impact: ImpactPreset::Layered,
            ember: Some(EmberEmission {
                radius_m: 0.09,
                base_offset_m: 0.35,
                burn_s: 2.0,
                fade_s: 0.4,
                emissive: [10.0, 2.5, 0.5],
            }),
            smoke: Some(SmokeEmission {
                lifetime_s: 2.2,
                width_m: 0.35,
                growth_mps: 1.3,
                lateral_accel_mps2: 0.4,
                rise_accel_mps2: 0.35,
                start_m: 10.0,
                fade_m: 20.0,
            }),
            body: None,
            streak: Some(StreakEmission {
                radius_m: 0.018,
                length_per_speed_s: 0.018,
                min_length_m: 2.0,
                emissive: [30.0, 12.0, 3.0],
            }),
        }
    }
}

/// A round model's frame and dimensions, as its `<stem>.model.ron` states them: scene 0 places the
/// tip at the origin and the body along +Z for `length_m`, `diameter_m` across X and Y centred on
/// the axis. The pen cam's stage tests read every model's glb against its file.
#[derive(Debug, PartialEq)]
pub(crate) struct BodyModel {
    /// The glb's asset path, the key a round's `effects.body.model` names.
    pub(crate) scene: &'static str,
    pub(crate) diameter_m: f32,
    pub(crate) length_m: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BodyModelFile {
    diameter_m: Sourced<f32>,
    length_m: Sourced<f32>,
}

/// Every embedded round model: `(scene path, its .model.ron text)`. A test holds this list to the
/// `*.model.ron` files under `assets/`.
pub(crate) const MODEL_FILES: &[(&str, &str)] = &[(
    "shell/shell.glb",
    include_str!("../../assets/shell/shell.model.ron"),
)];

impl BodyModel {
    /// Every embedded model, parsed once. Panics naming the file and field on a malformed one: a
    /// broken authored contract is a build defect, as for the round registry.
    pub(crate) fn shipped() -> &'static [BodyModel] {
        static MODELS: std::sync::LazyLock<Vec<BodyModel>> = std::sync::LazyLock::new(|| {
            MODEL_FILES
                .iter()
                .map(|&(scene, text)| {
                    crate::provenance::shipped(
                        &format!("assets/{}.model.ron", scene.trim_end_matches(".glb")),
                        || BodyModel::parse(scene, text),
                    )
                    .unwrap_or_else(|reason| {
                        panic!(
                            "assets/{}.model.ron: {reason}",
                            scene.trim_end_matches(".glb")
                        )
                    })
                })
                .collect()
        });
        &MODELS
    }

    fn parse(scene: &'static str, text: &str) -> Result<Self, String> {
        let file: BodyModelFile = ron::from_str(text).map_err(|err| err.to_string())?;
        Ok(Self {
            scene,
            diameter_m: number("diameter_m", file.diameter_m.0, true)?,
            length_m: number("length_m", file.length_m.0, true)?,
        })
    }

    /// The embedded model whose scene is `scene`, or the refusal naming the vocabulary.
    pub(crate) fn named(scene: &str) -> Result<&'static BodyModel, String> {
        Self::shipped()
            .iter()
            .find(|model| model.scene == scene)
            .ok_or_else(|| {
                let known: Vec<&str> = Self::shipped().iter().map(|model| model.scene).collect();
                format!("no round model {scene:?}; the embedded models are {known:?}")
            })
    }
}

/// A tracer streak's look: a capsule along the travel, its drawn length the round's speed × a
/// time, never under a floor, glowing in its emissive colour alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StreakEmission {
    pub(crate) radius_m: f32,
    /// Drawn length per m/s of the round's speed (s).
    pub(crate) length_per_speed_s: f32,
    pub(crate) min_length_m: f32,
    pub(crate) emissive: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EmberEmission {
    pub(crate) radius_m: f32,
    /// Offset along the shell's trailing local +Z, metres.
    pub(crate) base_offset_m: f32,
    pub(crate) burn_s: f32,
    pub(crate) fade_s: f32,
    pub(crate) emissive: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SmokeEmission {
    pub(crate) lifetime_s: f32,
    pub(crate) width_m: f32,
    pub(crate) growth_mps: f32,
    /// Coefficients in displacement = coefficient × age², m/s².
    pub(crate) lateral_accel_mps2: f32,
    pub(crate) rise_accel_mps2: f32,
    /// No smoke ribbon inside this travelled arc from the muzzle, metres.
    pub(crate) start_m: f32,
    /// The ribbon's birth taper after `start_m`, metres.
    pub(crate) fade_m: f32,
}

/// Required presences: a `Sourced<Option<_>>` refuses an omitted field as well as missing
/// provenance. Serde's implicit absent-Option default cannot invent an untraced round here.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct File {
    impact: Sourced<ImpactPreset>,
    ember: Sourced<Option<EmberFile>>,
    smoke: Sourced<Option<SmokeFile>>,
    body: Sourced<Option<BodyFile>>,
    streak: Sourced<Option<StreakFile>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BodyFile {
    model: Sourced<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StreakFile {
    radius_m: Sourced<f32>,
    length_per_speed_s: Sourced<f32>,
    min_length_m: Sourced<f32>,
    emissive: Sourced<(f32, f32, f32)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmberFile {
    radius_m: Sourced<f32>,
    base_offset_m: Sourced<f32>,
    burn_s: Sourced<f32>,
    fade_s: Sourced<f32>,
    emissive: Sourced<(f32, f32, f32)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmokeFile {
    lifetime_s: Sourced<f32>,
    width_m: Sourced<f32>,
    growth_mps: Sourced<f32>,
    lateral_accel_mps2: Sourced<f32>,
    rise_accel_mps2: Sourced<f32>,
    start_m: Sourced<f32>,
    fade_m: Sourced<f32>,
}

fn number(field: &str, value: f32, positive: bool) -> Result<f32, String> {
    if !value.is_finite() || value < 0.0 || (positive && value == 0.0) {
        return Err(format!(
            "effects.{field} must be finite and {} (got {value}) — author a valid emission value",
            if positive { "positive" } else { "nonnegative" }
        ));
    }
    Ok(value)
}

impl File {
    /// The effects of a round whose flying body is `flying_caliber` (m) across.
    pub(super) fn resolve(self, flying_caliber: f32) -> Result<FlightEffects, String> {
        let ember = self.ember.0.map(|file| -> Result<EmberEmission, String> {
            let (r, g, b) = file.emissive.0;
            let emissive = [number("ember.emissive.red", r, false)?,
                number("ember.emissive.green", g, false)?, number("ember.emissive.blue", b, false)?];
            if emissive.iter().all(|&channel| channel == 0.0) {
                return Err("effects.ember.emissive states no emission — author a nonzero colour or explicitly no ember".into());
            }
            Ok(EmberEmission {
                radius_m: number("ember.radius_m", file.radius_m.0, true)?,
                base_offset_m: number("ember.base_offset_m", file.base_offset_m.0, false)?,
                burn_s: number("ember.burn_s", file.burn_s.0, true)?,
                fade_s: number("ember.fade_s", file.fade_s.0, true)?, emissive,
            })
        }).transpose()?;
        let smoke = self
            .smoke
            .0
            .map(|file| -> Result<SmokeEmission, String> {
                Ok(SmokeEmission {
                    lifetime_s: number("smoke.lifetime_s", file.lifetime_s.0, true)?,
                    width_m: number("smoke.width_m", file.width_m.0, true)?,
                    growth_mps: number("smoke.growth_mps", file.growth_mps.0, false)?,
                    lateral_accel_mps2: number(
                        "smoke.lateral_accel_mps2",
                        file.lateral_accel_mps2.0,
                        false,
                    )?,
                    rise_accel_mps2: number(
                        "smoke.rise_accel_mps2",
                        file.rise_accel_mps2.0,
                        false,
                    )?,
                    start_m: number("smoke.start_m", file.start_m.0, false)?,
                    fade_m: number("smoke.fade_m", file.fade_m.0, true)?,
                })
            })
            .transpose()?;
        let body = self
            .body
            .0
            .map(|file| -> Result<&'static BodyModel, String> {
                let model = BodyModel::named(&file.model.0)
                    .map_err(|reason| format!("effects.body.model: {reason}"))?;
                if (model.diameter_m - flying_caliber).abs() > 1.0e-4 {
                    return Err(format!(
                        "effects.body.model {:?} is {} m across and the round flies {} m across — \
                         the view draws a model unscaled, so name a model of the round's own \
                         diameter or draw no body",
                        model.scene, model.diameter_m, flying_caliber
                    ));
                }
                Ok(model)
            })
            .transpose()?;
        let streak = self
            .streak
            .0
            .map(|file| -> Result<StreakEmission, String> {
                let (r, g, b) = file.emissive.0;
                let emissive = [
                    number("streak.emissive.red", r, false)?,
                    number("streak.emissive.green", g, false)?,
                    number("streak.emissive.blue", b, false)?,
                ];
                if emissive.iter().all(|&channel| channel == 0.0) {
                    return Err(
                        "effects.streak.emissive states no emission — author a nonzero \
                        colour or explicitly no streak"
                            .into(),
                    );
                }
                Ok(StreakEmission {
                    radius_m: number("streak.radius_m", file.radius_m.0, true)?,
                    length_per_speed_s: number(
                        "streak.length_per_speed_s",
                        file.length_per_speed_s.0,
                        true,
                    )?,
                    min_length_m: number("streak.min_length_m", file.min_length_m.0, true)?,
                    emissive,
                })
            })
            .transpose()?;
        Ok(FlightEffects {
            impact: self.impact.0,
            ember,
            smoke,
            body,
            streak,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flight rendering relies on declared presence and positive lifetime/size divisors. A missing
    /// field, missing provenance or unusable value must refuse while the registry loads.
    #[test]
    fn emission_presence_and_numeric_domains_are_required() {
        let none = r#"(impact: (Compact, Fixture("fixture impact")), ember: (None, Fixture("fixture absence")), smoke: (None, Fixture("fixture absence")),
            body: (None, Fixture("fixture absence")), streak: (None, Fixture("fixture absence")))"#;
        assert_eq!(
            ron::from_str::<File>(none).unwrap().resolve(0.05).unwrap(),
            FlightEffects::NONE
        );
        for missing in [
            "()",
            r#"(impact: (Compact, Fixture("fixture impact")), ember: (None, Fixture("fixture")))"#,
            r#"(impact: (Compact, Fixture("fixture impact")), ember: None, smoke: (None, Fixture("fixture")))"#,
        ] {
            assert!(ron::from_str::<File>(missing).is_err());
        }
        for (field, value, positive) in [
            ("ember.radius_m", 0.0, true),
            ("ember.fade_s", -1.0, true),
            ("smoke.start_m", f32::NAN, false),
            ("smoke.width_m", f32::INFINITY, true),
            ("streak.radius_m", 0.0, true),
        ] {
            assert!(number(field, value, positive).unwrap_err().contains(field));
        }
    }

    /// Contact dressing belongs to the authored round. Neither a smaller nor larger flying body
    /// may silently choose another recipe; an omitted recipe must refuse at the registry edge.
    #[test]
    fn contact_recipe_is_required_and_independent_of_flying_diameter() {
        for (name, expected) in [
            ("Compact", ImpactPreset::Compact),
            ("Layered", ImpactPreset::Layered),
        ] {
            let text = format!(
                r#"(impact: ({name}, Fixture("fixture art")),
                ember: (None, Fixture("fixture absence")), smoke: (None, Fixture("fixture absence")),
                body: (None, Fixture("fixture absence")), streak: (None, Fixture("fixture absence")))"#
            );
            // FIXTURE DATA: distinct flying diameters, with no drawn body to constrain either.
            for diameter in [0.008, 0.09] {
                assert_eq!(
                    ron::from_str::<File>(&text)
                        .unwrap()
                        .resolve(diameter)
                        .unwrap()
                        .impact,
                    expected
                );
            }
            let missing = text.replacen(
                &format!("impact: ({name}, Fixture(\"fixture art\")),"),
                "",
                1,
            );
            assert!(
                ron::from_str::<File>(&missing).is_err(),
                "an omitted impact recipe must refuse"
            );
        }
    }

    /// A drawn body names an embedded model of the round's own flying diameter: the world view
    /// draws the model unscaled, so a mismatched or unknown model would draw a round at another
    /// round's size. FIXTURE DATA: the first embedded model, and rounds flying at its diameter
    /// and at twice it.
    #[test]
    fn a_drawn_body_names_an_embedded_model_of_the_flying_diameter() {
        let model = &BodyModel::shipped()[0];
        let effects = |name: &str| {
            format!(
                r#"(impact: (Compact, Fixture("fixture impact")), ember: (None, Fixture("f")), smoke: (None, Fixture("f")),
                body: (Some((model: ({name:?}, Fixture("f")))), Fixture("f")),
                streak: (None, Fixture("f")))"#
            )
        };
        let drawn = ron::from_str::<File>(&effects(model.scene))
            .unwrap()
            .resolve(model.diameter_m)
            .unwrap();
        assert_eq!(drawn.body, Some(model));
        for (name, caliber, word) in [
            (model.scene, 2.0 * model.diameter_m, "unscaled"),
            ("nothing.glb", model.diameter_m, "no round model"),
        ] {
            let err = ron::from_str::<File>(&effects(name))
                .unwrap()
                .resolve(caliber)
                .unwrap_err();
            assert!(
                err.contains("effects.body.model") && err.contains(word),
                "{err}"
            );
        }
    }

    /// Every `*.model.ron` under `assets/` is an embedded model and every embedded model has its
    /// file: a model file the list misses would never be read, and its glb never checked.
    #[test]
    fn the_embedded_models_are_every_model_file_on_disk() {
        fn walk(dir: &std::path::Path, root: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, found);
                } else if let Some(stem) = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .and_then(|path| path.strip_suffix(".model.ron"))
                {
                    found.push(format!("{stem}.glb"));
                }
            }
        }
        let root = crate::assets::asset_root();
        let mut found = Vec::new();
        walk(&root, &root, &mut found);
        found.sort();
        let mut listed: Vec<String> = MODEL_FILES
            .iter()
            .map(|(scene, _)| (*scene).to_owned())
            .collect();
        listed.sort();
        assert_eq!(found, listed);
    }
}
