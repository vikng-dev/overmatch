//! The capture plan: frames and cues alongside the scenario levers documented by `dev_capture`. `OVERMATCH_CAPTURE=<plan>.ron` names a RON
//! file holding one [`CapturePlan`], read once per process by [`plan`]; a missing file, a RON or
//! serde error, an unknown field or a value outside its field's domain fails the boot naming the
//! file and the field (ADR-0011). Paths in the plan resolve against the working directory.
//! Optional fields may be written bare (`step_s: 0.0078125`) or as `Some(…)`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bevy::input::keyboard::KeyCode;
use bevy::math::{Dir3, UVec2, Vec3};
use bevy::time::{Time, Virtual};
use serde::Deserialize;

/// The variable that names the plan file.
pub(crate) const VARIABLE: &str = "OVERMATCH_CAPTURE";

/// A capture run: the frames it takes, the keys it holds, and the scripted events it raises.
/// Every windowed root takes `frames` and `keys`; every other field is a cue a root mounts only
/// where its event has an authority to raise it.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapturePlan {
    /// The frames to disk, and the conditions they imply (module doc of `dev_capture`). Absent,
    /// the plan's cues run on a normal window and no frame is taken.
    #[serde(default)]
    pub(crate) frames: Option<Frames>,
    /// Keys held down as a player holds them ([`KeyHold`]), for a view that shows only while its
    /// key is held. Default none.
    #[serde(default)]
    pub(crate) keys: Vec<KeyHold>,
    /// Terrain strikes raised with no round flown ([`Impact`]); the offline root alone mounts them.
    #[serde(default)]
    pub(crate) impacts: Vec<Impact>,
    /// One shipped round fired at a construction or the ground beside it ([`Shot`]); the offline root
    /// alone mounts it.
    #[serde(default)]
    pub(crate) shot: Option<Shot>,
    /// Shots at a house wall ([`Wall`]): the offline root fires and frames them, and the dedicated
    /// server fires them for its capture clients, reading the same plan.
    #[serde(default)]
    pub(crate) wall: Option<Wall>,
    /// The game's damage model: offline module-repair panel evidence.
    #[serde(default)]
    pub(crate) repair: Option<Repair>,
    /// What the root sets the scene up as ([`Scene`]).
    #[serde(default)]
    pub(crate) scene: Scene,
}

/// The game's damage model: empty the named module functions and issue one repair command.
/// Only the offline root mounts this cue; it cannot supply input to a network match.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Repair {
    pub(crate) at: f32,
    pub(crate) modules: Vec<crate::damage::FunctionRole>,
}

/// The scene: each field is read by the roots it names and ignored by the rest.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Scene {
    /// The sun's elevation, degrees, strictly between 0 and 90, on every root that lights the
    /// world (`sky::sun_elevation_deg`): the key light, its sky and the baked terrain shadow move
    /// together. The illuminance is not rebalanced: a lower sun reads darker. Default the shipped
    /// sun.
    pub(crate) sun_elevation_deg: Option<f32>,
    /// The armour sandbox's x-ray at start: `false` shows the target in its own surface. Default
    /// `true`.
    pub(crate) xray: bool,
    /// The armour sandbox's wall prototype ([`RangeWall`]).
    pub(crate) range_wall: Option<RangeWall>,
    /// The offline root's scripted demo ([`Demo`]).
    pub(crate) demo: Option<Demo>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            sun_elevation_deg: None,
            xray: true,
            range_wall: None,
            demo: None,
        }
    }
}

/// The armour sandbox's wall prototype as the run opens it (`sandbox::wall`).
#[derive(Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RangeWall {
    /// The one case the script fires ([`RangeWallCase`]); absent, the wall waits for its keys.
    pub(crate) case: Option<RangeWallCase>,
    /// The blast tool's charge without its casing.
    pub(crate) bare: bool,
    /// The script's last frames stand at the witness panel rather than the wall's front.
    pub(crate) witness: bool,
    /// The help panel open.
    pub(crate) help: bool,
}

/// What the wall script fires, in the game's environment destruction (game state only).
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) enum RangeWallCase {
    /// One of the wall's rounds, by its shipped key.
    Round(String),
    /// A burst of filler `kg` at `standoff_m` from the wall's front face.
    Blast { kg: f32, standoff_m: f32 },
}

/// A scripted demo on the offline root, which takes the player's tank or camera from the player.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) enum Demo {
    /// Side-by-side certified shoe LOD levels on the offline root (`lod_showcase`).
    LodShowcase,
    /// The hull marks, struck on the parked target (`hull_marks::demo`).
    HullMarks,
    /// The track prints, driven at world `(x, z)` on travel heading `heading_deg` (from `+Z`
    /// toward `+X`), pivoting at `steer` (default 1, right) (`track_prints::demo`).
    TrackPrints {
        x: f32,
        z: f32,
        heading_deg: f32,
        #[serde(default = "right")]
        steer: f32,
        /// Hold one view for good in place of the relief level cycle.
        #[serde(default)]
        hold: Option<DemoHold>,
    },
}

