//! The capture plan: the one input of a capture run. `OVERMATCH_CAPTURE=<plan>.ron` names a RON
//! file holding one [`CapturePlan`], read once per process by [`plan`]; a missing file, a RON or
//! serde error, an unknown field or a value outside its field's domain fails the boot naming the
//! file and the field (ADR-0011). Paths in the plan resolve against the working directory.
//! Optional fields may be written bare (`step_s: 0.0078125`) or as `Some(…)`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bevy::math::{UVec2, Vec3};
use serde::Deserialize;

/// The variable that names the plan file.
pub(crate) const VARIABLE: &str = "OVERMATCH_CAPTURE";

/// A capture run: the frames it takes, and the scripted events it raises. Every field but
/// `frames` is a cue a root mounts only where its event has an authority to raise it.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapturePlan {
    /// The frames to disk, and the conditions they imply (module doc of `dev_capture`). Absent,
    /// the plan's cues run on a normal window and no frame is taken.
    #[serde(default)]
    pub(crate) frames: Option<Frames>,
    /// One standing tree falls ([`Fell`]); the offline root alone mounts it.
    #[serde(default)]
    pub(crate) fell: Option<Fell>,
    /// Terrain strikes raised with no round flown ([`Impact`]); the offline root alone mounts them.
    #[serde(default)]
    pub(crate) impacts: Vec<Impact>,
    /// One shipped round fired at a tree or the ground beside it ([`Shot`]); the offline root
    /// alone mounts it.
    #[serde(default)]
    pub(crate) shot: Option<Shot>,
    /// Shots at a house wall ([`Wall`]): the offline root fires and frames them, and the dedicated
    /// server fires them for its capture clients, reading the same plan.
    #[serde(default)]
    pub(crate) wall: Option<Wall>,
}

/// What the frames are and where they go.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Frames {
    /// The frame's path. With several times the time goes before the extension, zero-padded to a
    /// fixed width: `shot.png` becomes `shot-00004.0000s.png`, `shot-00008.5000s.png`. Byte order
    /// of the names is time order, so a glob assembles a sequence as is. Its directory is created
    /// at mount.
    pub(crate) out: PathBuf,
    /// App seconds (`Time` as `Update` sees it) of each frame, finite and non-negative; sorted,
    /// and times that share a file name kept once. Default `[6.0]`.
    #[serde(default = "default_at")]
    pub(crate) at: Vec<f32>,
    /// The frame in physical pixels, `(width, height)`. Default `(2560, 1440)`.
    #[serde(default = "default_size")]
    pub(crate) size: (u32, u32),
    /// `false` hides every UI tree, every frame. Default `true`.
    #[serde(default = "yes")]
    pub(crate) ui: bool,
    /// `false` disables the default gizmo group when the cameras are retargeted. Default `true`.
    #[serde(default = "yes")]
    pub(crate) gizmos: bool,
    /// Switch to the gunner optic two seconds before the first frame, by writing the resource the
    /// Shift toggle writes; ignored where that resource is absent.
    #[serde(default)]
    pub(crate) sight: bool,
    /// A positive, fixed virtual frame step, s, for reproducible comparisons. Asset loading still
    /// runs asynchronously, so capture after the scene is ready.
    #[serde(default)]
    pub(crate) step_s: Option<f64>,
    /// Who places the player's camera ([`Camera`]). Default the player's own writers.
    #[serde(default)]
    pub(crate) camera: Camera,
    /// Hold the settings page open on this tab (`video` or `gameplay`), its first row selected, on
    /// a root that mounts the page. The values it shows are the run's `video.ron`; point
    /// `OVERMATCH_CONFIG_DIR` at a scratch directory to stage them.
    #[serde(default)]
    pub(crate) settings: Option<String>,
    /// Report `Update` and `PostUpdate` schedule ambiguities.
    #[serde(default)]
    pub(crate) ambiguities: bool,
}

/// Who places the player's camera. One enum, so a pose and a follow cannot both be asked for.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub(crate) enum Camera {
    /// The root's own camera writers.
    #[default]
    Player,
    /// Held at `eye` looking at `look` (world metres, two distinct points) for the whole run, as a
    /// captor (`camera::CameraCaptive`): the player's own writers stand aside, and the lens stays
    /// the one the camera last had.
    Pose {
        eye: (f32, f32, f32),
        look: (f32, f32, f32),
    },
    /// Held at this world-space offset from the controlled tank, looking at its root.
    Follow { offset: (f32, f32, f32) },
}

