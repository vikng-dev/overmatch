//! The victim kill cam: what the struck player sees when a record knocks their own tank out. Four
//! beats on the run's clock, counted from the killing shot's world impact ([`Beats`]):
//!
//! 1. **Hit** — the player's own orbit view, free, the world showing the hit in real time.
//! 2. **Lesson** — captive: the look eases to a three-quarter from the shot's side; the main camera
//!    swaps to [`CameraProfile::KillCamLesson`], so the tank it rides leaves the view and the pen
//!    cam stage, which sits on it in the record's frame, draws the record over the live world at
//!    [`LESSON_RATE`] of the corner's tempo. The world clock is untouched.
//! 3. **Killer** — captive: the look turns down the round's line, the orbit's pivot rides the line
//!    back to the shooter's pivot, and the killer card shows name, vehicle, range and round.
//! 4. **Death** — free: the orbit about the shooter, the death panel and the respawn prompt.
//!
//! Respawn is live from [`HIT_S`] after the impact. Every pose is the game's own orbit
//! ([`crate::camera::orbit_rotation`], [`crate::camera::orbit_body`],
//! [`crate::camera::orbit_pivot`]) about one of three pivots: the victim's, a pivot travelling the
//! line, the shooter's. Which entries play it: a victim entry whose record knocks the tank out by
//! the game's own rule ([`knockout`]). Plan and owner record:
//! `.agents/scratch/damage-cam-2026-10-08/demo/killcam-plan.md`. Every duration and angle here is
//! TUNED (a look-test choice).

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::timeline::{self, CornerKind};
use super::view::{self, ViewRoot, Views};
use super::world::MainCamera;
use super::{Library, PLAYLIST, Run, Settings, Showing, Surface};
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
/// Beat 3: the look turns down the line, the ride, the hold on the shooter.
const ALIGN_S: f32 = 0.4;
const RIDE_S: f32 = 1.6;
const FRAME_TURN_S: f32 = 0.4;
const KILLER_HOLD_S: f32 = 2.0;
/// The demo's run ends this long into the death screen.
const DEATH_HOLD_S: f32 = 2.0;
/// The lesson's bearing: yawed off the shot's side, the body above the pivot by this slope (the
/// corner's three-quarter).
const LESSON_YAW_DEG: f32 = 55.0;
const LESSON_SLOPE: f32 = 0.5;
/// The shooter's framing: yawed off the line, looking down.
const FRAME_YAW_DEG: f32 = 30.0;
const FRAME_PITCH_DEG: f32 = 12.0;
/// The player's look at the hit, before any input: along the hull's forward, looking down. A
/// staging choice; in play it is wherever the player was looking.
const PLAYER_PITCH_DEG: f32 = 12.0;
/// The killer card's enter, and the prompts' strength before and at the death screen.
const CARD_ENTER_S: f32 = 0.18;
const PROMPT_DIM: f32 = 0.6;
/// The card's accent bar.
const ACCENT_PX: f32 = 3.0;

/// Who fired the killing round, as the victim's copy of the record names them.
#[derive(Clone, Debug, PartialEq)]
pub struct Killer {
    pub name: String,
    /// The shooter's vehicle, as its tank file names it to players.
    pub vehicle: String,
    /// The round, by its key in the round registry.
    pub round: String,
    /// The round's muzzle speed (m/s), from the registry.
    pub muzzle_speed: f32,
}

impl Killer {
    /// The killer of a record fired by `shooter` with the round `round` (a round registry key):
    /// the vehicle read from the shooter's own tank file, the muzzle speed from the registry.
    pub fn of(shooter: &Shooter, round: &str) -> Result<Self, String> {
        let path = crate::assets::asset_root().join(&shooter.vehicle);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let spec: TankSpec =
            ron::de::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let fired = crate::rounds::RoundRegistry::shipped()
            .get(round)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            name: shooter.name.clone(),
            vehicle: spec.display_name.clone(),
            round: round.to_owned(),
            muzzle_speed: fired.muzzle_speed,
        })
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
        .filter(|component| component.hp > 0.0 && !lost(&component.name))
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
    pub fn captive(self) -> bool {
        matches!(self, Self::Lesson | Self::Killer)
    }
}

/// The sequence's beats in run seconds, from the killing shot's world impact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beats {
    pub impact: f32,
}

