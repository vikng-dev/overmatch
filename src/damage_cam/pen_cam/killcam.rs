//! The victim kill cam: what the struck player sees when a record knocks their own tank out. Four
//! beats on the run's clock, counted from the killing shot's world impact ([`Beats`]):
//!
//! 1. **Hit** — the player's own orbit view, free, the world showing the hit in real time.
//! 2. **Lesson** — captive: the look eases to a three-quarter from the shot's side; the main camera
//!    swaps to [`CameraProfile::KillCamLesson`], so the tank it rides leaves the view and the pen
//!    cam stage, which sits on it in the record's frame, draws the record over the live world at
//!    [`LESSON_RATE`] of the corner's tempo. It runs until the record has settled and held
//!    ([`lesson_length`]; `demo/ux-spec.md` §3, "Time"). The world clock is untouched.
//! 3. **Killer** — captive: back on the victim's own orbit at the play radius, the look turns
//!    toward the shooter and tracks it, and the killer card shows name, vehicle, range and round.
//! 4. **Death** — free: the orbit about the victim, opening on the shooter where beat 3 played,
//!    with the death tint under the UI, the death panel and the respawn prompt.
//!
//! The kill cam never shows more of the enemy than the victim could see from where they died:
//! every pose orbits the victim's own pivot at the game's play radius, with no zoom, so beat 3
//! frames the shooter from the victim's place alone.
//!
//! Beat 3 plays only for a shooter the kill cam stages: a record that names none, a shooter whose
//! tank file or scene fails to load, or one not staged within [`SHOOTER_WAIT_S`] skips it, and the
//! death screen follows the lesson with the killer card showing what the record names.
//!
//! Respawn is live from [`HIT_S`] after the impact. Every pose is the game's own orbit
//! ([`crate::camera::orbit_rotation`], [`crate::camera::orbit_body`],
//! [`crate::camera::orbit_pivot`]) about the victim's pivot. Which entries play it: a victim entry
//! whose record knocks the tank out by the game's own rule ([`knockout`]). Plan and owner record:
//! `.agents/scratch/damage-cam-2026-10-08/demo/killcam-plan.md`; the shooter framing's open
//! alternative: `.agents/scratch/playtest-forks/killcam-shooter-framing.md`. Every duration, angle
//! and colour here is TUNED (a look-test choice).

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::demo::{Library, PLAYLIST, Run, Settings};
use super::plan::Plan;
use super::stage;
use super::timeline::{self, CornerKind};
use super::view::{self, ViewRoot, Views};
use super::world::MainCamera;
use super::{Showing, Surface};
use crate::camera::{ORBIT_FAR, orbit_body, orbit_pivot, orbit_rotation};
use crate::damage::{KnockoutReason, knockout_from_counts};
use crate::damage_cam::{ConsequenceState, PenetrationReport, Role, Shooter};
use crate::overlay::Overlay;
use crate::render_policy::{CameraProfile, VisualScope};
use crate::spec::{TankSpec, ViewKind};
use crate::ui_font::{PANEL_BG, TEXT, TEXT_DIM, UiFonts, drawable};

/// Beat 1 runs this long after the world impact; respawn is live from its end.
pub const HIT_S: f32 = 0.5;
/// The look eases into the lesson's bearing over this; every camera move here runs rest to rest.
const MOVE_S: f32 = 0.6;
/// The lesson's shortest length (real seconds).
pub const LESSON_S: f32 = 3.6;
/// The record's tempo in the lesson, as a fraction of the corner's.
pub const LESSON_RATE: f32 = 0.5;
/// The lesson holds on the settled record this long (real seconds) before it ends.
pub const LESSON_HOLD_S: f32 = 1.0;
/// Beat 3's length: the look's turn toward the shooter over [`MOVE_S`], then the track.
const KILLER_S: f32 = 3.0;
/// The demo's run ends this long into the death screen.
const DEATH_HOLD_S: f32 = 2.0;
/// The longest a kill run's clock holds for its shooter to stage, before beat 3 is dropped.
pub const SHOOTER_WAIT_S: f32 = 3.0;
/// The lesson's bearing: yawed off the shot's side, the body above the pivot by this slope (the
/// corner's three-quarter).
const LESSON_YAW_DEG: f32 = 55.0;
const LESSON_SLOPE: f32 = 0.5;
/// The player's look at the hit, before any input: along the hull's forward, looking down. A
/// staging choice; in play it is wherever the player was looking.
const PLAYER_PITCH_DEG: f32 = 12.0;
/// The killer card's enter, and the prompts' strength before and at the death screen.
pub(super) const CARD_ENTER_S: f32 = 0.18;
const PROMPT_DIM: f32 = 0.6;
/// The card's accent bar.
const ACCENT_PX: f32 = 3.0;

/// Who fired the killing round, as the victim's copy of the record names them: each part only
/// where the record and the files it names supply it, so a card shows what is known.
#[derive(Clone, Debug, PartialEq)]
pub struct Killer {
    /// The shooter's name, where the record names a shooter.
    pub player: Option<String>,
    /// The shooter's vehicle as its tank file names it to players, where that file reads.
    pub vehicle: Option<String>,
    /// The round's display name and muzzle speed (m/s), where the registry has the round; never
    /// its registry key.
    pub round: Option<(String, f32)>,
}

impl Killer {
    /// The killer of a record fired by `shooter` with the round `round` (a round registry key):
    /// the vehicle read from the shooter's own tank file, only where there is a shooter, and the
    /// round's display name and muzzle speed from the registry. A tank file that does not read or
    /// a round the registry lacks leaves that part unknown.
    pub fn of(shooter: Option<&Shooter>, round: &str) -> Self {
        Self {
            player: shooter.map(|shooter| shooter.name.clone()),
            ..Self::of_vehicle(shooter.map(|shooter| shooter.vehicle.as_str()), round)
        }
    }

