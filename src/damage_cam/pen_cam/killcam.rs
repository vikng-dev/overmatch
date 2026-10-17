//! The victim kill cam: what the struck player sees when a record knocks their own tank out. Four
//! beats on the run's clock, counted from the killing shot's world impact ([`Beats`]):
//!
//! 1. **Hit** — the player's own orbit view, free, the world showing the hit in real time.
//! 2. **Lesson** — captive: the look eases to a three-quarter from the shot's side; the main camera
//!    swaps to [`CameraProfile::KillCamLesson`], so the tank it rides leaves the view and the pen
//!    cam stage, which sits on it in the record's frame, draws the record over the live world at
//!    [`LESSON_RATE`] of the corner's tempo. The world clock is untouched.
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
/// The lesson's length (real seconds).
pub const LESSON_S: f32 = 3.6;
/// The record's tempo in the lesson, as a fraction of the corner's.
pub const LESSON_RATE: f32 = 0.5;
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
        let registry = crate::rounds::RoundRegistry::shipped();
        let round = registry
            .get(round)
            .and_then(|fired| Ok((registry.name(round)?.to_owned(), fired.muzzle_speed)))
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

/// The sequence's beats in run seconds, from the killing shot's world impact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beats {
    pub impact: f32,
    /// Whether beat 3 plays: only for a shooter the kill cam can stage. Without one the death
    /// screen follows the lesson.
    pub shooter: bool,
}