impl Beats {
    pub fn lesson(self) -> f32 {
        self.impact + HIT_S
    }
    pub fn killer(self) -> f32 {
        self.lesson() + LESSON_S
    }
    fn ride(self) -> f32 {
        self.killer() + ALIGN_S
    }
    pub fn arrive(self) -> f32 {
        self.ride() + RIDE_S
    }
    pub fn death(self) -> f32 {
        self.arrive() + KILLER_HOLD_S
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
    pub shooter_pivot: Vec3,
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

    /// Down the round's line toward the shooter.
    fn line_look(&self) -> Quat {
        look_along(-self.incoming())
    }

    /// The shooter framed: a three-quarter from the victim's side, looking down.
    fn frame_look(&self) -> Quat {
        let side = Quat::from_rotation_y(FRAME_YAW_DEG.to_radians()) * self.incoming_flat();
        let pitch = FRAME_PITCH_DEG.to_radians();
        look_along(-(side * pitch.cos() + Vec3::Y * pitch.sin()))
    }

    /// The pivot travelling the round's line from the victim's pivot to the shooter's at ride
    /// fraction `e`: the line's point, offset by the two pivots' offsets from the line's ends,
    /// blended.
    fn ride_pivot(&self, e: f32) -> Vec3 {
        let on_line = self.impact.lerp(self.muzzle, e);
        on_line + (self.victim_pivot - self.impact).lerp(self.shooter_pivot - self.muzzle, e)
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
        let drift = |t: f32| {
            if reduce {
                0.0
            } else {
                timeline::ORBIT_DEG_S.to_radians() * (t - beats.lesson()).max(0.0)
            }
        };
        match beats.beat(t) {
            Beat::Hit => (self.victim_pivot, hit),
            Beat::Lesson => {
                let w = timeline::ease_standard(timeline::progress(t, beats.lesson(), MOVE_S));
                (self.victim_pivot, hit.slerp(self.lesson_look(drift(t)), w))
            }
            Beat::Killer if t < beats.ride() => {
                let w = timeline::ease_standard(timeline::progress(t, beats.killer(), ALIGN_S));
                let from = self.lesson_look(drift(beats.killer()));
                (self.victim_pivot, from.slerp(self.line_look(), w))
            }
            Beat::Killer => {
                let e = timeline::ease_standard(timeline::progress(t, beats.ride(), RIDE_S));
                let turn = timeline::ease_standard(timeline::progress(
                    t,
                    beats.arrive() - FRAME_TURN_S,
                    FRAME_TURN_S,
                ));
                (
                    self.ride_pivot(e),
                    self.line_look().slerp(self.frame_look(), turn),
                )
            }
            Beat::Death => (self.shooter_pivot, death),
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
        }
    }
}

/// The shooter's tank on the battlefield: its view root, the tank file it was loaded from, and
/// the root under its hull node (`hull pose`⁻¹) once that node exists.
struct ShooterView {
    root: Entity,
    vehicle: String,
    spec: Handle<TankSpec>,
    hull_inverse: Option<Mat4>,
}

/// The shooter's tank file and its view glb, as the record names them.
pub fn shooter_scene(report: &PenetrationReport) -> Option<(String, String)> {
    let vehicle = &report.shooter.as_ref()?.vehicle;
    Some((vehicle.clone(), view::view_glb(vehicle)))
}

/// The killer card's three data lines, each reduced to what the font draws and named by what is
/// missing where nothing legible is left.
pub fn card_lines(killer: &Killer, range_m: f32) -> [String; 3] {
    let shown = |text: &str, missing: &str| drawable(text).unwrap_or_else(|| missing.to_owned());
    [
        shown(&killer.name, "UNKNOWN PLAYER"),
        shown(&killer.vehicle, "UNKNOWN VEHICLE"),
        format!(
            "{range_m:.0} m    {}    {:.0} m/s",
            shown(&killer.round, "UNKNOWN ROUND"),
            killer.muzzle_speed
        ),
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

/// A text row's place in its block.
#[derive(Component)]
pub(super) struct Index(usize);

/// The respawn stand-in: after `R`, the death panel reads this for [`RESPAWN_S`], then the run
/// starts a fresh life.
pub const RESPAWNING: &str = "RESPAWNING";
pub const RESPAWN_S: f32 = 0.8;

pub fn spawn(mut commands: Commands, fonts: Res<UiFonts>) {
    commands.insert_resource(KillCam::default());
    let text = |font: &Handle<Font>, px: f32, color: Color| {
        (
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(px),
                ..default()
            },
            TextColor(color),
        )
    };
    // The killer card, lower left: clear of the framed shooter at the centre.
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
        });
    // The death panel, centre, over the live world.
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
        });
    // The respawn prompt, bottom-centre.
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
            root.spawn((
                Text::new("R  RESPAWN"),
                text(&fonts.hud, 20.0, TEXT),
                Prompt,
            ));
        });
}