    /// The killer with no name, fired from the tank file `vehicle` (relative to the asset root),
    /// read as [`Self::of`] reads it.
    pub fn of_vehicle(vehicle: Option<&str>, round: &str) -> Self {
        let vehicle = vehicle.and_then(|vehicle| {
            let path = crate::assets::asset_root().join(vehicle);
            let spec = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| ron::de::from_str::<TankSpec>(&text).map_err(|e| e.to_string()));
            spec.inspect_err(|e| warn!("kill cam: the shooter's tank file {}: {e}", path.display()))
                .ok()
                .map(|spec| spec.display_name)
        });
        let round = crate::rounds::RoundRegistry::shipped()
            .name(round)
            .map_err(crate::guns::GunError::Round)
            .and_then(|name| {
                let fired = crate::guns::GunRegistry::shipped().round_by_key(round)?;
                Ok((name.to_owned(), fired.muzzle_speed))
            })
            .inspect_err(|e| warn!("kill cam: the round {round}: {e}"))
            .ok();
        Self {
            player: None,
            vehicle,
            round,
        }
    }
}

/// Whether `report` knocks the struck tank out, by the game's own rule over the record: its crew
/// (lost before the shot or by it) and any cookoff.
pub fn knockout(report: &PenetrationReport) -> Option<KnockoutReason> {
    let lost = |name: &str| {
        report
            .consequences
            .iter()
            .any(|c| c.volume == name && c.state == ConsequenceState::Lost)
    };
    let crew: Vec<_> = report
        .components
        .iter()
        .filter(|component| component.role == Role::Crew)
        .collect();
    let living = crew
        .iter()
        .filter(|component| component.hp > 0 && !lost(&component.name))
        .count();
    let cooked = report
        .consequences
        .iter()
        .any(|c| c.state == ConsequenceState::Cookoff);
    knockout_from_counts(crew.len() as u32, living as u32, cooked)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beat {
    Hit,
    Lesson,
    Killer,
    Death,
}

impl Beat {
    /// Whether the camera is the kill cam's, not the player's.
    #[cfg(test)]
    pub fn captive(self) -> bool {
        matches!(self, Self::Lesson | Self::Killer)
    }
}

/// The lesson's length (real seconds) for the record of `plan`: until the record has settled in
/// the glass at [`LESSON_RATE`] ([`stage::pace`]) and held [`LESSON_HOLD_S`] on it, never under
/// [`LESSON_S`]; that minimum when the stage has no plan to draw.
pub fn lesson_length(plan: Option<&Plan>) -> f32 {
    plan.map_or(LESSON_S, |plan| {
        let settle = stage::pace(plan, CornerKind::Glass).settle;
        LESSON_S.max(settle / LESSON_RATE + LESSON_HOLD_S)
    })
}

/// The sequence's beats in run seconds, from the killing shot's world impact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beats {
    pub impact: f32,
    /// Whether beat 3 plays: only for a shooter the kill cam can stage. Without one the death
    /// screen follows the lesson.
    pub shooter: bool,
    /// The lesson's length (real seconds): [`lesson_length`] of the record it draws.
    pub lesson_s: f32,
}

impl Beats {
    pub fn lesson(self) -> f32 {
        self.impact + HIT_S
    }
    pub fn killer(self) -> f32 {
        self.lesson() + self.lesson_s
    }
    /// Where the killer card enters: the look's arrival on the shooter, or the death screen's
    /// start without beat 3.
    pub fn arrive(self) -> f32 {
        if self.shooter {
            self.killer() + MOVE_S
        } else {
            self.killer()
        }
    }
    pub fn death(self) -> f32 {
        if self.shooter {
            self.killer() + KILLER_S
        } else {
            self.killer()
        }
    }
    /// Where the demo's run ends.
    pub fn end(self) -> f32 {
        self.death() + DEATH_HOLD_S
    }

    pub fn beat(self, t: f32) -> Beat {
        if t < self.lesson() {
            Beat::Hit
        } else if t < self.killer() {
            Beat::Lesson
        } else if t < self.death() {
            Beat::Killer
        } else {
            Beat::Death
        }
    }

    pub fn respawn_live(self, t: f32) -> bool {
        t >= self.lesson()
    }

    /// The record's corner time in the lesson.
    pub fn lesson_c(self, t: f32) -> Option<f32> {
        (self.beat(t) == Beat::Lesson).then(|| (t - self.lesson()) * LESSON_RATE)
    }
}

/// The turn index the lesson's stage is keyed by: no corner turn has it.
const LESSON_TURN: usize = usize::MAX - 1;

/// The stage's record and clock in the lesson: the glass, drawn on the world.
pub fn lesson_showing(beats: Beats, t: f32, record: usize) -> Option<Showing> {
    beats.lesson_c(t).map(|c| Showing {
        record,
        turn: LESSON_TURN,
        kind: CornerKind::Glass,
        c,
        opacity: 1.0,
        scale: 1.0,
        remaining: 1.0,
        surface: Surface::World,
    })
}

/// What the kill cam's orbit is placed from, in the battlefield's frame.
#[derive(Clone, Copy, Debug)]
pub struct Rig {
    pub victim_pivot: Vec3,
    /// What the look turns toward in beat 3; never an orbit centre. `None` where no shooter is
    /// staged ([`Beats::shooter`]).
    pub shooter_pivot: Option<Vec3>,
    pub impact: Vec3,
    pub muzzle: Vec3,
    /// The orbit's radius: the game's at its spawn zoom.
    pub radius: f32,
    pub ground_y: f32,
}