impl Beats {
    pub fn lesson(self) -> f32 {
        self.impact + HIT_S
    }
    pub fn killer(self) -> f32 {
        self.lesson() + LESSON_S
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

    fn library() -> Library {
        Library::load()
    }

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

    /// A record kills by the game's rule: a cookoff always does; a record that leaves a crew
    /// member alive and cooks nothing off does not. At least one playlist entry plays the kill
    /// cam (fixture check).
    #[test]
    fn the_kill_rule_reads_crew_and_cookoff() {
        let library = library();
        for record in &library.records {
            let report = &record.report;
            let cooked = report
                .consequences
                .iter()
                .any(|c| c.state == ConsequenceState::Cookoff);
            let crew_left = report.components.iter().any(|c| {
                c.role == Role::Crew
                    && c.hp > 0
                    && !report
                        .consequences
                        .iter()
                        .any(|q| q.volume == c.name && q.state == ConsequenceState::Lost)
            });
            let reason = knockout(report);
            if cooked {
                assert_eq!(reason, Some(KnockoutReason::Cookoff), "{}", record.stem);
            } else if crew_left {
                assert_eq!(reason, None, "{}", record.stem);
            }
            assert_eq!(record.knockout, reason, "{}", record.stem);
        }
        let settings = Settings::default();
        let kills = (0..PLAYLIST.len())
            .filter(|&entry| Run::new(entry, &library, &settings, true).kill.is_some())
            .count();
        assert!(kills >= 1, "no playlist entry plays the kill cam");
    }

    /// Every kill entry's record names its killer, read from the shooter's tank file and the round
    /// registry, and only a victim entry plays the kill cam.
    #[test]
    fn a_kill_names_its_killer() {
        let library = library();
        let settings = Settings::default();
        for (index, entry) in PLAYLIST.iter().enumerate() {
            let run = Run::new(index, &library, &settings, true);
            let Some((shot, _)) = run.kill else {
                continue;
            };
            assert!(entry.victim, "{}", entry.label);
            let record = &library.records[run.shots[shot].record];
            let killer = &record.killer;
            let name = killer.player.as_ref().expect("a kill names its killer");
            let vehicle = killer
                .vehicle
                .as_ref()
                .expect("the killer's tank file reads");
            assert!(!name.is_empty() && !vehicle.is_empty());
            let (_, muzzle_speed) = killer.round.as_ref().expect("the registry has the round");
            assert!(*muzzle_speed > 0.0);
            assert!(
                run.turns.is_empty(),
                "{}: the corner opens on a kill",
                entry.label
            );
        }
    }

    /// The beats run in order; the camera is captive exactly in the lesson and the killer beats;
    /// respawn is live from the end of the hit; by the lesson's end the record's corner clock is
    /// past its last event for every record that knocks its tank out (the travel window's end, a
    /// cone's front at its longest reach, a component's fill).
    #[test]
    fn the_beats_run_in_order() {
        let beats = Beats {
            impact: 2.0,
            shooter: true,
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
        let library = Library::load();
        let mut knockouts = 0;
        for record in library.records.iter().filter(|r| r.knockout.is_some()) {
            let last_event = super::super::stage::settled(&record.plan, CornerKind::Glass);
            assert!(
                beats.lesson_c(beats.killer() - 1e-3).unwrap() >= last_event,
                "{}: settles at {last_event}",
                record.stem
            );
            knockouts += 1;
        }
        assert!(knockouts > 0, "a record knocks its tank out");
        assert!(beats.lesson_c(beats.killer()).is_none());
    }

    /// The camera never jumps: at every beat boundary its pose is continuous.
    #[test]
    fn the_camera_is_continuous_across_beats() {
        let rig = rig();
        let beats = Beats {
            impact: 2.0,
            shooter: true,
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

    /// Through the demo's wiring, from the lesson to the death screen: the camera stays on the
    /// victim's own orbit, never farther than the play radius from the victim's pivot, so it never
    /// zooms or changes centre; from the look's arrival in beat 3 it looks through the victim's
    /// pivot at the shooter's, and when the stage moves the shooter it follows. Where the look
    /// does not reach the ground the body sits at the play radius. The shooter's move is fixture
    /// data.
    #[test]
    fn the_killer_beat_tracks_the_shooter_from_the_victims_orbit() {
        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        app.update();
        let (shot, beats) = app.world().resource::<Run>().kill.unwrap();
        assert!(beats.shooter);
        let mut t = beats.lesson();
        while t < beats.end() {
            app.world_mut().resource_mut::<Run>().t = t;
            app.update();
            let rig = staged(&app);
            let (pose, _) = camera(&mut app);
            let r = pose.translation.distance(rig.victim_pivot);
            assert!(
                r <= rig.radius + 1e-3,
                "t {t}: {r} m off the victim's pivot"
            );
            if beats.beat(t) == Beat::Killer && t >= beats.arrive() {
                let shooter = rig.shooter_pivot.expect("a staged shooter");
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
            t += 1.0 / 10.0;
        }

        let moved = Vec3::new(40.0, 0.0, -25.0);
        let t = beats.arrive() + 0.5;
        app.world_mut().resource_mut::<Run>().t = t;
        app.update();
        let before = staged(&app).shooter_pivot.unwrap();
        app.world_mut().resource_mut::<Run>().shots[shot].muzzle += moved;
        app.update();
        let rig = staged(&app);
        let shooter = rig.shooter_pivot.unwrap();
        assert!(
            shooter.distance(before) > 10.0,
            "the stage moved the shooter"
        );
        let (pose, _) = camera(&mut app);
        let to_shooter = (shooter - pose.translation).normalize();
        assert!(
            pose.forward().dot(to_shooter) > 0.9999,
            "the look tracks the shooter"
        );
        assert!((pose.translation.distance(rig.victim_pivot) - rig.radius).abs() < 1e-2);
    }

    /// Beat 3's look follows the shooter's pivot wherever it stands, off the round's line too, and
    /// its orbit stays the victim's. The 60 m sidestep is fixture data.
    #[test]
    fn the_killer_look_follows_the_shooter_not_the_line() {
        let mut rig = rig();
        let beats = Beats {
            impact: 2.0,
            shooter: true,
        };
        let side = rig.incoming_flat().cross(Vec3::Y) * 60.0;
        rig.shooter_pivot = rig.shooter_pivot.map(|pivot| pivot + side);
        let t = beats.arrive() + 0.5;
        let hit = Rig::player_look();
        let pose = rig.pose(beats, t, hit, rig.death_look(beats, false), false);
        let to_shooter = (rig.shooter_pivot.unwrap() - pose.translation).normalize();
        assert!(pose.forward().dot(to_shooter) > 0.9999);
        assert!((pose.translation.distance(rig.victim_pivot) - rig.radius).abs() < 1e-2);
    }

    /// The flat ground pulls the body in, as the game's ground ray does.
    #[test]
    fn the_ground_pulls_the_body_in() {
        let rig = rig();
        let steep = look_along(Vec3::new(0.0, 1.0, -0.2));
        let pose = rig.place(rig.victim_pivot, steep);
        assert!(pose.translation.y >= rig.ground_y - 1e-4);
        assert!(pose.translation.distance(rig.victim_pivot) < rig.radius);
    }

    /// A report with `crew` crew volumes at the given hit points, `lost` of them lost by the shot,
    /// and a cookoff or not; every other field from a recorded fixture.
    fn crew_report(crew: &[u32], lost: &[usize], cooked: bool) -> PenetrationReport {
        let mut report = library().records[0].report.clone();
        report.components = crew
            .iter()
            .enumerate()
            .map(|(index, &hp)| crate::damage_cam::ComponentState {
                name: format!("Crew{index}"),
                role: Role::Crew,
                count: 30,
                hp,
            })
            .collect();
        report.consequences = lost
            .iter()
            .map(|&index| crate::damage_cam::Consequence {
                t: 0.0,
                volume: format!("Crew{index}"),
                state: ConsequenceState::Lost,
                function: None,
            })
            .collect();
        if cooked {
            report.consequences.push(crate::damage_cam::Consequence {
                t: 0.0,
                volume: "Ammo".into(),
                state: ConsequenceState::Cookoff,
                function: None,
            });
        }
        report
    }

    /// Crew loss knocks a tank out when the shot's casualties and the losses before it leave no
    /// one; a survivor, or a tank without crew volumes, is not knocked out by crew loss; a cookoff
    /// knocks out whatever the crew.
    #[test]
    fn crew_loss_counts_prior_losses_and_the_final_casualty() {
        let cases: [(&[u32], &[usize], bool, Option<KnockoutReason>); 7] = [
            // The shot kills the last of them.
            (&[30, 30], &[0, 1], false, Some(KnockoutReason::CrewLoss)),
            // One survives the shot.
            (&[30, 30], &[0], false, None),
            // Earlier hits left one; the shot kills him.
            (&[0, 0, 30], &[2], false, Some(KnockoutReason::CrewLoss)),
            // Earlier hits left one; the shot misses him.
            (&[0, 0, 30], &[0], false, None),
            // No crew volumes: crew loss cannot be read.
            (&[], &[], false, None),
            // A cookoff, whatever the crew.
            (&[30, 30], &[], true, Some(KnockoutReason::Cookoff)),
            (&[], &[], true, Some(KnockoutReason::Cookoff)),
        ];
        for (crew, lost, cooked, want) in cases {
            let report = crew_report(crew, lost, cooked);
            assert_eq!(
                knockout(&report),
                want,
                "crew {crew:?} lost {lost:?} {cooked}"
            );
        }
    }

    /// The shooter's view is the vehicle the record names, never the struck one.
    #[test]
    fn the_shooter_is_the_vehicle_the_record_names() {
        let mut report = library().records[0].report.clone();
        report.shooter = Some(Shooter {
            name: "Other".into(),
            vehicle: "other_tank/other_tank.tank.ron".into(),
        });
        assert_eq!(
            shooter_scene(&report),
            Some((
                "other_tank/other_tank.tank.ron".to_owned(),
                "other_tank/other_tank.glb".to_owned()
            ))
        );
        report.shooter = None;
        assert_eq!(shooter_scene(&report), None);
    }

    /// The card's data lines draw in the shipped font: an undrawable name falls back to saying it
    /// is unknown, a partly drawable one keeps what draws; a record with no shooter names neither,
    /// and one with no muzzle shows no range.
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
    }

    /// Where a standing-in view's nodes sit under their parents (m). Fixture data.
    const HULL_AT: Vec3 = Vec3::new(0.0, 0.9, 0.1);
    const TURRET_AT: Vec3 = Vec3::new(0.0, 1.0, -0.2);
    const SIGHT_AT: Vec3 = Vec3::new(0.3, 0.4, 0.5);
    /// The flat ground under the standing-in battlefield (m). Fixture data.
    const GROUND_Y: f32 = -0.6;

    /// The tank file at `vehicle`, read as [`Killer::of`] reads it.
    fn tank_spec(vehicle: &str) -> TankSpec {
        let path = crate::assets::asset_root().join(vehicle);
        ron::de::from_str(&std::fs::read_to_string(path).expect("the tank file"))
            .expect("the tank file parses")
    }

    /// Stand a tank view in for its glb under `root`: the hull node, the yaw servo above the
    /// gunner's view and the gunner's view node, named as `spec` names them, at fixture offsets.
    fn stand_in(world: &mut World, root: Entity, spec: &TankSpec) {
        let sight = spec.views[&ViewKind::Gunner].node.clone();
        let yaw = spec
            .servos
            .iter()
            .filter(|(_, servo)| servo.role() == crate::tank::ServoRole::Yaw)
            .map(|(name, _)| name.clone())
            .min()
            .expect("a yaw servo");
        let hull = world
            .spawn((
                Name::new(view::HULL_NODE),
                Transform::from_translation(HULL_AT),
                ChildOf(root),
            ))
            .id();
        let turret = world
            .spawn((
                Name::new(yaw.clone()),
                Transform::from_translation(TURRET_AT),
                ChildOf(hull),
            ))
            .id();
        if sight != yaw {
            world.spawn((
                Name::new(sight),
                Transform::from_translation(SIGHT_AT),
                ChildOf(turret),
            ));
        }
    }

    /// How the kill run's shooter reaches the stage.
    #[derive(Clone, Copy, PartialEq)]
    enum Shooting {
        /// Its scene has spawned: a view stands in for it.
        Spawned,
        /// Its view root exists and its scene has not spawned yet.
        Loading,
        /// The stage loads it through the asset server, which has no loader for it.
        Unloadable,
    }

    /// The demo's own frame ([`super::super::demo::sequence`]) on a headless app: the run of `entry` over
    /// the library as `edit` leaves it, the left button held with a drag, and the main camera. The
    /// battlefield is staged by the stage's own systems from views standing in for the glbs: the
    /// victim's and the stage's, and the shooter's as `shooting` says. Systems whose scene
    /// resources a headless app lacks (the corner's images, the tracers) are skipped by the
    /// ignoring error handler; everything that stages the battlefield and decides the cameras,
    /// their input, the clock and the panels runs as in the demo.
    fn demo_app_with(entry: usize, shooting: Shooting, edit: impl FnOnce(&mut Library)) -> App {
        let mut library = library();
        edit(&mut library);
        let settings = Settings::default();
        let run = Run::new(entry, &library, &settings, false);
        let mut app = App::new();
        app.set_error_handler(bevy::ecs::error::ignore);
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TransformPlugin,
        ));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app.init_asset::<TankSpec>()
            .init_asset::<WorldAsset>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_asset::<super::super::material::GlowMaterial>()
            .init_asset::<super::super::material::OutlineMaterial>()
            .init_asset::<super::super::material::SliceMaterial>()
            .init_asset::<super::super::look::LookMaterial>();

        let victim = tank_spec(&library.vehicle);
        let world = app.world_mut();
        let battlefield = world
            .spawn((Transform::IDENTITY, Visibility::Hidden, ViewRoot::World))
            .id();
        stand_in(world, battlefield, &victim);
        let stage = world
            .spawn((Transform::IDENTITY, Visibility::Hidden, ViewRoot::Stage))
            .id();
        stand_in(world, stage, &victim);
        let spec = world.resource_mut::<Assets<TankSpec>>().add(victim);
        world.insert_resource(Views::standing_in(spec.clone(), battlefield, GROUND_Y));
        world.insert_resource(view::StageView::standing_in(spec, stage));

        let mut cam = KillCam::default();
        let shooter = run.kill.and_then(|(shot, _)| {
            library.records[run.shots[shot].record]
                .report
                .shooter
                .clone()
        });
        if let Some(shooter) = shooter
            && shooting != Shooting::Unloadable
        {
            let spec = tank_spec(&shooter.vehicle);
            let root = world
                .spawn((Transform::IDENTITY, Visibility::Hidden, ViewRoot::Shooter))
                .id();
            if shooting == Shooting::Spawned {
                stand_in(world, root, &spec);
            }
            cam.shooter = Some(ShooterView {
                root,
                vehicle: shooter.vehicle,
                spec: world.resource_mut::<Assets<TankSpec>>().add(spec),
                scene: Handle::default(),
                hull_inverse: None,
            });
        }

        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Left);
        app.insert_resource(library)
            .insert_resource(settings)
            .insert_resource(run)
            .insert_resource(cam)
            .insert_resource(mouse)
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(AccumulatedMouseMotion {
                delta: Vec2::new(40.0, 0.0),
            })
            .insert_resource(UiFonts {
                hud: Handle::default(),
                body: Handle::default(),
            })
            .init_resource::<Time>()
            .add_systems(Startup, (spawn, super::super::stage::spawn));
        super::super::demo::sequence(&mut app);
        app.world_mut().spawn((
            Transform::IDENTITY,
            Projection::Perspective(PerspectiveProjection::default()),
            CameraProfile::BattlefieldThirdPerson,
            MainCamera,
        ));
        app
    }

    fn demo_app(entry: usize, shooting: Shooting) -> App {
        demo_app_with(entry, shooting, |_| {})
    }

    /// The rig the stage placed.
    fn staged(app: &App) -> Rig {
        app.world()
            .resource::<KillCam>()
            .rig
            .expect("the stage placed the rig")
    }

    /// The inset camera's switch, set to `on` where given; what it reads now.
    fn inset(app: &mut App, on: Option<bool>) -> bool {
        let mut query = app
            .world_mut()
            .query_filtered::<&mut Camera, With<super::super::stage::StageCamera>>();
        let mut camera = query.single_mut(app.world_mut()).expect("the inset camera");
        if let Some(on) = on {
            camera.is_active = on;
        }
        camera.is_active
    }

    fn advance_time(app: &mut App, seconds: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(seconds));
    }