/// The track-print demo view held for good.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DemoHold {
    Drive,
    Pivot,
}

fn default_scale_factor() -> f32 {
    2.0
}

fn right() -> f32 {
    1.0
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
    /// Logical-to-physical UI ratio, finite and positive. ASSUMED default: retina-like 2.
    /// The board capture recipes also exercise the ordinary scale-1 window.
    #[serde(default = "default_scale_factor")]
    pub(crate) scale_factor: f32,
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
    pub(crate) step_s: Option<FrameStep>,
    /// Who places the player's camera ([`Camera`]). Default the player's own writers.
    #[serde(default)]
    pub(crate) camera: Camera,
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

/// One key read as pressed from app second `from` until app second `to` (`Time` as `Update`
/// sees it), `0 <= from < to`: pressed on the first frame inside the window and released on the
/// first frame past it, so a reader sees one press edge and one release edge.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyHold {
    /// Bevy's physical key code by its variant name (`Tab`, `KeyR`).
    pub(crate) key: KeyCode,
    pub(crate) from: f32,
    pub(crate) to: f32,
}

/// On the first frame at or after app second `at`, the shipped round `round` dresses the terrain
/// point under world `x, z` as a strike on `surface`: the game's damage-model view raises its
/// impact read there with no round flown and no shot recorded, so a plan presents what that round's
/// and that surface's data state.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Impact {
    pub(crate) x: f32,
    pub(crate) z: f32,
    pub(crate) round: String,
    pub(crate) surface: StruckSurface,
    pub(crate) at: f32,
}

/// What a raised strike finds: the painted ground under its point, or a shipped hard substance.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub(crate) enum StruckSurface {
    Terrain,
    Substance(String),
}

impl Impact {
    /// The contact read of the round as the first gun chambering it fires it, and the surface's
    /// response, from the shipped tables, or why a key names neither.
    pub(crate) fn resolved(
        &self,
    ) -> Result<(crate::rounds::ImpactStyle, crate::ballistics::ImpactSurface), String> {
        let style = crate::guns::GunRegistry::shipped()
            .fired_rounds()
            .find(|(_, key, _)| *key == self.round)
            .map(|(_, _, round)| round.impact_style())
            .ok_or_else(|| format!("impacts: round {:?}: no gun chambers it", self.round))?;
        let surface = match &self.surface {
            StruckSurface::Terrain => crate::ballistics::ImpactSurface::Terrain,
            StruckSurface::Substance(key) => crate::ballistics::ImpactSurface::Material(
                crate::substances::SubstanceRegistry::shipped_static()
                    .get(key)
                    .map_err(|error| format!("impacts: substance {key:?}: {error}"))?
                    .hard()
                    .ok_or_else(|| format!("impacts: substance {key:?} is not a hard substance"))?
                    .feedback,
            ),
        };
        Ok((style, surface))
    }
}

/// A capture's fixed frame step: positive, not rounding to nothing, and within what the virtual
/// clock takes in one update ([`Time::max_delta`] of a default [`Virtual`]). A step that rounds to
/// zero freezes the capture's clock and its deadline; the virtual clock clamps one past that
/// bound, so the frames would step by less than the plan states; a huge one panics `Instant`
/// arithmetic in `Time::<Real>::update_with_duration`. All are refused where the plan is read.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(try_from = "f64")]
pub(crate) struct FrameStep(pub(crate) std::time::Duration);

impl TryFrom<f64> for FrameStep {
    type Error = String;

    fn try_from(step: f64) -> Result<Self, String> {
        let most = Time::<Virtual>::default().max_delta();
        std::time::Duration::try_from_secs_f64(step)
            .ok()
            .filter(|duration| !duration.is_zero() && *duration <= most)
            .map(Self)
            .ok_or_else(|| {
                format!(
                    "frames.step_s: {step} is not a positive step of at most {} s, \
                     what the virtual clock takes in one update",
                    most.as_secs_f64()
                )
            })
    }
}

/// On the first frame at or after app second `at`, the shipped round `round` in fuze mode `mode`
/// is fired at a real construction or at the ground beside it, and the frames are timed from its burst.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(try_from = "ShotInput")]
pub(crate) struct Shot {
    pub(crate) key: String,
    pub(crate) round: crate::rounds::Round,
    pub(crate) target: ShotTarget,
    pub(crate) at: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShotInput {
    round: String,
    #[serde(default)]
    mode: u8,
    target: ShotTarget,
    at: f32,
}

impl TryFrom<ShotInput> for Shot {
    type Error = super::RoundCueError;