/// A look whose view axis is `forward`.
fn look_along(forward: Vec3) -> Quat {
    Transform::IDENTITY.looking_to(forward, Vec3::Y).rotation
}

fn forward_of(look: Quat) -> Dir3 {
    Dir3::new(look * Vec3::NEG_Z).unwrap_or(Dir3::NEG_Z)
}

impl Rig {
    /// The round's travel, shooter to victim.
    fn incoming(&self) -> Vec3 {
        (self.impact - self.muzzle).normalize_or(Vec3::NEG_Z)
    }

    fn incoming_flat(&self) -> Vec3 {
        let d = self.incoming();
        Vec3::new(d.x, 0.0, d.z).normalize_or(Vec3::NEG_Z)
    }

    /// The player's look at the hit, before any input.
    pub fn player_look() -> Quat {
        let down = PLAYER_PITCH_DEG.to_radians();
        look_along(Vec3::new(0.0, -down.sin(), -down.cos()))
    }

    /// The lesson's look: a three-quarter from the shot's side, drifted by `drift` (rad).
    fn lesson_look(&self, drift: f32) -> Quat {
        let side =
            Quat::from_rotation_y(LESSON_YAW_DEG.to_radians() + drift) * -self.incoming_flat();
        look_along(-(side + Vec3::Y * LESSON_SLOPE))
    }

    /// From the victim's pivot toward the shooter's where one is staged: the look the victim's own
    /// orbit takes to see the shooter, recomputed from the rig each frame so it tracks the shooter
    /// wherever the stage places it.
    fn shooter_look(&self) -> Option<Quat> {
        self.shooter_pivot
            .map(|shooter| look_along((shooter - self.victim_pivot).normalize_or(Vec3::NEG_Z)))
    }

    /// The lesson's drift (rad) at run time `t`.
    fn drift(beats: Beats, t: f32, reduce: bool) -> f32 {
        if reduce {
            0.0
        } else {
            timeline::ORBIT_DEG_S.to_radians() * (t - beats.lesson()).max(0.0)
        }
    }

    /// The death screen's opening look: toward the shooter, or without one the lesson's look as the
    /// lesson ends.
    pub fn death_look(&self, beats: Beats, reduce: bool) -> Quat {
        self.shooter_look()
            .unwrap_or_else(|| self.lesson_look(Self::drift(beats, beats.killer(), reduce)))
    }

    /// The game's orbit body for `pivot` and `look`, pulled in by the flat ground.
    pub fn place(&self, pivot: Vec3, look: Quat) -> Transform {
        let ground_y = self.ground_y;
        let body = orbit_body(pivot, forward_of(look), self.radius, |ray, max| {
            let down = ray.direction.y;
            if down < -1e-6 {
                let hit = (ground_y - ray.origin.y) / down;
                if hit >= 0.0 {
                    return hit.min(max);
                }
            }
            max
        });
        Transform::from_translation(body).with_rotation(look)
    }

    /// The pivot and look at run time `t`, given the player's free looks: `hit` in beat 1, `death`
    /// in beat 4.
    pub fn orbit(
        &self,
        beats: Beats,
        t: f32,
        hit: Quat,
        death: Quat,
        reduce: bool,
    ) -> (Vec3, Quat) {
        self.orbit_drifting(beats, t, t, hit, death, reduce)
    }

    /// [`Self::orbit`] with the lesson's drift taken at `drift_t` rather than `t`: a held clock's
    /// orbit keeps drifting.
    pub fn orbit_drifting(
        &self,
        beats: Beats,
        t: f32,
        drift_t: f32,
        hit: Quat,
        death: Quat,
        reduce: bool,
    ) -> (Vec3, Quat) {
        let drift = |at: f32| Self::drift(beats, at + drift_t - t, reduce);
        match beats.beat(t) {
            Beat::Hit => (self.victim_pivot, hit),
            Beat::Lesson => {
                let w = timeline::ease_standard(timeline::progress(t, beats.lesson(), MOVE_S));
                (self.victim_pivot, hit.slerp(self.lesson_look(drift(t)), w))
            }
            Beat::Killer => {
                let w = timeline::ease_standard(timeline::progress(t, beats.killer(), MOVE_S));
                let from = self.lesson_look(drift(beats.killer()));
                (
                    self.victim_pivot,
                    from.slerp(self.shooter_look().unwrap_or(from), w),
                )
            }
            Beat::Death => (self.victim_pivot, death),
        }
    }

    pub fn pose(&self, beats: Beats, t: f32, hit: Quat, death: Quat, reduce: bool) -> Transform {
        let (pivot, look) = self.orbit(beats, t, hit, death, reduce);
        self.place(pivot, look)
    }
}

/// The kill cam's state: the player's free looks, the staged shooter, and the rig placed from the
/// scene.
#[derive(Resource)]
pub struct KillCam {
    /// The player's look in beat 1.
    pub hit: Quat,
    /// The player's look in beat 4; `None` until the beat opens on the shooter's framing.
    pub death: Option<Quat>,
    /// The entry it last staged, to reset the looks on a new one.
    staged: Option<usize>,
    /// The shooter's view, of the vehicle the killing record names.
    shooter: Option<ShooterView>,
    /// Where the orbit is placed from this frame, once the scene is staged.
    pub rig: Option<Rig>,
    /// The orbit's vertical field (rad): the victim's riding crewman's naked eye, as in play.
    pub fov: f32,
    /// Seconds the run's clock has held for the stage on this entry.
    waited: f32,
}

impl Default for KillCam {
    fn default() -> Self {
        Self {
            hit: Rig::player_look(),
            death: None,
            staged: None,
            shooter: None,
            rig: None,
            fov: std::f32::consts::FRAC_PI_4,
            waited: 0.0,
        }
    }
}