/// On the first fixed tick at or after app second `at`, the standing tree of map instance
/// `instance` falls broken at segment `cut` toward azimuth bin `dir`, exactly as a law sets it.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Fell {
    pub(crate) instance: String,
    pub(crate) at: f32,
    pub(crate) cut: u8,
    pub(crate) dir: u8,
}

/// On the first frame at or after app second `at`, a round of calibre `calibre_m` strikes the
/// terrain under world `x, z`: the view's impact read (dust, smoke, the ground mark) is raised
/// there with no round flown and no shot recorded.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Impact {
    pub(crate) x: f32,
    pub(crate) z: f32,
    pub(crate) calibre_m: f32,
    pub(crate) at: f32,
}

/// On the first frame at or after app second `at`, the shipped round `round` in fuze mode `mode`
/// is fired at a real tree or at the ground beside it, and the frames are timed from its burst.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Shot {
    pub(crate) round: String,
    #[serde(default)]
    pub(crate) mode: u8,
    pub(crate) target: ShotTarget,
    pub(crate) at: f32,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShotTarget {
    Tree,
    Ground,
}

/// Shots at one house wall, each at its own second; `sun` picks the wall facing the sun and fires
/// along its rays.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Wall {
    pub(crate) shots: Vec<WallShot>,
    #[serde(default)]
    pub(crate) sun: bool,
}

/// One shot at the wall, at app second `at`: a shipped round square at it, a bare charge of `kg`
/// set off at its foot, or the house's elements and components `names` shot out, below `below_m`
/// (building frame) when given (`dev_capture::wall`'s module doc).
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) enum WallShot {
    Round {
        round: String,
        #[serde(default)]
        mode: u8,
        at: f32,
    },
    Charge {
        kg: f32,
        at: f32,
    },
    Dust {
        names: Vec<String>,
        #[serde(default)]
        below_m: Option<f32>,
        at: f32,
    },
}

fn default_at() -> Vec<f32> {
    vec![6.0]
}

fn default_size() -> (u32, u32) {
    (2560, 1440)
}

fn yes() -> bool {
    true
}