    fn try_from(input: ShotInput) -> Result<Self, Self::Error> {
        Ok(Self {
            round: super::resolve_round(&input.round, input.mode, "shot")?,
            key: input.round,
            target: input.target,
            at: input.at,
        })
    }
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShotTarget {
    Construction,
    Ground,
}

/// Shots at one house wall, each at its own second, and the constructions the offline root
/// places beside that house for a preview.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Wall {
    pub(crate) shots: Vec<WallShot>,
    /// Constructions placed beside the cue's house on the offline root alone, dev only: the shipped
    /// map never places them ([`Preview`]). Default none.
    #[serde(default)]
    pub(crate) preview: Option<Preview>,
}

/// A preview beside the cue's house (`dev_capture::wall`): what it places, and the camera that
/// frames it, `eye` looking at `look`, both metres in the house's frame. The plan's own
/// `frames.camera`, when it places one, wins.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preview {
    pub(crate) place: Vec<Placed>,
    pub(crate) eye: (f32, f32, f32),
    pub(crate) look: (f32, f32, f32),
}

/// One preview construction (`dev_capture::wall`): the shipped construction `asset`, named `id`,
/// standing `offset` metres from the cue's house root along its own x and z, on the ground there,
/// turned `yaw_deg` about +Y from the house's heading, at unit scale.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Placed {
    pub(crate) asset: String,
    pub(crate) id: String,
    pub(crate) offset: (f32, f32),
    pub(crate) yaw_deg: f32,
}

/// Where a shot goes instead of the cue's wall: at `at` in the frame of the construction whose
/// map instance is `instance`, the cue's house when absent (asset metres at its scale), arriving
/// along `toward` in that frame.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(try_from = "AimInput")]
pub(crate) struct Aim {
    pub(crate) instance: Option<String>,
    pub(crate) at: (f32, f32, f32),
    pub(crate) toward: Dir3,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AimInput {
    #[serde(default)]
    instance: Option<String>,
    at: (f32, f32, f32),
    toward: (f32, f32, f32),
}

impl TryFrom<AimInput> for Aim {
    type Error = String;

    fn try_from(input: AimInput) -> Result<Self, Self::Error> {
        if !Vec3::from(input.at).is_finite() {
            return Err("wall.shots.aim.at: author finite coordinates".into());
        }
        let repair = "wall.shots.aim.toward: author a finite nonzero direction with representable length; rescale its components to unit magnitude";
        let toward = Dir3::new(Vec3::from(input.toward)).map_err(|_| repair.to_owned())?;
        if !toward.as_vec3().is_normalized() {
            return Err(repair.into());
        }
        Ok(Self {
            instance: input.instance,
            at: input.at,
            toward,
        })
    }
}

impl Wall {
    fn validate_targets(&self) -> Result<(), String> {
        let map = std::cell::LazyCell::new(|| crate::map::load(&crate::assets::asset_root()));
        for name in self
            .shots
            .iter()
            .filter_map(|shot| shot.aim.as_ref()?.instance.as_deref())
        {
            let preview_count = self
                .preview
                .iter()
                .flat_map(|preview| &preview.place)
                .filter(|placed| placed.id == name)
                .count();
            let map_count = map
                .instances
                .iter()
                .filter(|instance| {
                    instance.id == name
                        && matches!(
                            map.prototypes[&instance.prototype],
                            crate::map::Prototype::Construction { .. }
                        )
                })
                .count();
            if preview_count + map_count != 1 {
                return Err(format!(
                    "wall.shots.aim.instance: {name:?} names {} constructions; name exactly one construction placed by the map or wall.preview, with a unique id",
                    preview_count + map_count
                ));
            }
        }
        Ok(())
    }
}

/// One shot at the wall, at app second `at`: a shipped round square at it, or a bare charge of
/// `kg` of the shipped burst core `core` (a `materials.ron` key) set off at its foot
/// (`dev_capture::wall`'s module doc).
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(try_from = "WallShotInput")]
pub(crate) struct WallShot {
    pub(super) what: super::wall::What,
    pub(super) at: f32,
    /// The shot's aim; `None` fires at the cue's wall.
    pub(super) aim: Option<Aim>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum WallShotInput {
    Round {
        round: String,
        #[serde(default)]
        mode: u8,
        at: f32,
        #[serde(default)]
        aim: Option<Aim>,
    },
    Charge {
        kg: f32,
        core: String,
        at: f32,
        #[serde(default)]
        aim: Option<Aim>,
    },
}

impl TryFrom<WallShotInput> for WallShot {
    type Error = super::wall::CueError;

    fn try_from(input: WallShotInput) -> Result<Self, Self::Error> {
        let (what, at, aim) = match input {
            WallShotInput::Round {
                round,
                mode,
                at,
                aim,
            } => (super::wall::What::round(&round, mode)?, at, aim),
            WallShotInput::Charge { kg, core, at, aim } => {
                (super::wall::What::charge(kg, &core)?, at, aim)
            }
        };
        Ok(Self { what, at, aim })
    }
}

/// Operator input is refused at admission; the startup boundary adds its file path.
#[derive(Debug)]
pub(crate) enum PlanError {
    Read(std::io::Error),
    Parse(ron::error::SpannedError),
    Field(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => {
                write!(f, "cannot read plan: {error}; fix the path or permissions")
            }
            Self::Parse(error) => error.fmt(f),
            Self::Field(error) => f.write_str(error),
        }
    }
}