/// The shooter's tank on the battlefield: its view root, the tank file it was loaded from, its
/// scene, and the root under its hull node (`hull pose`⁻¹) once that node exists.
struct ShooterView {
    root: Entity,
    vehicle: String,
    spec: Handle<TankSpec>,
    scene: Handle<WorldAsset>,
    hull_inverse: Option<Mat4>,
}

impl ShooterView {
    fn failed(&self, assets: &AssetServer) -> bool {
        assets.load_state(&self.spec).is_failed() || assets.load_state(&self.scene).is_failed()
    }
}

/// The shooter's tank file and its view glb, as the record names them.
pub fn shooter_scene(report: &PenetrationReport) -> Option<(String, String)> {
    let vehicle = &report.shooter.as_ref()?.vehicle;
    Some((vehicle.clone(), view::view_glb(vehicle)))
}

/// The killer card's three data lines, each reduced to what the font draws and named by what is
/// missing where the killer has nothing legible; the range only where the record has the
/// shooter's muzzle (`range_m`).
pub fn card_lines(killer: &Killer, range_m: Option<f32>) -> [String; 3] {
    let shown = |text: Option<&str>, missing: &str| {
        text.and_then(drawable)
            .unwrap_or_else(|| missing.to_owned())
    };
    let round = match &killer.round {
        Some((name, speed)) => format!("{}    {speed:.0} m/s", shown(Some(name), "UNKNOWN ROUND")),
        None => "UNKNOWN ROUND".to_owned(),
    };
    [
        shown(killer.player.as_deref(), "UNKNOWN PLAYER"),
        shown(killer.vehicle.as_deref(), "UNKNOWN VEHICLE"),
        match range_m {
            Some(range_m) => format!("{range_m:.0} m    {round}"),
            None => round,
        },
    ]
}

#[derive(Component)]
pub(super) struct Card;

#[derive(Component)]
pub(super) struct CardAccent;

#[derive(Component)]
pub(super) struct CardText;

#[derive(Component)]
pub(super) struct DeathPanel;

#[derive(Component)]
pub(super) struct DeathText;

#[derive(Component)]
pub(super) struct Prompt;

/// The death screen's wash over the world: shown from the kill cam's handoff to the death screen
/// or the respawn, never during the kill cam's own beats.
#[derive(Component)]
pub(super) struct DeathTint;

/// A text row's place in its block.
#[derive(Component)]
pub(super) struct Index(pub(super) usize);

/// The respawn stand-in: after `R`, the death panel reads this for [`RESPAWN_S`], then the run
/// starts a fresh life.
pub const RESPAWNING: &str = "RESPAWNING";
pub const RESPAWN_S: f32 = 0.8;

/// A text style of the kill cam's UI.
fn text(font: &Handle<Font>, px: f32, color: Color) -> (TextFont, TextColor) {
    (
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(px),
            ..default()
        },
        TextColor(color),
    )
}

pub fn spawn(mut commands: Commands, fonts: Res<UiFonts>) {
    spawn_death_tint(&mut commands);
    spawn_card(&mut commands, &fonts);
    spawn_death_panel(&mut commands, &fonts);
    spawn_prompt(&mut commands, &fonts, "R  RESPAWN");
}

/// The death tint: the whole screen, on the bottom UI rung so it washes the world alone; hidden.
pub(super) fn spawn_death_tint(commands: &mut Commands) -> Entity {
    commands
        .spawn((crate::overlay::death_tint(), Visibility::Hidden, DeathTint))
        .id()
}

/// The killer card, lower left, hidden: clear of the shooter the look centres in beat 3.
pub(super) fn spawn_card(commands: &mut Commands, fonts: &UiFonts) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(48.0),
                bottom: Val::Px(150.0),
                ..default()
            },
            GlobalZIndex(Overlay::PEN_CAM_CORNER_Z),
            Visibility::Hidden,
            UiTransform::from_scale(Vec2::splat(0.96)),
            Card,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::axes(Val::Px(18.0), Val::Px(12.0)),
                    min_width: Val::Px(320.0),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(PANEL_BG),
            ))
            .with_children(|card| {
                card.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(0.0),
                        left: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        height: Val::Px(ACCENT_PX),
                        ..default()
                    },
                    BackgroundColor(Color::WHITE),
                    CardAccent,
                ));
                for (index, (font, px, color)) in [
                    (&fonts.hud, 13.0, TEXT_DIM),
                    (&fonts.hud, 26.0, TEXT),
                    (&fonts.body, 18.0, TEXT),
                    (&fonts.body, 16.0, TEXT_DIM),
                ]
                .into_iter()
                .enumerate()
                {
                    card.spawn((Text::new(""), text(font, px, color), CardText, Index(index)));
                }
            });
        })
        .id()
}

/// The death panel, centre, over the live world, hidden.
pub(super) fn spawn_death_panel(commands: &mut Commands, fonts: &UiFonts) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Percent(30.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            },
            GlobalZIndex(Overlay::PEN_CAM_CORNER_Z),
            Visibility::Hidden,
            DeathPanel,
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new(""),
                text(&fonts.hud, 48.0, TEXT),
                DeathText,
                Index(0),
            ));
            panel.spawn((
                Text::new(""),
                text(&fonts.body, 20.0, TEXT_DIM),
                DeathText,
                Index(1),
            ));
        })
        .id()
}

/// A prompt reading `label`, bottom-centre, hidden.
pub(super) fn spawn_prompt(commands: &mut Commands, fonts: &UiFonts, label: &str) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                bottom: Val::Px(96.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(Overlay::PEN_CAM_CORNER_Z),
            Visibility::Hidden,
        ))
        .with_children(|root| {
            root.spawn((Text::new(label), text(&fonts.hud, 20.0, TEXT), Prompt));
        })
        .id()
}