    fn camera(app: &mut App) -> (Transform, CameraProfile) {
        let mut query = app
            .world_mut()
            .query_filtered::<(&Transform, &CameraProfile), With<MainCamera>>();
        let (transform, profile) = query.single(app.world()).unwrap();
        (*transform, *profile)
    }

    fn press(app: &mut App, key: KeyCode) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.clear();
        keys.press(key);
    }

    fn release(app: &mut App) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release_all();
        keys.clear();
    }

    fn kill_entry() -> usize {
        let library = library();
        (0..PLAYLIST.len())
            .find(|&entry| {
                Run::new(entry, &library, &Settings::default(), true)
                    .kill
                    .is_some()
            })
            .expect("a kill entry")
    }

    /// Through the demo's wiring, across every handoff: the player's drag turns the camera only
    /// while it is free (the hit and the death screen), the kill cam is the camera's writer in
    /// every beat, and only the lesson's profile swaps the ridden tank for its glass.
    #[test]
    fn the_drag_moves_the_camera_only_while_it_is_free() {
        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        for (t, beat) in [
            (beats.impact + 0.1, Beat::Hit),
            (beats.lesson() + 1.0, Beat::Lesson),
            (beats.killer() + 1.0, Beat::Killer),
            (beats.death() + 0.5, Beat::Death),
        ] {
            app.world_mut().resource_mut::<Run>().t = t;
            // The first frame in a beat latches its look; the second is the one judged.
            app.update();
            let before = {
                let cam = app.world().resource::<KillCam>();
                (cam.hit, cam.death)
            };
            app.update();
            let cam = app.world().resource::<KillCam>();
            let moved = match beat {
                Beat::Death => cam.death != before.1 && before.1.is_some(),
                _ => cam.hit != before.0,
            };
            assert_eq!(moved, !beat.captive(), "{beat:?}");
            let rig = staged(&app);
            let want = rig.pose(
                beats,
                t,
                cam.hit,
                cam.death.unwrap_or_else(|| rig.death_look(beats, false)),
                false,
            );
            let (transform, profile) = camera(&mut app);
            assert!(
                transform.translation.distance(want.translation) < 1e-3,
                "{beat:?}: the camera is not the kill cam's"
            );
            assert_eq!(
                profile == CameraProfile::KillCamLesson,
                beat == Beat::Lesson,
                "{beat:?}"
            );
        }
    }

    /// The stage stands the shooter's hull on the ground at the record's muzzle, facing the
    /// impact, with the pivot beat 3 looks toward above it: the shooter's view root is placed so
    /// its hull node, wherever the scene puts it under the root, lands there.
    #[test]
    fn the_staged_shooter_stands_at_the_muzzle_facing_the_impact() {
        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        app.update();
        let rig = staged(&app);
        let run = app.world().resource::<Run>();
        let shot = &run.shots[run.kill.unwrap().0];
        let library = app.world().resource::<Library>();
        let impact = Vec3::from(library.records[shot.record].report.impact.point);
        let muzzle = shot.muzzle;
        let cam = app.world().resource::<KillCam>();
        let root = cam.shooter.as_ref().expect("a shooter view").root;
        let root_pose = *app.world().get::<Transform>(root).unwrap();
        let hull = root_pose.to_matrix() * Mat4::from_translation(HULL_AT);
        let at = hull.w_axis.truncate();
        assert!(
            at.distance(Vec3::new(muzzle.x, 0.0, muzzle.z)) < 1e-3,
            "the hull stands at {at}, the muzzle is at {muzzle}"
        );
        let forward = (hull * Vec4::NEG_Z).truncate();
        let toward = Vec3::new(impact.x - muzzle.x, 0.0, impact.z - muzzle.z).normalize();
        assert!(forward.normalize().dot(toward) > 0.9999, "{forward}");
        assert_eq!(
            app.world().get::<Visibility>(root),
            Some(&Visibility::Inherited)
        );
        let pivot = rig.shooter_pivot.expect("a staged shooter");
        assert!(Vec2::new(pivot.x - at.x, pivot.z - at.z).length() < 1.0 + TURRET_AT.length());
    }

    /// The inset camera draws a corner's record and stays off for the kill cam's lesson, whatever
    /// it read before the frame.
    #[test]
    fn the_inset_draws_the_corner_and_not_the_lesson() {
        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        app.update();
        app.world_mut().resource_mut::<Run>().t = beats.lesson() + 1.0;
        inset(&mut app, Some(true));
        app.update();
        assert!(!inset(&mut app, None), "the lesson draws on the world");

        let library = library();
        let corner = (0..PLAYLIST.len())
            .find(|&entry| {
                !Run::new(entry, &library, &Settings::default(), false)
                    .turns
                    .is_empty()
            })
            .expect("an entry opens the corner");
        let mut app = demo_app(corner, Shooting::Spawned);
        app.update();
        let start = app.world().resource::<Run>().turns[0].start;
        app.world_mut().resource_mut::<Run>().t = start + 0.5;
        inset(&mut app, Some(false));
        app.update();
        assert!(inset(&mut app, None), "the corner draws in the inset");
    }

    /// Give the main camera the primary window as its target, at `size` physical pixels and
    /// `scale`, or resize that window. Only the window changes: the camera's cached target values
    /// stay as a headless app leaves them, unresolved.
    fn window(app: &mut App, size: UVec2, scale: f32) {
        let main = main_camera(app);
        let world = app.world_mut();
        if !world.entity(main).contains::<Camera>() {
            world.entity_mut(main).insert(Camera::default());
        }
        let mut query = world.query_filtered::<&mut Window, With<bevy::window::PrimaryWindow>>();
        if let Ok(mut window) = query.single_mut(world) {
            window.resolution.set_physical_resolution(size.x, size.y);
            window.resolution.set_scale_factor_override(Some(scale));
        } else {
            world.spawn((
                Window {
                    resolution: bevy::window::WindowResolution::new(size.x, size.y)
                        .with_scale_factor_override(scale),
                    ..default()
                },
                bevy::window::PrimaryWindow,
            ));
        }
    }

    /// Point `camera` at a new image of `size` physical pixels at `scale`, as the corner's layout
    /// retargets the inset; the camera's cached target values are left as they were.
    fn retarget(app: &mut App, camera: Entity, size: UVec2, scale: f32) {
        let world = app.world_mut();
        let handle = world
            .resource_mut::<Assets<Image>>()
            .add(Image::new_target_texture(
                size.x,
                size.y,
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                None,
            ));
        world
            .entity_mut(camera)
            .insert(bevy::camera::RenderTarget::Image(
                bevy::camera::ImageRenderTarget {
                    handle,
                    scale_factor: scale,
                },
            ));
    }

    fn main_camera(app: &mut App) -> Entity {
        let mut query = app.world_mut().query_filtered::<Entity, With<MainCamera>>();
        query.single(app.world()).unwrap()
    }

    /// Every mask camera and the camera it draws for.
    fn mask_cameras(app: &mut App) -> Vec<(Entity, Entity)> {
        let mut query = app
            .world_mut()
            .query::<(Entity, &super::super::look::MaskCamera)>();
        query
            .iter(app.world())
            .map(|(entity, mask)| (entity, mask.viewer))
            .collect()
    }

    /// The mask images' ids and sizes, where they exist.
    fn mask_images(app: &App) -> [(AssetId<Image>, Option<UVec2>); 2] {
        let masks = app.world().resource::<super::super::look::Masks>();
        let images = app.world().resource::<Assets<Image>>();
        [masks.hull.id(), masks.part.id()].map(|id| (id, images.get(id).map(Image::size)))
    }

    /// The kill cam's lesson owns the one mask set, drawn for the main camera: two mask cameras
    /// at its window's physical size and scale factor, before it, with its lens and, in the same
    /// frame, its final pose. A window resize makes a new set at the new size in the frame it
    /// happens and removes the old images; the lesson's end leaves no mask camera, no mask image
    /// and no stage mesh in the base look.
    #[test]
    fn the_lesson_masks_follow_its_camera_and_go_with_it() {
        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        let main = main_camera(&mut app);
        window(&mut app, UVec2::new(1600, 900), 2.0);
        app.world_mut().resource_mut::<Run>().t = beats.lesson() + 1.0;
        app.update();
        app.world_mut().resource_mut::<Run>().t = beats.lesson() + 1.5;
        app.update();

        let follows = |app: &mut App, size: UVec2, scale: f32| {
            let masks = mask_cameras(app);
            assert_eq!(masks.len(), 2, "one set");
            for (_, size_now) in mask_images(app) {
                assert_eq!(size_now, Some(size));
            }
            let world = app.world();
            assert_eq!(
                world.resource::<super::super::look::Masks>().viewer(),
                Some(main)
            );
            let pose = *world.get::<GlobalTransform>(main).unwrap();
            let Projection::Perspective(lens) = world.get::<Projection>(main).unwrap() else {
                panic!("the main camera is perspective");
            };
            let order = world.get::<Camera>(main).unwrap().order;
            for (mask, viewer) in masks {
                assert_eq!(viewer, main);
                assert_eq!(*world.get::<GlobalTransform>(mask).unwrap(), pose);
                let Projection::Perspective(mask_lens) = world.get::<Projection>(mask).unwrap()
                else {
                    panic!("the mask camera is perspective");
                };
                assert_eq!(mask_lens.fov, lens.fov);
                assert!(world.get::<Camera>(mask).unwrap().order < order);
                let bevy::camera::RenderTarget::Image(target) =
                    world.get::<bevy::camera::RenderTarget>(mask).unwrap()
                else {
                    panic!("the mask camera draws to an image");
                };
                assert_eq!(target.scale_factor, scale);
            }
            pose
        };
        let before = follows(&mut app, UVec2::new(1600, 900), 2.0);
        app.world_mut().resource_mut::<Run>().t = beats.lesson() + 2.0;
        app.update();
        let after = follows(&mut app, UVec2::new(1600, 900), 2.0);
        assert_ne!(before, after, "the lesson's camera moves");

        let old = mask_images(&app);
        window(&mut app, UVec2::new(1280, 720), 1.0);
        app.update();
        follows(&mut app, UVec2::new(1280, 720), 1.0);
        for (id, _) in old {
            assert!(app.world().resource::<Assets<Image>>().get(id).is_none());
        }

        let old = mask_images(&app);
        app.world_mut().resource_mut::<Run>().t = beats.killer() + 1.0;
        app.update();
        app.update();
        assert!(mask_cameras(&mut app).is_empty());
        assert_eq!(
            app.world().resource::<super::super::look::Masks>().viewer(),
            None
        );
        for (id, _) in old {
            assert!(app.world().resource::<Assets<Image>>().get(id).is_none());
        }
        let mut dressed = app.world_mut().query_filtered::<(), (
            With<super::super::view::ViewMesh>,
            With<MeshMaterial3d<super::super::look::LookMaterial>>,
        )>();
        assert_eq!(dressed.iter(app.world()).count(), 0);
    }

    /// A corner turn that draws the base look's hull owns the one mask set, drawn for the inset
    /// camera at its image's size; retargeting the inset to a larger image (the corner expanded)
    /// makes a new set at that size in the same frame and removes the old images.
    #[test]
    fn a_corner_turn_gives_the_masks_to_the_inset() {
        let library = library();
        let settings = Settings::default();
        let (entry, start) = (0..PLAYLIST.len())
            .find_map(|entry| {
                let run = Run::new(entry, &library, &settings, false);
                run.turns
                    .iter()
                    .find(|turn| {
                        matches!(
                            run.kind_of(turn.record, &library),
                            CornerKind::Glass | CornerKind::Victim
                        )
                    })
                    .map(|turn| (entry, turn.start))
            })
            .expect("an entry opens the corner on a penetration");
        let mut app = demo_app(entry, Shooting::Spawned);
        app.update();
        window(&mut app, UVec2::new(1600, 900), 2.0);
        let inset = {
            let mut query = app
                .world_mut()
                .query_filtered::<Entity, With<super::super::stage::StageCamera>>();
            query.single(app.world()).unwrap()
        };
        retarget(&mut app, inset, UVec2::new(480, 270), 2.0);
        app.world_mut().resource_mut::<Run>().t = start + 0.5;
        app.update();
        let owned = |app: &mut App, size: UVec2| {
            let masks = mask_cameras(app);
            assert_eq!(masks.len(), 2);
            assert!(masks.iter().all(|(_, viewer)| *viewer == inset));
            for (_, now) in mask_images(app) {
                assert_eq!(now, Some(size));
            }
        };
        owned(&mut app, UVec2::new(480, 270));
        let old = mask_images(&app);
        retarget(&mut app, inset, UVec2::new(960, 540), 2.0);
        app.world_mut().resource_mut::<Run>().t = start + 0.6;
        app.update();
        owned(&mut app, UVec2::new(960, 540));
        for (id, _) in old {
            assert!(app.world().resource::<Assets<Image>>().get(id).is_none());
        }
    }

    /// Switching to another scenario during the lesson hands the camera back to the world with the
    /// battlefield profile: the ridden tank is drawn again and the stage is not.
    #[test]
    fn a_scenario_switch_leaves_no_lesson_profile() {
        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        app.world_mut().resource_mut::<Run>().t = beats.lesson() + 1.0;
        app.update();
        assert_eq!(camera(&mut app).1, CameraProfile::KillCamLesson);
        press(&mut app, super::super::demo::ENTRY_KEYS[0]);
        app.update();
        release(&mut app);
        app.update();
        let run = app.world().resource::<Run>();
        assert!(run.kill.is_none());
        let muzzle = run.shots[0].muzzle;
        let (transform, profile) = camera(&mut app);
        assert_eq!(profile, CameraProfile::BattlefieldThirdPerson);
        assert!(transform.translation.distance(muzzle) < 1e-3);
    }

    /// While the shooter's scene is still spawning there is no rig: the world keeps the camera and
    /// the run's clock holds, so nothing plays to a camera no one places; once it spawns, the stage
    /// places the rig and the clock runs.
    #[test]
    fn the_clock_holds_while_the_shooter_loads() {
        let mut app = demo_app(kill_entry(), Shooting::Loading);
        app.update();
        let start = app.world().resource::<Run>().t;
        let placed = camera(&mut app).0;
        assert_ne!(
            placed.translation,
            Vec3::ZERO,
            "no writer placed the camera"
        );
        advance_time(&mut app, 0.5);
        app.update();
        assert_eq!(app.world().resource::<Run>().t, start);
        assert!(app.world().resource::<KillCam>().rig.is_none());
        let root = app
            .world()
            .resource::<KillCam>()
            .shooter
            .as_ref()
            .unwrap()
            .root;
        let vehicle = app
            .world()
            .resource::<KillCam>()
            .shooter
            .as_ref()
            .unwrap()
            .vehicle
            .clone();
        stand_in(app.world_mut(), root, &tank_spec(&vehicle));
        app.update();
        assert!(staged(&app).shooter_pivot.is_some());
        advance_time(&mut app, 0.5);
        app.update();
        assert!(app.world().resource::<Run>().t > start);
        assert!(app.world().resource::<Run>().kill.unwrap().1.shooter);
    }

    /// The clock never holds longer than [`SHOOTER_WAIT_S`]: a shooter whose scene never spawns
    /// loses beat 3, and the run plays on about the victim.
    #[test]
    fn the_clock_holds_no_longer_than_the_wait() {
        let mut app = demo_app(kill_entry(), Shooting::Loading);
        app.update();
        let start = app.world().resource::<Run>().t;
        let mut frames = 0;
        while app.world().resource::<Run>().t == start {
            frames += 1;
            assert!(frames < 20, "the clock still holds");
            advance_time(&mut app, 0.5);
            app.update();
        }
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        assert!(!beats.shooter);
        assert!(staged(&app).shooter_pivot.is_none());
        assert!(app.world().resource::<KillCam>().waited >= SHOOTER_WAIT_S);
    }

    /// A shooter the asset server cannot load loses beat 3 as soon as the load fails, without
    /// waiting out [`SHOOTER_WAIT_S`].
    #[test]
    fn a_failed_shooter_load_skips_beat_three() {
        let mut app = demo_app(kill_entry(), Shooting::Unloadable);
        let mut frames = 0;
        while app.world().resource::<Run>().kill.unwrap().1.shooter {
            frames += 1;
            assert!(frames < 2000, "the load never failed");
            std::thread::sleep(std::time::Duration::from_millis(1));
            app.update();
        }
        assert!(app.world().resource::<KillCam>().waited < SHOOTER_WAIT_S);
        let start = app.world().resource::<Run>().t;
        app.update();
        advance_time(&mut app, 0.5);
        app.update();
        assert!(app.world().resource::<Run>().t > start, "the clock runs");
    }

    /// A kill record whose shooter's tank file does not exist loads, and plays as a shooter whose
    /// load failed: beat 3 drops, and the card names the record's player and an unknown vehicle.
    /// The missing path is fixture data.
    #[test]
    fn a_missing_shooter_file_skips_beat_three() {
        an_unreadable_shooter_file_skips_beat_three("no_such_tank/no_such_tank.tank.ron");
    }

    /// A kill record whose shooter's tank file exists but does not parse plays as one whose file
    /// is missing. The file and its text are fixture data, removed when the test ends.
    #[test]
    fn an_unparseable_shooter_file_skips_beat_three() {
        struct Removed(std::path::PathBuf);
        impl Drop for Removed {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let vehicle = format!("unparseable-{}.tank.ron", std::process::id());
        let file = Removed(crate::assets::asset_root().join(&vehicle));
        std::fs::write(&file.0, "(this is not a tank").expect("the fixture file");
        an_unreadable_shooter_file_skips_beat_three(&vehicle);
    }

    /// The kill entry, its record's shooter's tank file replaced by `vehicle`, played until beat 3
    /// drops: the card names the record's player and an unknown vehicle.
    fn an_unreadable_shooter_file_skips_beat_three(vehicle: &str) {
        let entry = kill_entry();
        let mut player = String::new();
        let mut app = demo_app_with(entry, Shooting::Unloadable, |library| {
            let run = Run::new(entry, library, &Settings::default(), true);
            let stem = library.records[run.shots[run.kill.unwrap().0].record].stem;
            let reports = Library::reports()
                .into_iter()
                .map(|(known, mut report)| {
                    if known == stem {
                        let shooter = report.shooter.as_mut().expect("the kill names a shooter");
                        shooter.vehicle = vehicle.to_owned();
                        player = shooter.name.clone();
                    }
                    (known, report)
                })
                .collect();
            *library = Library::of(reports);
        });
        let mut frames = 0;
        while app.world().resource::<Run>().kill.unwrap().1.shooter {
            frames += 1;
            assert!(frames < 2000, "the load never failed");
            std::thread::sleep(std::time::Duration::from_millis(1));
            app.update();
        }
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        app.world_mut().resource_mut::<Run>().t = beats.death() + 1e-4;
        app.update();
        let mut rows: Vec<(usize, String)> = app
            .world_mut()
            .query::<(&CardText, &Index, &Text)>()
            .iter(app.world())
            .map(|(_, index, text)| (index.0, text.0.clone()))
            .collect();
        rows.sort();
        assert_eq!(rows[1].1, player);
        assert_eq!(rows[2].1, "UNKNOWN VEHICLE");
    }

    /// A synthetic record with no shooter and a round the registry lacks loads: the card names an
    /// unknown player, vehicle and round. The round key is fixture data.
    #[test]
    fn a_shooterless_record_of_an_unknown_round_loads() {
        let mut reports = Library::reports();
        let (stem, report) = reports.first_mut().expect("the playlist has records");
        let stem = *stem;
        report.shooter = None;
        report.round = "no_such_round".to_owned();
        let library = Library::of(reports);
        let killer = &library.records[library.index(stem)].killer;
        assert_eq!(
            card_lines(killer, None),
            ["UNKNOWN PLAYER", "UNKNOWN VEHICLE", "UNKNOWN ROUND"].map(str::to_owned)
        );
    }

    /// A kill record that names no shooter plays the hit, the lesson and the death screen with no
    /// beat 3 and no hold: the death screen orbits the victim, continuing the lesson's look, and
    /// the card shows what the record has.
    #[test]
    fn a_kill_without_a_shooter_skips_beat_three() {
        let entry = kill_entry();
        let mut app = demo_app_with(entry, Shooting::Spawned, |library| {
            let settings = Settings::default();
            let run = Run::new(entry, library, &settings, true);
            let record = &mut library.records[run.shots[run.kill.unwrap().0].record];
            record.report.shooter = None;
            record.killer = Killer::of(None, &record.report.round);
        });
        app.update();
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        assert!(!beats.shooter);
        assert_eq!(beats.death(), beats.killer());
        let rig = staged(&app);
        assert!(rig.shooter_pivot.is_none());
        let start = app.world().resource::<Run>().t;
        advance_time(&mut app, 0.5);
        app.update();
        assert!(app.world().resource::<Run>().t > start, "the clock runs");
        let lesson_end = rig.pose(
            beats,
            beats.killer() - 1e-4,
            Quat::IDENTITY,
            Quat::IDENTITY,
            false,
        );
        app.world_mut().resource_mut::<Run>().t = beats.death() + 1e-4;
        app.update();
        let (transform, _) = camera(&mut app);
        assert!(
            transform.translation.distance(lesson_end.translation) < 0.05,
            "the death screen continues the lesson about the victim"
        );
        let mut rows: Vec<(usize, String)> = app
            .world_mut()
            .query::<(&CardText, &Index, &Text)>()
            .iter(app.world())
            .map(|(_, index, text)| (index.0, text.0.clone()))
            .collect();
        rows.sort();
        assert_eq!(rows[1].1, "UNKNOWN PLAYER");
        assert_eq!(rows[2].1, "UNKNOWN VEHICLE");
    }

    /// `R` on the death screen shows the respawn, then starts a fresh life on the living tank: the
    /// death panel does not stay up.
    #[test]
    fn respawn_completes_visibly() {
        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        app.update();
        let panel = |app: &mut App| {
            let mut query = app
                .world_mut()
                .query_filtered::<&Visibility, With<DeathPanel>>();
            *query.single(app.world()).unwrap()
        };
        let texts = |app: &mut App| {
            let mut query = app.world_mut().query::<(&DeathText, &Index, &Text)>();
            let mut rows: Vec<(usize, String)> = query
                .iter(app.world())
                .map(|(_, index, text)| (index.0, text.0.clone()))
                .collect();
            rows.sort();
            rows
        };
        app.world_mut().resource_mut::<Run>().t = beats.death() + 0.5;
        press(&mut app, KeyCode::KeyR);
        app.update();
        assert!(app.world().resource::<Run>().respawned_at.is_some());
        assert_eq!(panel(&mut app), Visibility::Inherited);
        assert_eq!(texts(&mut app)[0].1, RESPAWNING);
        release(&mut app);
        app.world_mut().resource_mut::<Run>().t = beats.death() + 0.5 + RESPAWN_S;
        app.update();
        let run = app.world().resource::<Run>();
        assert!(run.respawned_at.is_none() && run.t < beats.impact);
        assert_eq!(beats.beat(run.t), Beat::Hit);
        assert_eq!(panel(&mut app), Visibility::Hidden);
    }

    /// The shooter stands at the muzzle the record names and the card's range is its distance to
    /// the impact; a record without a muzzle is staged the demo's distance back along its own line.
    /// The kill fixture's muzzle sits off that staging distance, so the two cannot be confused.
    #[test]
    fn the_shooter_stands_at_the_records_muzzle() {
        let library = library();
        let run = Run::new(kill_entry(), &library, &Settings::default(), true);
        let shot = &run.shots[run.kill.unwrap().0];
        let record = &library.records[shot.record];
        let report = &record.report;
        let impact = Vec3::from(report.impact.point);
        let muzzle = Vec3::from(report.muzzle.expect("the kill fixture records a muzzle"));
        assert!(shot.muzzle.distance(muzzle) < 1e-4);
        let range = muzzle.distance(impact);
        let staging = super::super::world::SHOOTER_DISTANCE_M;
        assert!(
            (range - staging).abs() > 10.0,
            "fixture: a muzzle off the staging"
        );
        let [_, _, line] = card_lines(&record.killer, Some(shot.muzzle.distance(impact)));
        assert!(line.starts_with(&format!("{range:.0} m")), "{line}");
        let mut unstaged = report.clone();
        unstaged.muzzle = None;
        let staged = super::super::demo::staged_muzzle(&unstaged);
        assert!((staged.distance(impact) - staging).abs() < 1e-2);
    }

    /// Through the demo's wiring: the death tint is off through the hit, the lesson and beat 3, and
    /// on from the handoff to the death screen, or to the respawn where `R` cuts the kill cam
    /// short; it sits below every other UI node, so it never colours one.
    #[test]
    fn the_death_tint_starts_at_the_handoff_beneath_the_ui() {
        let tint = |app: &mut App| {
            let mut query = app
                .world_mut()
                .query_filtered::<&Visibility, With<DeathTint>>();
            *query.single(app.world()).expect("one death tint")
        };
        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        app.update();
        let beats = app.world().resource::<Run>().kill.unwrap().1;
        assert!(beats.shooter);
        let mut t = beats.impact;
        while t < beats.end() {
            app.world_mut().resource_mut::<Run>().t = t;
            app.update();
            let want = if beats.beat(t) == Beat::Death {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            assert_eq!(tint(&mut app), want, "t {t}: {:?}", beats.beat(t));
            t += 1.0 / 10.0;
        }

        let mut z = app
            .world_mut()
            .query_filtered::<(Option<&GlobalZIndex>, Has<DeathTint>), With<Node>>();
        let mut rest = Vec::new();
        let mut own = None;
        for (index, is_tint) in z.iter(app.world()) {
            let index = index.map_or(0, |index| index.0);
            if is_tint {
                own = Some(index);
            } else {
                rest.push(index);
            }
        }
        let own = own.expect("the tint is a node");
        assert!(!rest.is_empty());
        assert!(rest.iter().all(|&other| other > own), "{own} vs {rest:?}");

        let mut app = demo_app(kill_entry(), Shooting::Spawned);
        app.update();
        app.world_mut().resource_mut::<Run>().t = beats.lesson() + 1.0;
        app.update();
        assert_eq!(tint(&mut app), Visibility::Hidden, "the lesson is untinted");
        press(&mut app, KeyCode::KeyR);
        app.update();
        assert!(app.world().resource::<Run>().respawned_at.is_some());
        assert_eq!(
            tint(&mut app),
            Visibility::Inherited,
            "the respawn is tinted"
        );
    }

    /// Through the demo's wiring, every kill's card names its round by the registry's display
    /// name, never by the record's registry key.
    #[test]
    fn the_card_names_the_round_never_its_key() {
        let library = library();
        let registry = crate::rounds::RoundRegistry::shipped();
        for entry in 0..PLAYLIST.len() {
            if Run::new(entry, &library, &Settings::default(), true)
                .kill
                .is_none()
            {
                continue;
            }
            let mut app = demo_app(entry, Shooting::Spawned);
            app.update();
            let (shot, beats) = app.world().resource::<Run>().kill.unwrap();
            app.world_mut().resource_mut::<Run>().t = beats.death() + 1e-4;
            app.update();
            let run = app.world().resource::<Run>();
            let key = library.records[run.shots[shot].record].report.round.clone();
            let line = app
                .world_mut()
                .query::<(&CardText, &Index, &Text)>()
                .iter(app.world())
                .find(|(_, index, _)| index.0 == 3)
                .map(|(_, _, text)| text.0.clone())
                .expect("the card's round line");
            let name = registry.name(&key).expect("the kill's round is registered");
            assert!(line.contains(name), "{line}");
            assert!(!line.contains(&key), "{line} shows the key {key}");
        }
    }

    /// The legend names `=` and what `R` does on the run.
    #[test]
    fn the_legend_names_the_kill_keys() {
        assert!(super::super::demo::legend_r(true).contains("respawn"));
        assert!(super::super::demo::legend_r(false).contains("replay"));
    }
}