/// Refuse operator input at either file admission or binding to the startup world.
pub(super) fn refuse(error: PlanError) -> ! {
    let path = PathBuf::from(
        std::env::var_os(VARIABLE).expect("a capture cue requires a loaded plan path"),
    );
    eprintln!("{VARIABLE}={}: {error}", path.display());
    std::process::exit(2);
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
    pub(crate) fn parse(text: &str) -> Result<Self, PlanError> {
        let mut plan: Self = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(PlanError::Parse)?;
        plan.validate().map_err(PlanError::Field)?;
        if let Some(frames) = &mut plan.frames {
            frames.at = super::normalize_times(std::mem::take(&mut frames.at));
        }
        Ok(plan)
    }

    fn load(path: &Path) -> Result<Self, PlanError> {
        let text = std::fs::read_to_string(path).map_err(PlanError::Read)?;
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
        if let Some(repair) = &self.repair {
            finite_at("repair.at", repair.at)?;
            if repair.modules.is_empty() {
                return Err("repair.modules: name at least one authored module function".into());
            }
        }
        if let Some(frames) = &self.frames {
            if frames
                .out
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_none_or(str::is_empty)
                || frames.out.extension().and_then(|ext| ext.to_str()) != Some("png")
            {
                return Err(
                    "frames.out: name a UTF-8 PNG file with a nonempty stem and .png extension"
                        .into(),
                );
            }
            if frames.at.is_empty() {
                return Err("frames.at: no frame time".into());
            }
            for &at in &frames.at {
                finite_at("frames.at", at)?;
            }
            if frames.size.0 == 0 || frames.size.1 == 0 {
                return Err(format!("frames.size: {:?} has a zero side", frames.size));
            }
            if !(frames.scale_factor.is_finite() && frames.scale_factor > 0.0) {
                return Err(format!(
                    "frames.scale_factor: {} is not finite and positive",
                    frames.scale_factor
                ));
            }
            if !(frames.size().as_vec2() / frames.scale_factor).is_finite() {
                return Err(
                    "frames.scale_factor: the requested logical target size is unrepresentable"
                        .into(),
                );
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
                    if !Vec3::from(offset).is_finite() || Vec3::from(offset) == Vec3::ZERO {
                        return Err("frames.camera: a follow offset is finite and nonzero".into());
                    }
                }
            }
        }
        for hold in &self.keys {
            finite_at("keys", hold.from)?;
            finite_at("keys", hold.to)?;
            if hold.from >= hold.to {
                return Err(format!(
                    "keys: {:?} held from {} s to {} s is an empty window",
                    hold.key, hold.from, hold.to
                ));
            }
        }
        for impact in &self.impacts {
            let numbers = [impact.x, impact.z, impact.at];
            if !numbers.iter().all(|n| n.is_finite()) {
                return Err(format!(
                    "impacts: {impact:?} needs finite coordinates and time"
                ));
            }
        }
        if let Some(shot) = &self.shot {
            finite_at("shot.at", shot.at)?;
        }
        let scene = &self.scene;
        if let Some(degrees) = scene.sun_elevation_deg
            && !(degrees > 0.0 && degrees < 90.0)
        {
            return Err(format!(
                "scene.sun_elevation_deg: {degrees} is not strictly between 0 and 90"
            ));
        }
        if let Some(RangeWall {
            case: Some(RangeWallCase::Blast { kg, standoff_m }),
            ..
        }) = scene.range_wall
            && !(kg.is_finite() && kg > 0.0 && standoff_m.is_finite() && standoff_m >= 0.0)
        {
            return Err(format!(
                "scene.range_wall.case: a blast of {kg} kg at {standoff_m} m"
            ));
        }
        if let Some(Demo::TrackPrints {
            x,
            z,
            heading_deg,
            steer,
            ..
        }) = scene.demo
            && ![x, z, heading_deg, steer].iter().all(|n| n.is_finite())
        {
            return Err("scene.demo: a track-print drive needs finite numbers".into());
        }
        if let Some(wall) = &self.wall {
            wall.validate_targets()?;
            if wall.shots.is_empty() {
                return Err("wall.shots: no shot".into());
            }
            for shot in &wall.shots {
                finite_at("wall.shots.at", shot.at)?;
            }
            if let Some(preview) = &wall.preview {
                let (eye, look) = (Vec3::from(preview.eye), Vec3::from(preview.look));
                if !(eye.is_finite() && look.is_finite() && eye != look) {
                    return Err("wall.preview: the camera is two distinct finite points".into());
                }
                for placed in &preview.place {
                    let (x, z) = placed.offset;
                    if !(x.is_finite() && z.is_finite() && placed.yaw_deg.is_finite()) {
                        return Err(format!(
                            "wall.preview {}: a finite offset and yaw",
                            placed.id
                        ));
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
/// Invalid operator input prints its path and cause and refuses startup with exit status 2.
pub(crate) fn plan() -> Option<&'static CapturePlan> {
    static PLAN: OnceLock<Option<CapturePlan>> = OnceLock::new();
    PLAN.get_or_init(|| {
        let path = PathBuf::from(std::env::var_os(VARIABLE)?);
        let plan = CapturePlan::load(&path).unwrap_or_else(|error| refuse(error));
        Some(plan)
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The game's damage model: Fixture charge inputs must be refused before a cue exists.
    fn charge_refusal(kg: &str, core: &str, field: &str, fix: &str) {
        let text = format!(r#"(wall: (shots: [Charge(kg: {kg}, core: "{core}", at: 1.0)]))"#);
        let error = CapturePlan::parse(&text).unwrap_err().to_string();
        assert!(error.contains(field) && error.contains(fix), "{error}");
    }

    /// The game's damage-model capture cues cannot defer unknown targets until their firing time.
    #[test]
    fn wall_aim_targets_are_admitted_before_cues_run() {
        let registry = crate::guns::GunRegistry::shipped();
        let (_, key, _) = registry.fired_rounds().next().unwrap();
        // Fixture: a target absent from both the selected map and the preview.
        let text = format!(
            r#"(wall: (shots: [Round(round: {key:?}, at: 0.0, aim: (instance: "missing capture construction", at: (0.0, 0.0, 0.0), toward: (1.0, 0.0, 0.0)))]))"#
        );
        let error = CapturePlan::parse(&text).unwrap_err().to_string();
        for required in [
            "wall.shots.aim.instance",
            "missing capture construction",
            "name exactly one construction",
        ] {
            assert!(error.contains(required), "{error}");
        }
    }

    /// The game's damage-model cues retain a checked direction; a finite-only gate admits
    /// underflow and overflow that would fail while firing or positioning a charge.
    #[test]
    fn wall_aim_directions_are_checked_once_at_admission() {
        let registry = crate::guns::GunRegistry::shipped();
        let (_, key, _) = registry.fired_rounds().next().unwrap();
        // Fixture: degenerate, nonfinite and unrepresentable lengths, and a valid nonunit vector.
        for toward in [
            "(0.0, 0.0, 0.0)",
            "(NaN, 0.0, 0.0)",
            "(inf, 0.0, 0.0)",
            "(1e-30, 0.0, 0.0)",
            "(1e30, 0.0, 0.0)",
            "(1e-22, 0.0, 0.0)",
        ] {
            let text = format!(
                r#"(wall: (shots: [Round(round: {key:?}, at: 0.0, aim: (at: (0.0, 0.0, 0.0), toward: {toward}))]))"#
            );
            let error = CapturePlan::parse(&text).unwrap_err().to_string();
            for required in ["wall.shots.aim.toward", "rescale"] {
                assert!(error.contains(required), "{error}");
            }
        }
        let text = format!(
            r#"(wall: (shots: [Round(round: {key:?}, at: 0.0, aim: (at: (0.0, 0.0, 0.0), toward: (8.0, 0.0, 0.0)))]))"#
        );
        let plan = CapturePlan::parse(&text).unwrap();
        assert_eq!(
            plan.wall.unwrap().shots[0].aim.as_ref().unwrap().toward,
            Dir3::X
        );
    }

    /// The game's damage model: operators need an unknown core refused at plan admission;
    /// deferring the lookup until cue setup would panic instead of returning this diagnostic.
    #[test]
    fn an_unknown_wall_charge_core_is_refused() {
        charge_refusal(
            "1.0",
            "missing capture core",
            "wall.shots.core",
            "name a materials.ron",
        );
    }

    /// The game's damage model: any material without burst facts is invalid as a charge core,
    /// even though it is a valid registry key for other callers.
    #[test]
    fn a_wall_charge_core_without_burst_facts_is_refused() {
        let registry = crate::substances::SubstanceRegistry::shipped_static();
        let core = registry
            .keys()
            .into_iter()
            .find(|key| registry.get(key).unwrap().burst().is_none())
            .unwrap();
        charge_refusal("1.0", core, "wall.shots.core", "burst facts");
    }

    /// The game's damage model: finite mass alone does not make a representable blast.
    /// Operators must receive a plan refusal before derived energy can panic during firing.
    #[test]
    fn a_wall_charge_whose_energy_overflows_is_refused() {
        let registry = crate::substances::SubstanceRegistry::shipped_static();
        let core = registry
            .keys()
            .into_iter()
            .find(|key| registry.get(key).unwrap().burst().is_some())
            .unwrap();
        let facts = registry.get(core).unwrap().burst().unwrap();
        // Fixture mass: twice the core's finite-energy limit, with a finite radius.
        let kg = (f32::MAX / facts.chemical_energy as f32) * 2.0;
        let radius = (3.0 * kg / (4.0 * std::f32::consts::PI * facts.density as f32)).cbrt();
        assert!(kg.is_finite() && radius.is_finite() && radius > 0.0);
        assert!((kg * facts.chemical_energy as f32).is_infinite());
        charge_refusal(&kg.to_string(), core, "wall.shots.kg", "energy");
    }

    /// The game's damage model: zero, nonfinite and underflowing Fixture masses cannot
    /// reach the cue as an incomplete charge; an overbroad finite-only check admits them.
    #[test]
    fn a_wall_charge_outside_simulation_units_is_refused() {
        let registry = crate::substances::SubstanceRegistry::shipped_static();
        let core = registry
            .keys()
            .into_iter()
            .find(|key| registry.get(key).unwrap().burst().is_some())
            .unwrap();
        for kg in ["0.0", "-1.0", "NaN", "inf"] {
            charge_refusal(kg, core, "wall.shots.kg", "finite positive mass");
        }
        charge_refusal("1e-45", core, "wall.shots.kg", "increase");
    }

    /// The game's damage model: wall plans carry resolved shots. An unknown Fixture key
    /// must be refused at admission before setup can panic.
    #[test]
    fn an_unknown_wall_round_is_refused() {
        let text = r#"(wall: (shots: [Round(round: "missing capture round", at: 1.0)]))"#;
        assert!(
            CapturePlan::parse(text)
                .unwrap_err()
                .to_string()
                .contains("wall.shots.round")
        );
    }

    /// The game's damage-model shot cue must refuse operator typos before setup and retain every
    /// authored mode. Fixture: an absent key; modes read their domain from the round's owner.
    #[test]
    fn shot_rounds_and_modes_are_resolved_at_admission() {
        let registry = crate::guns::GunRegistry::shipped();
        let (_, key, _) = registry.fired_rounds().next().unwrap();
        let error = CapturePlan::parse(
            r#"(shot: (round: "missing capture round", target: Construction, at: 0.0))"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("shot.round") && error.contains("name a round"),
            "{error}"
        );
        let weapon_round = crate::tank::WeaponRound::resolve(key).unwrap();
        for mode in 0..=u8::MAX {
            let text =
                format!("(shot: (round: {key:?}, mode: {mode}, target: Construction, at: 0.0))");
            match weapon_round.in_mode(mode) {
                Some(expected) => {
                    let shot = CapturePlan::parse(&text).unwrap().shot.unwrap();
                    assert_eq!(shot.key, key);
                    assert_eq!(shot.round, expected);
                }
                None => {
                    let error = CapturePlan::parse(&text).unwrap_err().to_string();
                    assert!(
                        error.contains("shot.mode") && error.contains("authored fuze modes"),
                        "{error}"
                    );
                }
            }
        }
    }

    /// The tracked inspection recipe must remain a valid input to the one scene-selection edge.
    #[test]
    fn the_lod_inspection_recipe_selects_the_showcase() {
        let plan = CapturePlan::parse(include_str!("../../scripts/capture/lod-showcase.ron"))
            .expect("the tracked showcase plan parses and validates");
        assert_eq!(plan.scene.demo, Some(Demo::LodShowcase));
        assert!(plan.frames.is_none());
    }

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
                ),
                keys: [(key: Tab, from: 2.0, to: 3.5)],
                impacts: [(x: 1.0, z: -2.0, round: "88mm_HE", surface: Substance("Wood"), at: 5.0)],
                shot: (round: "88mm_HE", mode: 1, target: Ground, at: 6.0),
                wall: (
                    shots: [
                        Round(round: "88mm_AP", at: 7.0),
                        Charge(kg: 2.0, core: "88mm_burst_core", at: 8.0),
                    ],
                ),
                scene: (
                    sun_elevation_deg: 12.5,
                    xray: false,
                    range_wall: (case: Blast(kg: 1.0, standoff_m: 3.0), bare: true, witness: true),
                    demo: TrackPrints(x: 70.0, z: -20.0, heading_deg: 90.0, hold: Pivot),
                ),
            )"#,
        )
        .expect("a valid plan");
        let frames = plan.frames.expect("frames");
        assert_eq!(frames.out, PathBuf::from("out/f.png"));
        assert_eq!(frames.at, vec![9.0, 12.0]);
        assert_eq!(frames.size(), UVec2::new(640, 360));
        assert!(!frames.ui && !frames.gizmos && frames.sight);
        assert_eq!(
            frames.step_s,
            Some(FrameStep(std::time::Duration::from_secs_f64(0.0078125)))
        );
        assert_eq!(
            frames.camera,
            Camera::Pose {
                eye: (1.0, 2.0, 3.0),
                look: (0.0, 0.0, 0.0),
            }
        );
        assert_eq!(
            plan.keys,
            vec![KeyHold {
                key: KeyCode::Tab,
                from: 2.0,
                to: 3.5,
            }]
        );
        assert_eq!(
            plan.impacts,
            vec![Impact {
                x: 1.0,
                z: -2.0,
                round: "88mm_HE".into(),
                surface: StruckSurface::Substance("Wood".into()),
                at: 5.0,
            }]
        );
        assert_eq!(
            plan.shot,
            Some(Shot {
                key: "88mm_HE".into(),
                round: super::super::resolve_round("88mm_HE", 1, "shot").unwrap(),
                target: ShotTarget::Ground,
                at: 6.0,
            })
        );
        assert_eq!(
            plan.wall,
            Some(Wall {
                shots: vec![
                    WallShot {
                        what: super::super::wall::What::round("88mm_AP", 0).unwrap(),
                        at: 7.0,
                        aim: None,
                    },
                    WallShot {
                        what: super::super::wall::What::charge(2.0, "88mm_burst_core").unwrap(),
                        at: 8.0,
                        aim: None,
                    },
                ],
                preview: None,
            })
        );
        assert_eq!(
            plan.scene,
            Scene {
                sun_elevation_deg: Some(12.5),
                xray: false,
                range_wall: Some(RangeWall {
                    case: Some(RangeWallCase::Blast {
                        kg: 1.0,
                        standoff_m: 3.0,
                    }),
                    bare: true,
                    witness: true,
                    help: false,
                }),
                demo: Some(Demo::TrackPrints {
                    x: 70.0,
                    z: -20.0,
                    heading_deg: 90.0,
                    steer: 1.0,
                    hold: Some(DemoHold::Pivot),
                }),
            }
        );
    }

    /// A PLAN NAMES ONLY WHAT IT CHANGES: a bare `out` takes one frame at six seconds, at the
    /// documented size, with the UI and gizmos drawn, the player's camera, no cue and the shipped
    /// scene (the x-ray on); an empty plan takes no frame. A changed default silently changes every
    /// plan that leans on it.
    #[test]
    fn a_minimal_plan_takes_the_documented_defaults() {
        let plan = CapturePlan::parse(r#"(frames: (out: "f.png"))"#).expect("a valid plan");
        let frames = plan.frames.as_ref().expect("frames");
        assert_eq!(frames.at, vec![6.0]);
        assert_eq!(frames.size(), UVec2::new(2560, 1440));
        assert_eq!(frames.scale_factor, 2.0);
        assert!(frames.ui && frames.gizmos && !frames.sight);
        assert_eq!((frames.step_s, frames.camera), (None, Camera::Player));
        assert_eq!(
            (plan.impacts.is_empty(), &plan.shot, &plan.wall),
            (true, &None, &None)
        );
        assert!(plan.keys.is_empty());
        let scene = &plan.scene;
        assert!(scene.xray);
        assert_eq!(
            (scene.sun_elevation_deg, &scene.range_wall, scene.demo),
            (None, &None, None)
        );
        let wall = CapturePlan::parse("(scene: (range_wall: ()))").expect("a valid plan");
        assert_eq!(wall.scene.range_wall, Some(RangeWall::default()));
        let scene = CapturePlan::parse("()").expect("an empty plan");
        assert_eq!(scene.frames, None);
    }

    /// The board's capture recipes must preserve their requested UI geometry; invalid outside
    /// input fails at plan admission, rather than publishing a misleading or unusable image.
    #[test]
    fn capture_scale_is_explicit_and_refused_outside_its_domain() {
        for scale in [1.0, 2.0] {
            let text = format!("(frames: (out: \"board.png\", scale_factor: {scale}))");
            assert_eq!(
                CapturePlan::parse(&text)
                    .unwrap()
                    .frames
                    .unwrap()
                    .scale_factor,
                scale
            );
        }
        for scale in ["0.0", "-1.0", "NaN", "inf", "1e-40"] {
            let text = format!("(frames: (out: \"board.png\", scale_factor: {scale}))");
            assert!(
                CapturePlan::parse(&text)
                    .unwrap_err()
                    .to_string()
                    .contains("scale_factor")
            );
        }
    }

    /// A PLAN THAT CANNOT MEAN WHAT IT SAYS FAILS THE BOOT, NAMING ITS FIELD (ADR-0011): a typo, a
    /// missing path, an empty or negative time, a zero side, a non-positive step, a degenerate
    /// pose, an empty or negative key window, an unknown key, a non-positive calibre or charge, a
    /// sun at or past the zenith, an unknown demo. Each was a silent default or a skipped cue under the variables
    /// the plan replaced.
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
            (r#"(frames: (out: "frame"))"#.to_owned(), "frames.out"),
            (r#"(frames: (out: "frame.jpg"))"#.to_owned(), "frames.out"),
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
                r#"(frames: (out: "f.png", step_s: 1e300))"#.to_owned(),
                "frames.step_s",
            ),
            (
                r#"(frames: (out: "f.png", step_s: 1e19))"#.to_owned(),
                "frames.step_s",
            ),
            (
                format!(
                    r#"(frames: (out: "f.png", step_s: {}))"#,
                    Time::<Virtual>::default().max_delta().as_secs_f64() * 1.01
                ),
                "frames.step_s",
            ),
            (
                r#"(frames: (out: "f.png", step_s: 1e-12))"#.to_owned(),
                "frames.step_s",
            ),
            (
                r#"(frames: (out: "f.png", step_s: -1.0))"#.to_owned(),
                "frames.step_s",
            ),
            (
                r#"(frames: (out: "f.png", step_s: NaN))"#.to_owned(),
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
                r#"(impacts: [(x: inf, z: 0.0, round: "88mm_AP", surface: Terrain, at: 1.0)])"#
                    .to_owned(),
                "impacts",
            ),
            (
                "(keys: [(key: Tab, from: 3.0, to: 3.0)])".to_owned(),
                "keys",
            ),
            (
                "(keys: [(key: Tab, from: -1.0, to: 3.0)])".to_owned(),
                "keys",
            ),
            (
                "(keys: [(key: Tabb, from: 1.0, to: 3.0)])".to_owned(),
                "Tabb",
            ),
            (
                r#"(shot: (round: "88mm_AP", target: Construction, at: -2.0))"#.to_owned(),
                "shot.at",
            ),
            ("(wall: (shots: []))".to_owned(), "wall.shots"),
            (
                r#"(wall: (shots: [Charge(kg: 0.0, core: "88mm_burst_core", at: 1.0)]))"#
                    .to_owned(),
                "wall.shots",
            ),
            (
                "(wall: (shots: [Charge(kg: 1.0, at: 1.0)]))".to_owned(),
                "core",
            ),
            (
                "(scene: (sun_elevation_deg: 90.0))".to_owned(),
                "scene.sun_elevation_deg",
            ),
            (
                "(scene: (range_wall: (case: Blast(kg: 0.0, standoff_m: 1.0))))".to_owned(),
                "scene.range_wall.case",
            ),
            ("(scene: (demo: Spin))".to_owned(), "Spin"),
        ] {
            let error = CapturePlan::parse(&text)
                .expect_err("the plan is refused")
                .to_string();
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

    /// The wood preview the owner judges is an admitted plan: every shot aimed by data, every
    /// preview asset a shipped construction the map's load gate admits, and a camera of two
    /// distinct points. An aim reverted to the wall's own choice, or a stray asset, fails here.
    #[test]
    fn the_wood_preview_plan_aims_every_shot_at_admitted_constructions() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/capture/wood-preview.ron");
        let plan = CapturePlan::load(&path).unwrap_or_else(|error| panic!("{error}"));
        let wall = plan.wall.expect("the preview is a wall cue");
        assert!(wall.shots.iter().all(|shot| shot.aim.is_some()));
        let preview = wall.preview.expect("the preview places constructions");
        let assets: Vec<&str> = preview.place.iter().map(|p| p.asset.as_str()).collect();
        let library = crate::construction::ConstructionLibrary::load(
            &crate::map::tests::shipped_assets(),
            assets.iter().copied(),
        );
        assert!(assets.iter().all(|asset| library.get(asset).is_some()));
        for named in wall
            .shots
            .iter()
            .filter_map(|shot| shot.aim.as_ref()?.instance.as_ref())
        {
            assert!(preview.place.iter().any(|p| &p.id == named), "{named}");
        }
    }

    /// A raised strike reads its style and its surface from the shipped tables, so a capture
    /// presents what the data states; a key naming neither is refused, never read as a default.
    /// Mutant: a cue that ignores its round or its substance fails here.
    #[test]
    fn a_raised_strike_resolves_its_round_and_surface_from_the_shipped_tables() {
        let cue = |round: &str, surface| Impact {
            x: 0.0,
            z: 0.0,
            round: round.into(),
            surface,
            at: 0.0,
        };
        let key = "88mm_AP";
        let style = crate::guns::GunRegistry::shipped()
            .fired_rounds()
            .find(|(_, round, _)| *round == key)
            .expect("a shipped gun chambers the round")
            .2
            .impact_style();
        let substance = "Wood";
        let feedback = crate::substances::SubstanceRegistry::shipped_static()
            .get(substance)
            .expect("a shipped substance")
            .hard()
            .expect("a hard substance")
            .feedback;
        assert_eq!(
            cue(key, StruckSurface::Substance(substance.into())).resolved(),
            Ok((style, crate::ballistics::ImpactSurface::Material(feedback)))
        );
        assert_eq!(
            cue(key, StruckSurface::Terrain).resolved(),
            Ok((style, crate::ballistics::ImpactSurface::Terrain))
        );
        assert!(
            cue("no such round", StruckSurface::Terrain)
                .resolved()
                .is_err()
        );
        assert!(
            cue(key, StruckSurface::Substance("no such substance".into()))
                .resolved()
                .is_err()
        );
    }
}