/// Whether the kill cam owns the main camera: a kill run whose rig is staged. Until the victim's
/// view is placed and, on a run with beat 3, the shooter's scene and tank file load, there is no
/// rig, the world keeps the camera, and the run's clock holds ([`holding`]); [`stage`] drops beat
/// 3 rather than hold past [`SHOOTER_WAIT_S`] or a failed load.
pub fn active(run: Res<Run>, cam: Res<KillCam>) -> bool {
    run.kill.is_some() && cam.rig.is_some()
}

/// Whether the world owns the main camera.
pub fn inactive(run: Res<Run>, cam: Res<KillCam>) -> bool {
    !active(run, cam)
}

/// Whether a kill run's clock waits for its stage.
pub fn holding(run: &Run, cam: &KillCam) -> bool {
    run.kill.is_some() && cam.rig.is_none()
}

/// The player's free look: a left-drag turns the orbit by the game's look law in beats 1 and 4,
/// never while the camera is captive.
pub fn input(
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    run: Res<Run>,
    mut cam: ResMut<KillCam>,
) {
    let Some((_, beats)) = run.kill else {
        return;
    };
    if run.paused || !mouse.pressed(MouseButton::Left) {
        return;
    }
    match beats.beat(run.t) {
        Beat::Hit => cam.hit = orbit_rotation(cam.hit, motion.delta),
        Beat::Death => {
            if let Some(look) = cam.death {
                cam.death = Some(orbit_rotation(look, motion.delta));
            }
        }
        Beat::Lesson | Beat::Killer => {}
    }
}