impl CapturePlan {
    /// The plan `text` holds, validated and its frame times normalized; the error names the field.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut plan: Self = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(|error| error.to_string())?;
        plan.validate()?;
        if let Some(frames) = &mut plan.frames {
            frames.at = super::normalize_times(std::mem::take(&mut frames.at));
        }
        Ok(plan)
    }

    fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(&text)
    }

    fn validate(&self) -> Result<(), String> {
        let finite_at = |field: &str, at: f32| {
            if at.is_finite() && at >= 0.0 {
                Ok(())
            } else {
                Err(format!(
                    "{field}: {at} is not a finite, non-negative second"
                ))
            }
        };
        if let Some(frames) = &self.frames {
            if frames.at.is_empty() {
                return Err("frames.at: no frame time".into());
            }
            for &at in &frames.at {
                finite_at("frames.at", at)?;
            }
            if frames.size.0 == 0 || frames.size.1 == 0 {
                return Err(format!("frames.size: {:?} has a zero side", frames.size));
            }
            if let Some(step) = frames.step_s
                && !(step.is_finite() && step > 0.0)
            {
                return Err(format!("frames.step_s: {step} is not a positive step"));
            }
            match frames.camera {
                Camera::Player => {}
                Camera::Pose { eye, look } => {
                    let (eye, look) = (Vec3::from(eye), Vec3::from(look));
                    if !(eye.is_finite() && look.is_finite() && eye != look) {
                        return Err("frames.camera: a pose is two distinct finite points".into());
                    }
                }
                Camera::Follow { offset } => {
                    if !Vec3::from(offset).is_finite() {
                        return Err("frames.camera: a follow offset is finite".into());
                    }
                }
            }
            if let Some(tab) = &frames.settings
                && crate::settings::CapturePage::named(tab).is_none()
            {
                return Err(format!("frames.settings: no settings tab is named {tab:?}"));
            }
        }
        if let Some(fell) = &self.fell
            && !fell.at.is_finite()
        {
            return Err(format!("fell.at: {} is not finite", fell.at));
        }
        for impact in &self.impacts {
            let numbers = [impact.x, impact.z, impact.calibre_m, impact.at];
            if !numbers.iter().all(|n| n.is_finite()) || impact.calibre_m <= 0.0 {
                return Err(format!(
                    "impacts: {impact:?} needs finite numbers and a positive calibre"
                ));
            }
        }
        if let Some(shot) = &self.shot {
            finite_at("shot.at", shot.at)?;
        }
        if let Some(wall) = &self.wall {
            if wall.shots.is_empty() {
                return Err("wall.shots: no shot".into());
            }
            for shot in &wall.shots {
                match *shot {
                    WallShot::Round { at, .. } => finite_at("wall.shots", at)?,
                    WallShot::Charge { kg, at } => {
                        finite_at("wall.shots", at)?;
                        if !(kg.is_finite() && kg > 0.0) {
                            return Err(format!("wall.shots: a charge of {kg} kg"));
                        }
                    }
                    WallShot::Dust {
                        ref names,
                        below_m,
                        at,
                    } => {
                        finite_at("wall.shots", at)?;
                        if names.is_empty() || names.iter().any(String::is_empty) {
                            return Err(format!("wall.shots: dust names {names:?}"));
                        }
                        if below_m.is_some_and(|below| !below.is_finite()) {
                            return Err(format!("wall.shots: dust below {below_m:?} m"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Frames {
    pub(crate) fn size(&self) -> UVec2 {
        UVec2::new(self.size.0, self.size.1)
    }
}

/// This process's plan, read once from the file [`VARIABLE`] names; `None` when it is unset.
///
/// # Panics
///
/// When the variable names a file that is missing or holds no valid plan (module doc).
pub(crate) fn plan() -> Option<&'static CapturePlan> {
    static PLAN: OnceLock<Option<CapturePlan>> = OnceLock::new();
    PLAN.get_or_init(|| {
        let path = PathBuf::from(std::env::var_os(VARIABLE)?);
        let plan = CapturePlan::load(&path)
            .unwrap_or_else(|error| panic!("{VARIABLE}={}: {error}", path.display()));
        Some(plan)
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EVERY FIELD OF THE GRAMMAR PARSES TO ITS OWN SLOT, bare or as `Some(…)`, and the frame times
    /// come out sorted and folded: the drivers write plans by hand from the struct docs, and every
    /// cue plugin reads its slot unchecked.
    #[test]
    fn a_full_plan_parses_every_field() {
        let plan = CapturePlan::parse(
            r#"(
                frames: (
                    out: "out/f.png",
                    at: [12.0, 9.0, 9.00001],
                    size: (640, 360),
                    ui: false,
                    gizmos: false,
                    sight: true,
                    step_s: 0.0078125,
                    camera: Pose(eye: (1.0, 2.0, 3.0), look: (0.0, 0.0, 0.0)),
                    settings: Some("video"),
                    ambiguities: true,
                ),
                fell: (instance: "tree_0001", at: 4.0, cut: 1, dir: 3),
                impacts: [(x: 1.0, z: -2.0, calibre_m: 0.088, at: 5.0)],
                shot: (round: "88mm_HE", mode: 1, target: Ground, at: 6.0),
                wall: (
                    shots: [
                        Round(round: "88mm_AP", at: 7.0),
                        Charge(kg: 2.0, at: 8.0),
                        Dust(names: ["east", "north"], below_m: 0.225, at: 9.0),
                    ],
                    sun: true,
                ),
            )"#,
        )
        .expect("a valid plan");
        let frames = plan.frames.expect("frames");
        assert_eq!(frames.out, PathBuf::from("out/f.png"));
        assert_eq!(frames.at, vec![9.0, 12.0]);
        assert_eq!(frames.size(), UVec2::new(640, 360));
        assert!(!frames.ui && !frames.gizmos && frames.sight && frames.ambiguities);
        assert_eq!(frames.step_s, Some(0.0078125));
        assert_eq!(
            frames.camera,
            Camera::Pose {
                eye: (1.0, 2.0, 3.0),
                look: (0.0, 0.0, 0.0),
            }
        );
        assert_eq!(frames.settings.as_deref(), Some("video"));
        assert_eq!(
            plan.fell,
            Some(Fell {
                instance: "tree_0001".into(),
                at: 4.0,
                cut: 1,
                dir: 3,
            })
        );
        assert_eq!(
            plan.impacts,
            vec![Impact {
                x: 1.0,
                z: -2.0,
                calibre_m: 0.088,
                at: 5.0,
            }]
        );
        assert_eq!(
            plan.shot,
            Some(Shot {
                round: "88mm_HE".into(),
                mode: 1,
                target: ShotTarget::Ground,
                at: 6.0,
            })
        );
        assert_eq!(
            plan.wall,
            Some(Wall {
                shots: vec![
                    WallShot::Round {
                        round: "88mm_AP".into(),
                        mode: 0,
                        at: 7.0,
                    },
                    WallShot::Charge { kg: 2.0, at: 8.0 },
                    WallShot::Dust {
                        names: vec!["east".into(), "north".into()],
                        below_m: Some(0.225),
                        at: 9.0,
                    },
                ],
                sun: true,
            })
        );
    }

    /// A PLAN NAMES ONLY WHAT IT CHANGES: a bare `out` takes one frame at six seconds, at the
    /// documented size, with the UI and gizmos drawn, the player's camera and no cue; an empty plan
    /// takes no frame. A changed default silently changes every plan that leans on it.
    #[test]
    fn a_minimal_plan_takes_the_documented_defaults() {
        let plan = CapturePlan::parse(r#"(frames: (out: "f.png"))"#).expect("a valid plan");
        let frames = plan.frames.as_ref().expect("frames");
        assert_eq!(frames.at, vec![6.0]);
        assert_eq!(frames.size(), UVec2::new(2560, 1440));
        assert!(frames.ui && frames.gizmos && !frames.sight && !frames.ambiguities);
        assert_eq!((frames.step_s, frames.camera), (None, Camera::Player));
        assert_eq!(frames.settings, None);
        assert_eq!(
            (&plan.fell, plan.impacts.is_empty(), &plan.shot, &plan.wall),
            (&None, true, &None, &None)
        );
        let scene = CapturePlan::parse("()").expect("an empty plan");
        assert_eq!(scene.frames, None);
    }

    /// A PLAN THAT CANNOT MEAN WHAT IT SAYS FAILS THE BOOT, NAMING ITS FIELD (ADR-0011): a typo, a
    /// missing path, an empty or negative time, a zero side, a non-positive step, a degenerate
    /// pose, an unknown tab, a non-positive calibre or charge, an empty dust name. Each was a
    /// silent default or a skipped cue under the variables the plan replaced.
    #[test]
    fn a_plan_outside_its_domain_is_refused_naming_the_field() {
        let pose = "Pose(eye: (1.0, 1.0, 1.0), look: (1.0, 1.0, 1.0))";
        let follow = "Follow(offset: (1.0, 2.0, 3.0), up: 1.0)";
        for (text, field) in [
            (
                r#"(frames: (out: "f.png", at_s: [1.0]))"#.to_owned(),
                "at_s",
            ),
            (r#"(frame: (out: "f.png"))"#.to_owned(), "frame"),
            ("(frames: ())".to_owned(), "out"),
            (
                r#"(frames: (out: "f.png", at: []))"#.to_owned(),
                "frames.at",
            ),
            (
                r#"(frames: (out: "f.png", at: [-1.0]))"#.to_owned(),
                "frames.at",
            ),
            (
                r#"(frames: (out: "f.png", size: (0, 720)))"#.to_owned(),
                "frames.size",
            ),
            (
                r#"(frames: (out: "f.png", step_s: 0.0))"#.to_owned(),
                "frames.step_s",
            ),
            (
                format!(r#"(frames: (out: "f.png", camera: {pose}))"#),
                "frames.camera",
            ),
            (
                format!(r#"(frames: (out: "f.png", camera: {follow}))"#),
                "up",
            ),
            (
                r#"(frames: (out: "f.png", settings: "audio"))"#.to_owned(),
                "frames.settings",
            ),
            (
                "(impacts: [(x: 0.0, z: 0.0, calibre_m: 0.0, at: 1.0)])".to_owned(),
                "impacts",
            ),
            (
                r#"(shot: (round: "88mm_AP", target: Tree, at: -2.0))"#.to_owned(),
                "shot.at",
            ),
            ("(wall: (shots: []))".to_owned(), "wall.shots"),
            (
                "(wall: (shots: [Charge(kg: 0.0, at: 1.0)]))".to_owned(),
                "wall.shots",
            ),
            (
                r#"(wall: (shots: [Dust(names: ["east", ""], at: 1.0)]))"#.to_owned(),
                "wall.shots",
            ),
        ] {
            let error = CapturePlan::parse(&text).expect_err("the plan is refused");
            assert!(
                error.contains(field),
                "{text}: {error:?} names no `{field}`"
            );
        }
    }

    /// THE WORKED EXAMPLE THE MODULE DOC NAMES IS A VALID PLAN: it is the grammar's one example
    /// file, the one copied to write the next plan.
    #[test]
    fn the_example_plan_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/capture/example.ron");
        let plan = CapturePlan::load(&path).unwrap_or_else(|error| panic!("{error}"));
        assert!(plan.frames.is_some() && plan.wall.is_some());
    }
}