/// Whether the run plays the kill cam: the run condition of its camera writer.
pub fn active(run: Res<Run>) -> bool {
    run.kill.is_some()
}

/// Whether no kill cam owns the main camera.
pub fn inactive(run: Res<Run>) -> bool {
    run.kill.is_none()
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
/// the kill cam's orbit is placed from.
#[allow(clippy::too_many_arguments)]
pub fn stage(
    run: Res<Run>,
    library: Res<Library>,
    mut views: ResMut<Views>,
    specs: Res<Assets<TankSpec>>,
    asset_server: Res<AssetServer>,
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
                WorldAssetRoot(handle),
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
            hull_inverse: None,
        });
        return;
    }
    let Some(shooter) = cam.shooter.as_mut() else {
        return;
    };
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

    // The rig: both pivots by the game's own formula, the turret ring of each tank lifted.
    let (Some((shot_index, _)), Some(shooter_root), Some(ground_y)) =
        (run.kill, shooter_root, views.ground_y)
    else {
        return;
    };
    let (Some(victim_spec), Some(shooter_spec)) =
        (specs.get(views.spec()), specs.get(&shooter.spec))
    else {
        return;
    };
    let turret = |root: Entity, spec: &TankSpec| {
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
    let (Some(victim_turret), Some(shooter_turret)) = (
        turret(views.world, victim_spec),
        turret(shooter.root, shooter_spec),
    ) else {
        return;
    };
    let victim_root = transforms
        .get(views.world)
        .ok()
        .copied()
        .unwrap_or_default();
    let shot = &run.shots[shot_index];
    cam.rig = Some(Rig {
        victim_pivot: orbit_pivot(&victim_root, victim_turret),
        shooter_pivot: orbit_pivot(&shooter_root, shooter_turret),
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
        cam.death = Some(rig.frame_look());
    }
    let death = cam.death.unwrap_or_else(|| rig.frame_look());
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

/// The killer card, the death panel and the respawn prompt.
#[allow(clippy::type_complexity)]
pub fn hud(
    run: Res<Run>,
    library: Res<Library>,
    mut card: Query<(&mut Visibility, &mut UiTransform), (With<Card>, Without<DeathPanel>)>,
    mut accent: Query<&mut BackgroundColor, With<CardAccent>>,
    mut card_text: Query<(&Index, &mut Text, &mut TextColor), (With<CardText>, Without<DeathText>)>,
    mut panel: Query<&mut Visibility, (With<DeathPanel>, Without<Card>)>,
    mut panel_text: Query<(&Index, &mut Text), (With<DeathText>, Without<CardText>)>,
    mut prompt: Query<(&ChildOf, &mut TextColor), (With<Prompt>, Without<CardText>)>,
    mut visibility: Query<&mut Visibility, (Without<Card>, Without<DeathPanel>)>,
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
        let range = shot.muzzle.distance(Vec3::from(record.report.impact.point));
        let lines = record
            .killer
            .as_ref()
            .map(|killer| card_lines(killer, range));
        for (index, mut text, mut color) in &mut card_text {
            let line = match (index.0, &lines) {
                (0, _) => "KNOCKED OUT BY".to_owned(),
                (row, Some(lines)) => lines[row - 1].clone(),
                _ => String::new(),
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
    if let Ok(mut v) = panel.single_mut() {
        v.set_if_neq(if death {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
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
    use super::super::Corner;
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
            shooter_pivot: Vec3::new(muzzle.x, 4.4, muzzle.z),
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
                    && c.hp > 0.0
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
            let killer = record.killer.as_ref().expect("a kill names its killer");
            assert!(!killer.name.is_empty() && !killer.vehicle.is_empty());
            assert!(killer.muzzle_speed > 0.0);
            assert!(
                run.turns.is_empty(),
                "{}: the corner opens on a kill",
                entry.label
            );
        }
    }

    /// The beats run in order; the camera is captive exactly in the lesson and the killer beats;
    /// respawn is live from the end of the hit; by the lesson's end the record's corner clock is
    /// past its last event (the travel window's end, a cone's growth, a component's fill).
    #[test]
    fn the_beats_run_in_order() {
        let beats = Beats { impact: 2.0 };
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
        let last_event =
            CornerKind::Glass.line_window().1 + timeline::CONE_GROW_S + timeline::BOX_FILL_S;
        assert!(beats.lesson_c(beats.killer() - 1e-3).unwrap() >= last_event);
        assert!(beats.lesson_c(beats.killer()).is_none());
    }

    /// The camera never jumps: at every beat boundary its pose is continuous.
    #[test]
    fn the_camera_is_continuous_across_beats() {
        let rig = rig();
        let beats = Beats { impact: 2.0 };
        let hit = Rig::player_look();
        let death = rig.frame_look();
        for boundary in [
            beats.lesson(),
            beats.lesson() + MOVE_S,
            beats.killer(),
            beats.ride(),
            beats.arrive() - FRAME_TURN_S,
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

    /// The lesson is the game's orbit about the victim's pivot at the play radius, and the ride
    /// keeps the camera on the round's line, off it by no more than the pivots' offsets, until the
    /// look turns to frame the shooter; the ride ends at the play radius from the shooter's pivot,
    /// looking at it.
    #[test]
    fn the_camera_orbits_then_rides_the_line() {
        let rig = rig();
        let beats = Beats { impact: 2.0 };
        let hit = Rig::player_look();
        let death = rig.frame_look();
        let mut t = beats.lesson();
        while t < beats.killer() {
            let pose = rig.pose(beats, t, hit, death, false);
            let r = pose.translation.distance(rig.victim_pivot);
            assert!(r <= rig.radius + 1e-3, "t {t}: {r} m off the pivot");
            t += 1.0 / 30.0;
        }
        let incoming = rig.incoming();
        let off_line = |p: Vec3| {
            let v = p - rig.impact;
            (v - incoming * v.dot(incoming)).length()
        };
        let perpendicular = |v: Vec3| (v - incoming * v.dot(incoming)).length();
        let bound = perpendicular(rig.victim_pivot - rig.impact)
            .max(perpendicular(rig.shooter_pivot - rig.muzzle))
            + 1e-2;
        let mut t = beats.ride();
        while t < beats.arrive() - FRAME_TURN_S {
            let pose = rig.pose(beats, t, hit, death, false);
            assert!(off_line(pose.translation) <= bound, "t {t}: off the line");
            t += 1.0 / 30.0;
        }
        let end = rig.pose(beats, beats.arrive(), hit, death, false);
        assert!((end.translation.distance(rig.shooter_pivot) - rig.radius).abs() < 1e-2);
        let to_pivot = (rig.shooter_pivot - end.translation).normalize();
        assert!(end.forward().dot(to_pivot) > 0.9999);
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
    fn crew_report(crew: &[f32], lost: &[usize], cooked: bool) -> PenetrationReport {
        let mut report = library().records[0].report.clone();
        report.components = crew
            .iter()
            .enumerate()
            .map(|(index, &hp)| crate::damage_cam::ComponentState {
                name: format!("Crew{index}"),
                role: Role::Crew,
                count: 3.0,
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
        let cases: [(&[f32], &[usize], bool, Option<KnockoutReason>); 7] = [
            // The shot kills the last of them.
            (&[3.0, 3.0], &[0, 1], false, Some(KnockoutReason::CrewLoss)),
            // One survives the shot.
            (&[3.0, 3.0], &[0], false, None),
            // Earlier hits left one; the shot kills him.
            (
                &[0.0, 0.0, 3.0],
                &[2],
                false,
                Some(KnockoutReason::CrewLoss),
            ),
            // Earlier hits left one; the shot misses him.
            (&[0.0, 0.0, 3.0], &[0], false, None),
            // No crew volumes: crew loss cannot be read.
            (&[], &[], false, None),
            // A cookoff, whatever the crew.
            (&[3.0, 3.0], &[], true, Some(KnockoutReason::Cookoff)),
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
    /// is unknown, a partly drawable one keeps what draws.
    #[test]
    fn the_card_draws_only_what_the_font_covers() {
        let killer = Killer {
            name: "\u{864E}\u{738B}".into(),
            vehicle: "Tiger \u{864E} I".into(),
            round: "88mm_AP".into(),
            muzzle_speed: 773.0,
        };
        let [name, vehicle, round] = card_lines(&killer, 700.0);
        assert_eq!(name, "UNKNOWN PLAYER");
        assert_eq!(vehicle, "Tiger I");
        assert!(round.contains("88mm_AP") && round.contains("700 m"));
    }

    /// The camera writers and the input gate in the demo's own order, on a headless app with the
    /// K1 run and a fixture rig.
    fn camera_app(entry: usize) -> App {
        let library = library();
        let settings = Settings::default();
        let run = Run::new(entry, &library, &settings, true);
        let mut app = App::new();
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Left);
        app.insert_resource(library)
            .insert_resource(settings)
            .insert_resource(run)
            .insert_resource(mouse)
            .insert_resource(AccumulatedMouseMotion {
                delta: Vec2::new(40.0, 0.0),
            })
            .insert_resource(KillCam {
                rig: Some(rig()),
                ..KillCam::default()
            })
            .add_systems(Update, (input, super::super::camera_writers()).chain());
        app.world_mut().spawn((
            Transform::IDENTITY,
            Projection::Perspective(PerspectiveProjection::default()),
            CameraProfile::BattlefieldThirdPerson,
            MainCamera,
        ));
        app
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

    /// Across the real handoffs the player's drag turns the camera only while it is free (the hit
    /// and the death screen), the kill cam is the camera's writer in every beat, and the lesson's
    /// profile swaps the ridden tank for its glass.
    #[test]
    fn the_drag_moves_the_camera_only_while_it_is_free() {
        let mut app = camera_app(kill_entry());
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
            let rig = rig();
            let want = rig.pose(
                beats,
                t,
                cam.hit,
                cam.death.unwrap_or_else(|| rig.frame_look()),
                false,
            );
            let mut camera = app
                .world_mut()
                .query_filtered::<(&Transform, &CameraProfile), With<MainCamera>>();
            let (transform, profile) = camera.single(app.world()).unwrap();
            assert!(
                transform.translation.distance(want.translation) < 1e-3,
                "{beat:?}: the camera is not the kill cam's"
            );
            assert_eq!(
                *profile == CameraProfile::KillCamLesson,
                beat == Beat::Lesson,
                "{beat:?}"
            );
        }
    }

    /// On a run without a kill the world places the camera and the kill cam's input is inert.
    #[test]
    fn without_a_kill_the_world_places_the_camera() {
        let mut app = camera_app(0);
        app.update();
        let run = app.world().resource::<Run>();
        let muzzle = run.shots[0].muzzle;
        let mut camera = app
            .world_mut()
            .query_filtered::<&Transform, With<MainCamera>>();
        let transform = camera.single(app.world()).unwrap();
        assert!(transform.translation.distance(muzzle) < 1e-3);
        assert_eq!(app.world().resource::<KillCam>().hit, Rig::player_look());
    }

    /// `R` on the death screen shows the respawn, then starts a fresh life on the living tank: the
    /// death panel does not stay up.
    #[test]
    fn respawn_completes_visibly() {
        let library = library();
        let settings = Settings::default();
        let entry = kill_entry();
        let run = Run::new(entry, &library, &settings, false);
        let beats = run.kill.unwrap().1;
        let mut app = App::new();
        app.insert_resource(library)
            .insert_resource(settings)
            .insert_resource(run)
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(ButtonInput::<MouseButton>::default())
            .insert_resource(AccumulatedMouseMotion::default())
            .insert_resource(UiFonts {
                hud: Handle::default(),
                body: Handle::default(),
            })
            .init_resource::<Corner>()
            .init_resource::<Time>()
            .add_systems(Startup, spawn)
            .add_systems(
                Update,
                (super::super::controls, super::super::advance, hud).chain(),
            );
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
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyR);
        app.update();
        assert!(app.world().resource::<Run>().respawned_at.is_some());
        assert_eq!(panel(&mut app), Visibility::Inherited);
        assert_eq!(texts(&mut app)[0].1, RESPAWNING);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::KeyR);
            keys.clear();
        }
        app.world_mut().resource_mut::<Run>().t = beats.death() + 0.5 + RESPAWN_S;
        app.update();
        let run = app.world().resource::<Run>();
        assert!(run.respawned_at.is_none() && run.t < beats.impact);
        assert_eq!(beats.beat(run.t), Beat::Hit);
        assert_eq!(panel(&mut app), Visibility::Hidden);
    }

    /// The legend names `=` and what `R` does on the run.
    #[test]
    fn the_legend_names_the_kill_keys() {
        assert!(super::super::hud::legend_r(true).contains("respawn"));
        assert!(super::super::hud::legend_r(false).contains("replay"));
    }
}