/// Stage the battlefield: the victim's tank as the body the camera rides, the shooter's tank of
/// the vehicle the record names on the ground at the staged muzzle facing the impact, and the rig
/// the kill cam's orbit is placed from. A kill run whose shooter cannot be staged — the record
/// names none, its tank file or scene failed to load, or it is still unstaged after
/// [`SHOOTER_WAIT_S`] of held clock — loses beat 3 ([`Run::drop_killer_beat`]) and is staged
/// about the victim alone.
#[expect(
    clippy::too_many_arguments,
    reason = "stages the battlefield: run, library, views, specs, loader, clock, camera and the view hierarchy reads"
)]
pub fn stage(
    mut run: ResMut<Run>,
    library: Res<Library>,
    mut views: ResMut<Views>,
    specs: Res<Assets<TankSpec>>,
    asset_server: Res<AssetServer>,
    time: Res<Time>,
    mut cam: ResMut<KillCam>,
    mut commands: Commands,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    names: Query<&Name>,
    mut transforms: Query<&mut Transform>,
    mut visibility: Query<&mut Visibility>,
    scopes: Query<&VisualScope>,
) {
    let entry = &PLAYLIST[run.entry];
    let scope = if entry.victim {
        VisualScope::VIEW_SUBJECT_BODY
    } else {
        VisualScope::WORLD_SOLID
    };
    if scopes.get(views.world).ok() != Some(&scope) {
        commands.entity(views.world).insert(scope);
    }
    if cam.staged != Some(run.entry) {
        cam.staged = Some(run.entry);
        cam.hit = Rig::player_look();
        cam.death = None;
        cam.waited = 0.0;
    }
    if holding(&run, &cam) {
        cam.waited += time.delta_secs();
    }
    cam.rig = None;

    // The shooter's view, of the vehicle the record of the shot the victim watches names.
    let shot = run
        .kill
        .map(|(shot, _)| shot)
        .or(entry.victim.then_some(0))
        .and_then(|shot| run.shots.get(shot));
    let scene = shot.and_then(|shot| shooter_scene(&library.records[shot.record].report));
    if let Some((vehicle, glb)) = &scene
        && cam.shooter.as_ref().is_none_or(|s| s.vehicle != *vehicle)
    {
        if let Some(old) = cam.shooter.take() {
            commands.entity(old.root).despawn();
        }
        let handle: Handle<WorldAsset> =
            asset_server.load(GltfAssetLabel::Scene(0).from_asset(glb.clone()));
        let root = commands
            .spawn((
                WorldAssetRoot(handle.clone()),
                Transform::IDENTITY,
                Visibility::Hidden,
                ViewRoot::Shooter,
            ))
            .id();
        let spec: Handle<TankSpec> = asset_server.load(vehicle.clone());
        views.shooter_spec = Some(spec.clone());
        cam.shooter = Some(ShooterView {
            root,
            vehicle: vehicle.clone(),
            spec,
            scene: handle,
            hull_inverse: None,
        });
        return;
    }
    let turret = |root: Entity, spec: &TankSpec, transforms: &Query<&mut Transform>| {
        let named = |wanted: &str| {
            children
                .iter_descendants(root)
                .find(|node| names.get(*node).is_ok_and(|n| n.as_str() == wanted))
        };
        let node = view::turret_node(spec, &named, &parents, &names)?;
        let pose = view::pose_under(node, root, &parents, |node| {
            transforms.get(node).ok().copied()
        });
        Some(pose.w_axis.truncate())
    };

    // The shooter, on the ground at the muzzle facing the impact, once its hull node exists.
    let mut shooter_pivot = None;
    if let Some(shooter) = cam.shooter.as_mut() {
        if shooter.hull_inverse.is_none() {
            let hull = children.iter_descendants(shooter.root).find(|node| {
                names
                    .get(*node)
                    .is_ok_and(|n| n.as_str() == view::HULL_NODE)
            });
            if let Some(hull) = hull {
                let pose = view::pose_under(hull, shooter.root, &parents, |node| {
                    transforms.get(node).ok().copied()
                });
                shooter.hull_inverse = Some(pose.inverse());
            }
        }
        let shooter_root = match (shooter.hull_inverse, shot, scene.is_some()) {
            (Some(hull_inverse), Some(shot), true) => {
                let impact = Vec3::from(library.records[shot.record].report.impact.point);
                let at = Vec3::new(shot.muzzle.x, 0.0, shot.muzzle.z);
                let toward = Vec3::new(impact.x - at.x, 0.0, impact.z - at.z);
                let pose = Transform::from_translation(at).looking_to(toward, Vec3::Y);
                Some(Transform::from_matrix(pose.to_matrix() * hull_inverse))
            }
            _ => None,
        };
        if let Ok(mut v) = visibility.get_mut(shooter.root) {
            v.set_if_neq(if shooter_root.is_some() {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
        if let Some(root) = shooter_root
            && let Ok(mut transform) = transforms.get_mut(shooter.root)
            && *transform != root
        {
            *transform = root;
        }
        shooter_pivot = shooter_root.and_then(|root_pose| {
            let spec = specs.get(&shooter.spec)?;
            Some(orbit_pivot(
                &root_pose,
                turret(shooter.root, spec, &transforms)?,
            ))
        });
    }

    let Some((shot_index, beats)) = run.kill else {
        return;
    };
    if beats.shooter
        && shooter_pivot.is_none()
        && (cam.waited >= SHOOTER_WAIT_S
            || cam
                .shooter
                .as_ref()
                .is_none_or(|shooter| shooter.failed(&asset_server)))
    {
        warn!(
            "kill cam: the shooter could not be staged after {:.1} s; beat 3 is skipped",
            cam.waited
        );
        run.drop_killer_beat();
    }
    let Some((_, beats)) = run.kill else {
        return;
    };

    // The rig: both pivots by the game's own formula, the turret ring of each tank lifted.
    let Some(ground_y) = views.ground_y else {
        return;
    };
    let Some(victim_spec) = specs.get(views.spec()) else {
        return;
    };
    let Some(victim_turret) = turret(views.world, victim_spec, &transforms) else {
        return;
    };
    if beats.shooter && shooter_pivot.is_none() {
        return;
    }
    let victim_root = transforms
        .get(views.world)
        .ok()
        .copied()
        .unwrap_or_default();
    let shot = &run.shots[shot_index];
    cam.rig = Some(Rig {
        victim_pivot: orbit_pivot(&victim_root, victim_turret),
        shooter_pivot: shooter_pivot.filter(|_| beats.shooter),
        impact: Vec3::from(library.records[shot.record].report.impact.point),
        muzzle: shot.muzzle,
        radius: ORBIT_FAR,
        ground_y,
    });
    cam.fov = victim_spec
        .views
        .get(&ViewKind::Commander)
        .map(|view| view.optics.vertical_fov())
        .unwrap_or(std::f32::consts::FRAC_PI_4);
}

/// The main camera's one writer on a kill run: its pose on the game's orbit, its field and its
/// profile, the lesson's swapping the ridden tank for its glass.
pub fn place(
    run: Res<Run>,
    settings: Res<Settings>,
    mut cam: ResMut<KillCam>,
    mut commands: Commands,
    mut main: Query<(Entity, &mut Transform, &mut Projection, &CameraProfile), With<MainCamera>>,
) {
    let (Some((_, beats)), Some(rig)) = (run.kill, cam.rig) else {
        return;
    };
    let Ok((entity, mut transform, mut projection, profile)) = main.single_mut() else {
        return;
    };
    let t = run.t;
    let beat = beats.beat(t);
    if beat != Beat::Death {
        cam.death = None;
    } else if cam.death.is_none() {
        cam.death = Some(rig.death_look(beats, settings.reduce_motion));
    }
    let death = cam
        .death
        .unwrap_or_else(|| rig.death_look(beats, settings.reduce_motion));
    let want = rig.pose(beats, t, cam.hit, death, settings.reduce_motion);
    if *transform != want {
        *transform = want;
    }
    if let Projection::Perspective(p) = projection.as_mut()
        && p.fov != cam.fov
    {
        p.fov = cam.fov;
    }
    let want = if beat == Beat::Lesson {
        CameraProfile::KillCamLesson
    } else {
        CameraProfile::BattlefieldThirdPerson
    };
    if *profile != want {
        commands.entity(entity).insert(want);
    }
}

/// The killer card, the death panel, the death tint and the respawn prompt.
#[expect(
    clippy::too_many_arguments,
    reason = "one query per disjoint death-screen element, kept disjoint by their `Without` filters"
)]
pub fn hud(
    run: Res<Run>,
    library: Res<Library>,
    mut card: Query<(&mut Visibility, &mut UiTransform), (With<Card>, Without<DeathPanel>)>,
    mut accent: Query<&mut BackgroundColor, With<CardAccent>>,
    mut card_text: Query<(&Index, &mut Text, &mut TextColor), (With<CardText>, Without<DeathText>)>,
    mut panel: Query<&mut Visibility, (With<DeathPanel>, Without<Card>)>,
    mut panel_text: Query<(&Index, &mut Text), (With<DeathText>, Without<CardText>)>,
    mut prompt: Query<(&ChildOf, &mut TextColor), (With<Prompt>, Without<CardText>)>,
    mut tint: Query<&mut Visibility, (With<DeathTint>, Without<Card>, Without<DeathPanel>)>,
    mut visibility: Query<
        &mut Visibility,
        (Without<Card>, Without<DeathPanel>, Without<DeathTint>),
    >,
) {
    let kill = run.kill.map(|(shot, beats)| (&run.shots[shot], beats));
    let t = run.t;
    let respawning = run.respawned_at.is_some();
    let (card_on, enter) = match kill {
        Some((_, beats)) if t >= beats.arrive() && !respawning => {
            let x = timeline::ease_enter(timeline::progress(t, beats.arrive(), CARD_ENTER_S));
            (true, x)
        }
        _ => (false, 0.0),
    };
    if let Ok((mut v, mut transform)) = card.single_mut() {
        v.set_if_neq(if card_on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        let scale = Vec2::splat(0.96 + 0.04 * enter);
        if transform.scale != scale {
            transform.scale = scale;
        }
    }
    if let Some((shot, _)) = kill {
        let record = &library.records[shot.record];
        if let Ok(mut background) = accent.single_mut() {
            let color = timeline::verdict_color(CornerKind::of(record.report.impact.verdict));
            background.0 = color.with_alpha(enter);
        }
        let impact = Vec3::from(record.report.impact.point);
        let range = record
            .report
            .muzzle
            .map(|muzzle| Vec3::from(muzzle).distance(impact));
        let lines = card_lines(&record.killer, range);
        for (index, mut text, mut color) in &mut card_text {
            let line = match index.0 {
                0 => "KNOCKED OUT BY".to_owned(),
                row => lines[row - 1].clone(),
            };
            if text.0 != line {
                text.0 = line;
            }
            color.0 = color.0.with_alpha(enter);
        }
        for (index, mut text) in &mut panel_text {
            let line = match (index.0, respawning) {
                (0, false) => "KNOCKED OUT".to_owned(),
                (0, true) => RESPAWNING.to_owned(),
                (_, false) => record
                    .knockout
                    .map(|reason| reason.label().to_owned())
                    .unwrap_or_default(),
                (_, true) => String::new(),
            };
            if text.0 != line {
                text.0 = line;
            }
        }
    }
    let death = respawning || kill.is_some_and(|(_, beats)| beats.beat(t) == Beat::Death);
    let shown = if death {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut v in panel.iter_mut().chain(tint.iter_mut()) {
        v.set_if_neq(shown);
    }
    for (parent, mut color) in &mut prompt {
        let live = !respawning && kill.is_some_and(|(_, beats)| beats.respawn_live(t));
        if let Ok(mut v) = visibility.get_mut(parent.parent()) {
            v.set_if_neq(if live {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
        color.0 = color.0.with_alpha(if death { 1.0 } else { PROMPT_DIM });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rig in the demo's staging: the victim at the origin, the shooter 700 m back along a level
    /// line, both pivots above their hulls. Fixture numbers.
    fn rig() -> Rig {
        let incoming = Vec3::new(0.5736, 0.0, 0.8191).normalize();
        let impact = Vec3::new(-1.63, 1.5, -0.31);
        let muzzle = impact - incoming * super::super::world::SHOOTER_DISTANCE_M;
        Rig {
            victim_pivot: Vec3::new(0.0, 4.4, 0.1),
            shooter_pivot: Some(Vec3::new(muzzle.x, 4.4, muzzle.z)),
            impact,
            muzzle,
            radius: ORBIT_FAR,
            ground_y: -0.6,
        }
    }

    /// The beats run in order; the camera is captive exactly in the lesson and the killer beats;
    /// respawn is live from the end of the hit; by the lesson's end the record's corner clock is
    /// past its settle for every record that knocks its tank out (the nose at the span's end, a
    /// field's front at its longest reach, a component's fill), held there [`LESSON_HOLD_S`], and
    /// the lesson is never under [`LESSON_S`]. The library holds a knockout record that settles too
    /// late for the minimum alone, so a lesson blind to its record fails here.
    #[test]
    fn the_beats_run_in_order() {
        let beats = Beats {
            impact: 2.0,
            shooter: true,
            lesson_s: LESSON_S,
        };
        assert!(beats.lesson() < beats.killer());
        assert!(beats.killer() < beats.arrive() && beats.arrive() < beats.death());
        assert!(beats.death() < beats.end());
        for (t, beat) in [
            (0.0, Beat::Hit),
            (2.4, Beat::Hit),
            (2.6, Beat::Lesson),
            (beats.killer() + 0.1, Beat::Killer),
            (beats.death() + 0.1, Beat::Death),
        ] {
            assert_eq!(beats.beat(t), beat, "t {t}");
            assert_eq!(beat.captive(), matches!(beat, Beat::Lesson | Beat::Killer));
        }
        assert!(!beats.respawn_live(beats.impact + HIT_S - 0.01));
        assert!(beats.respawn_live(beats.impact + HIT_S));
        assert!(beats.lesson_c(beats.killer()).is_none());
        assert_eq!(lesson_length(None), LESSON_S);
        let library = Library::load();
        let (mut knockouts, mut longer) = (0, 0);
        for record in library.records.iter().filter(|r| r.knockout.is_some()) {
            let beats = Beats {
                lesson_s: lesson_length(Some(&record.plan)),
                ..beats
            };
            let settle = stage::pace(&record.plan, CornerKind::Glass).settle;
            let held = beats.killer() - LESSON_HOLD_S + 1e-3;
            assert!(
                beats.lesson_c(held).unwrap() >= settle,
                "{}: settles at {settle}",
                record.stem
            );
            assert!(beats.lesson_s >= LESSON_S, "{}", record.stem);
            if settle / LESSON_RATE > LESSON_S {
                longer += 1;
            }
            knockouts += 1;
        }
        assert!(knockouts > 0, "a record knocks its tank out");
        assert!(
            longer > 0,
            "a knockout record settles past the minimum lesson"
        );
    }

    /// The camera never jumps: at every beat boundary its pose is continuous.
    #[test]
    fn the_camera_is_continuous_across_beats() {
        let rig = rig();
        let beats = Beats {
            impact: 2.0,
            shooter: true,
            lesson_s: LESSON_S,
        };
        let hit = Rig::player_look();
        let death = rig.death_look(beats, false);
        for boundary in [
            beats.lesson(),
            beats.lesson() + MOVE_S,
            beats.killer(),
            beats.arrive(),
            beats.death(),
        ] {
            let a = rig.pose(beats, boundary - 1e-4, hit, death, false);
            let b = rig.pose(beats, boundary + 1e-4, hit, death, false);
            assert!(
                a.translation.distance(b.translation) < 0.05,
                "position jumps at {boundary}: {} m",
                a.translation.distance(b.translation)
            );
            assert!(
                a.rotation.angle_between(b.rotation).to_degrees() < 0.5,
                "look jumps at {boundary}"
            );
        }
    }

    /// Beat 3's look follows the shooter's pivot wherever it stands, off the round's line too, and
    /// its orbit stays the victim's: through the whole beat the camera stands no farther than the
    /// play radius from the victim's pivot, and from the look's arrival on it stands on that radius
    /// looking through the victim's pivot at the shooter's. The 60 m sidestep and the 0.1 s sweep
    /// step are fixture data.
    #[test]
    fn the_killer_look_follows_the_shooter_not_the_line() {
        let mut rig = rig();
        let beats = Beats {
            impact: 2.0,
            shooter: true,
            lesson_s: LESSON_S,
        };
        let side = rig.incoming_flat().cross(Vec3::Y) * 60.0;
        rig.shooter_pivot = rig.shooter_pivot.map(|pivot| pivot + side);
        let hit = Rig::player_look();
        let death = rig.death_look(beats, false);
        let shooter = rig.shooter_pivot.unwrap();
        let mut t = beats.killer();
        while t < beats.death() {
            let pose = rig.pose(beats, t, hit, death, false);
            let r = pose.translation.distance(rig.victim_pivot);
            assert!(
                r <= rig.radius + 1e-3,
                "t {t}: {r} m off the victim's pivot"
            );
            if t >= beats.arrive() {
                let to_shooter = (shooter - pose.translation).normalize();
                assert!(
                    pose.forward().dot(to_shooter) > 0.9999,
                    "t {t}: off the shooter"
                );
                let to_victim = (rig.victim_pivot - pose.translation).normalize();
                assert!(
                    pose.forward().dot(to_victim) > 0.9999,
                    "t {t}: off the victim"
                );
                assert!(
                    (r - rig.radius).abs() < 1e-2,
                    "t {t}: {r} m, not the play radius"
                );
            }
            t += 0.1;
        }
        let pose = rig.pose(beats, beats.arrive() + 0.5, hit, death, false);
        let to_shooter = (shooter - pose.translation).normalize();
        assert!(pose.forward().dot(to_shooter) > 0.9999);
        assert!((pose.translation.distance(rig.victim_pivot) - rig.radius).abs() < 1e-2);
    }

    /// The card's data lines draw in the shipped font: an undrawable name falls back to saying it
    /// is unknown, a partly drawable one keeps what draws; a record with no shooter names neither,
    /// and one with no muzzle shows no range. A round the registry lacks reads as unknown, and every
    /// registered round is named by its display name, never by its registry key.
    #[test]
    fn the_card_draws_only_what_the_font_covers() {
        let mut killer = Killer {
            player: Some("\u{864E}\u{738B}".into()),
            vehicle: Some("Tiger \u{864E} I".into()),
            round: Some(("Probe Mk. 1".into(), 773.0)),
        };
        let [name, vehicle, round] = card_lines(&killer, Some(700.0));
        assert_eq!(name, "UNKNOWN PLAYER");
        assert_eq!(vehicle, "Tiger I");
        assert!(round.contains("Probe Mk. 1") && round.contains("700 m"));
        (killer.player, killer.vehicle) = (None, None);
        let [name, vehicle, round] = card_lines(&killer, None);
        assert_eq!(
            (name.as_str(), vehicle.as_str()),
            ("UNKNOWN PLAYER", "UNKNOWN VEHICLE")
        );
        assert_eq!(round, "Probe Mk. 1    773 m/s");
        killer.round = None;
        assert_eq!(card_lines(&killer, None)[2], "UNKNOWN ROUND");
        assert_eq!(
            card_lines(&Killer::of_vehicle(None, "<unregistered>"), None)[2],
            "UNKNOWN ROUND"
        );
        let registry = crate::rounds::RoundRegistry::shipped();
        for key in registry.keys() {
            let [_, _, round] = card_lines(&Killer::of_vehicle(None, key), None);
            let name = registry.name(key).expect("a registered round's name");
            assert!(round.contains(name), "{key}: {round}");
            assert!(!round.contains(key), "{round} shows the key {key}");
        }
    }

    /// A kill record that names no shooter plays the hit, the lesson and the death screen with no
    /// beat 3: the death screen starts where the killer beat would, and orbits the victim,
    /// continuing the lesson's look.
    #[test]
    fn a_kill_without_a_shooter_skips_beat_three() {
        let beats = Beats {
            impact: 2.0,
            shooter: false,
            lesson_s: LESSON_S,
        };
        assert_eq!(beats.death(), beats.killer());
        let mut rig = rig();
        rig.shooter_pivot = None;
        let hit = Rig::player_look();
        let death = rig.death_look(beats, false);
        let lesson_end = rig.pose(beats, beats.killer() - 1e-4, hit, death, false);
        let death_start = rig.pose(beats, beats.death() + 1e-4, hit, death, false);
        assert!(
            death_start.translation.distance(lesson_end.translation) < 0.05,
            "the death screen continues the lesson about the victim"
        );
    }
}
