//! The victim kill cam in play: the struck player's own tank knocked out by a recorded shot, shown
//! in the four beats of the demo's kill cam ([`super::super::killcam`]) on the real clock, then the
//! death screen.
//!
//! * **Trigger.** The player's controlled tank carries the knockout fact ([`TankKnockedOut`], the
//!   replicated status, never a client guess). The cam plays the explanation of that life whose
//!   audience is the killed victim's (`ShotExplanation::lesson`) and whose report names the
//!   knockout (`killed`), once its impact is drawn ([`ImpactPresence::Presented`]). A shooter's
//!   copy, a victim projection without `killed` and a record of another life never play it.
//! * **Either order.** The fact and the record may arrive in either order. The clock starts at the
//!   fact; the record is waited for [`RECORD_WAIT_S`] from it. One in hand by then plays, its
//!   lesson starting no sooner than [`HIT_S`] after the fact and no sooner than its arrival.
//! * **Fallback.** No record in the window, a knockout without a killing shot, or the setting off:
//!   the death screen at the window's end (or at once, off), respawn never delayed. A killing record
//!   in hand while dead and not playing (late, drawn or not) offers the replay key. A life's cam
//!   plays on its own at most once.
//! * **Beats** ([`Beats`], seconds from the clock's zero): hit, the player's own view; lesson and
//!   killer, the camera captive ([`CameraCaptive`]) on the game's own orbit about the victim's
//!   pivot, the lesson's profile [`CameraProfile::KillCamLesson`] with the stage over the live world
//!   in the record's frame (the struck hull's pose at contact); death, the camera handed back where
//!   the killer beat left it, under the death screen.
//! * **Keys** (`command::Bindings`), none before [`HIT_S`] from the fact, and the game's respawn and
//!   map likewise ([`KillCamGate::settling`]): skip ends the run at the death screen, hold freezes the
//!   presentation clock (the orbit drifts on), replay plays the held record again from the lesson.
//!   Respawn and the map are never blocked after the dead-zone: either ends the run.
//! * **The device.** The death flow owns the player's device from the knockout fact to a live
//!   tank (`command::DeviceOwner`), so the kill cam's keys may share a tank binding.
//! * **Order** (`Update`). [`drive`] runs in [`KillCamGateSet`], before `overlay::OverlaySet::
//!   Declare` and so before every reader of the [`KillCamGate`] it writes: the death screen there,
//!   then the map's toggle and the respawn key, both after `OverlaySet::Toggle`. They read the gate
//!   of the frame they run in. [`give_way`] runs after them and marks a respawn or a map taken; the
//!   next frame's drive ends the run, so the camera, the gate and the HUD turn together.
//! * **Teardown.** A new life, no tank, a disconnect ([`super::CloseCorner`]), the spawn map, a
//!   change of the kill cam or reduce-motion setting: the run ends, the camera is handed back on
//!   the third-person profile and the stage goes with the slot's [`super::Rig::teardown`]. The
//!   killing record is kept through a disconnect: back on the same dead life, the death screen
//!   offers the replay (presentation only; never re-requested).
//!
//! The slot is closed by the knockout and stays closed while the tank is knocked out, so the kill
//! cam is the only writer of [`Corner`] then: one surface at a time. Every duration is the demo's,
//! TUNED.

use std::sync::Arc;

use avian3d::prelude::SpatialQuery;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use super::super::killcam::{
    self as demo, Beat, Beats, Card, CardAccent, CardText, DeathPanel, DeathText, DeathTint, HIT_S,
    Index, Killer, Prompt, card_lines, lesson_showing,
};
use super::super::plan::Plan;
use super::super::stage::StageShell;
use super::super::timeline::{self, CornerKind};
use super::super::{Corner, CornerRig, Exhibit, WorldViewer};
use super::{CloseCorner, Rig as SlotRig, Slot};
use crate::bake::TankBlueprint;
use crate::camera::{
    CameraCaptive, CaptiveCameraPlaced, OrbitCamera, TurretPivot, orbit_body, orbit_pivot, view_fov,
};
use crate::command::{Bindings, ButtonBinding, TankCommand};
use crate::damage::TankKnockedOut;
use crate::damage_cam::delivery::{ImpactPresence, ShotExplanations, StoredExplanation};
use crate::damage_cam::wire::TankLife;
use crate::overlay::{Overlay, OverlaySet, Overlays};
use crate::render_policy::CameraProfile;
use crate::settings::{KillCam as KillCamSetting, ReduceMotion, Settings};
use crate::sight::SightMode;
use crate::spec::{TankSpecHandle, ViewKind};
use crate::tank::{Controlled, Tank, TankViews, ViewNode, rig_world_pose};
use crate::ui_font::UiFonts;
use crate::view::PlayerView;
use crate::world::ground_distance;
use crate::{CombatantId, CombatantLife};

/// How long the kill cam waits for the killing record from the knockout fact, real seconds. TUNED:
/// delivery's impact timeout (`delivery::IMPACT_TIMEOUT_S`), the reference.
const RECORD_WAIT_S: f32 = 1.0;

/// The replay prompt. ASCII only — it reaches `Text`.
const REPLAY: &str = "SPACE  REPLAY";

/// What the kill cam asks of the game's death screen and of its respawn and map keys this frame,
/// written by [`drive`] in [`KillCamGateSet`] before any of them reads it.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct KillCamGate {
    /// The death screen stays down: the cam waits for its record or plays its first three beats.
    pub(crate) holds_death_screen: bool,
    /// The knockout is younger than the input dead-zone ([`HIT_S`]): respawn and the map do
    /// nothing yet.
    pub(crate) settling: bool,
}

/// The kill cam's [`drive`]: before `overlay::OverlaySet::Declare`, where the first reader of the
/// [`KillCamGate`] it writes runs; the others run after `OverlaySet::Toggle`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct KillCamGateSet;

/// Present where the composition mounts the game's death screen (the network client's). Without
/// it, the kill cam draws its own stand-in: the death tint and a panel naming the knockout.
#[derive(Resource, Default)]
pub(crate) struct GameDeathScreen;

/// The record that killed a life, as the kill cam keeps it.
#[derive(Clone)]
struct Killing {
    life: TankLife,
    stored: StoredExplanation,
    /// Whether it was in hand, drawn, within [`RECORD_WAIT_S`] of the fact: only then may it play
    /// on its own.
    timely: bool,
    /// App seconds it was first in hand, from the clock's zero.
    after_s: f32,
    killer: Killer,
    /// Muzzle to impact (m), where the record has the muzzle.
    range_m: Option<f32>,
}

/// Where beat 3 looks: the shooter's live tank, or where its muzzle fired from.
#[derive(Clone, Copy, Debug)]
enum ShooterAt {
    /// A live tank and its turret ring's offset in its root's frame.
    Tank(Entity, Vec3),
    Point(Vec3),
}

/// A run of the beats.
#[derive(Clone, Copy, Debug)]
struct Run {
    /// App seconds at the run's zero.
    zero: f64,
    beats: Beats,
    /// Presentation seconds the hold key has frozen.
    held: f32,
    /// The presentation clock and the real one since the zero, as this frame's drive left them.
    t: f32,
    real: f32,
    /// The player's look when the camera was taken.
    hit: Option<Quat>,
    /// The record's frame in the world: the struck hull at contact.
    frame: Transform,
    impact: Vec3,
    muzzle: Vec3,
    shooter: Option<ShooterAt>,
    /// The orbit radius the player's zoom set when the run began.
    radius: f32,
}

impl Run {
    fn beat(&self) -> Beat {
        self.beats.beat(self.t)
    }

    fn captive(&self) -> bool {
        matches!(self.beat(), Beat::Lesson | Beat::Killer)
    }
}

/// The kill cam's state between frames.
#[derive(Resource, Default)]
pub(super) struct Victim {
    /// The knocked-out life served.
    death: Option<Death>,
    record: Option<Killing>,
    /// The kill cam's UI exists.
    ui: bool,
}

impl Victim {
    fn run(&self) -> Option<Run> {
        self.death.as_ref().and_then(|death| death.run)
    }
}

/// A knocked-out life the kill cam serves.
struct Death {
    life: TankLife,
    /// App seconds at its knockout fact.
    zero: f64,
    run: Option<Run>,
    /// Whether this life's cam has played on its own, or will not.
    resolved: bool,
    /// The settings the run plays by: a change ends it.
    by: (KillCamSetting, ReduceMotion),
    /// A respawn or the spawn map was taken ([`give_way`]): the next drive ends the run and the
    /// wait for one.
    yielded: bool,
}

impl Death {
    /// The death screen stays down: a run plays, or the wait for one is open.
    fn holds_death_screen(&self) -> bool {
        self.run.is_some() || !self.resolved
    }
}

/// The kill cam's UI roots.
#[derive(Component)]
struct KillCamUi;

/// The stage's place in the world while the lesson draws it: the record's frame. The stage view's
/// root and the shell hang under it, so the slot's teardown takes all three at once.
#[derive(Component)]
struct StageAnchor;

/// The kill cam's state and systems, beside the slot's ([`super::wire`]); its [`drive`] and
/// [`give_way`] run in the slot's chain.
pub(super) fn wire(app: &mut App) {
    app.init_resource::<Victim>()
        .init_resource::<KillCamGate>()
        .init_resource::<CameraCaptive>()
        .configure_sets(Update, KillCamGateSet.before(OverlaySet::Declare))
        .add_observer(disconnect)
        .add_systems(PostUpdate, place.in_set(CaptiveCameraPlaced))
        .add_systems(Update, hud.after(drive));
}

/// Hand the camera back on the third-person profile, the lesson's mask owner gone, if the kill cam
/// holds it.
fn release(
    captive: &mut CameraCaptive,
    commands: &mut Commands,
    camera: Option<(Entity, &CameraProfile)>,
) {
    if !captive.0 {
        return;
    }
    captive.0 = false;
    if let Some((entity, profile)) = camera {
        let mut camera = commands.entity(entity);
        camera.remove::<WorldViewer>();
        if *profile == CameraProfile::KillCamLesson {
            camera.insert(CameraProfile::BattlefieldThirdPerson);
        }
    }
}

/// A disconnect: the run and the camera end here; the slot's own observer takes the stage. The
/// killing record stays for a reconnect to the same dead life.
fn disconnect(
    _close: On<CloseCorner>,
    mut victim: ResMut<Victim>,
    mut captive: ResMut<CameraCaptive>,
    mut gate: ResMut<KillCamGate>,
    mut commands: Commands,
    camera: Query<(Entity, &CameraProfile), With<PlayerView>>,
    ui: Query<Entity, With<KillCamUi>>,
) {
    victim.death = None;
    release(&mut captive, &mut commands, camera.single().ok());
    for entity in &ui {
        commands.entity(entity).despawn();
    }
    victim.ui = false;
    *gate = KillCamGate::default();
}

/// The player's tank as the kill cam reads it.
type OwnTank<'a> = (
    Entity,
    &'a CombatantId,
    &'a CombatantLife,
    Option<&'a TankKnockedOut>,
    Option<&'a crate::tank::Rig>,
);

/// The kill cam's keys, and the spawn map (no replay under it).
#[derive(SystemParam)]
pub(super) struct Keys<'w> {
    keys: Option<Res<'w, ButtonInput<KeyCode>>>,
    mouse: Option<Res<'w, ButtonInput<MouseButton>>>,
    bindings: Option<Res<'w, Bindings>>,
    overlays: Option<Res<'w, Overlays>>,
}

impl Keys<'_> {
    /// Whether `pick`'s binding was pressed this frame.
    fn just_pressed(&self, pick: fn(&Bindings) -> ButtonBinding) -> bool {
        self.devices()
            .is_some_and(|(keys, mouse, bindings)| pick(bindings).just_pressed(keys, mouse))
    }

    /// Whether `pick`'s binding is down.
    fn pressed(&self, pick: fn(&Bindings) -> ButtonBinding) -> bool {
        self.devices()
            .is_some_and(|(keys, mouse, bindings)| pick(bindings).pressed(keys, mouse))
    }

    fn devices(&self) -> Option<(&ButtonInput<KeyCode>, &ButtonInput<MouseButton>, &Bindings)> {
        Some((
            self.keys.as_deref()?,
            self.mouse.as_deref()?,
            self.bindings.as_deref()?,
        ))
    }

    fn map(&self) -> bool {
        self.overlays
            .as_deref()
            .is_some_and(|overlays| overlays.contains(Overlay::SpawnMap))
    }
}

/// The world the kill cam reads: the shooters, the player's camera, the stage's place.
#[derive(SystemParam)]
pub(super) struct Scene<'w, 's> {
    shooters: Query<
        'w,
        's,
        (
            Entity,
            &'static CombatantId,
            Option<&'static crate::tank::Rig>,
            Option<&'static TankSpecHandle>,
        ),
        (With<Tank>, Without<Controlled>),
    >,
    camera: Query<
        'w,
        's,
        (
            Entity,
            &'static Transform,
            &'static CameraProfile,
            Option<&'static OrbitCamera>,
        ),
        With<PlayerView>,
    >,
    view_nodes: Query<'w, 's, &'static ViewNode>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    parents: Query<'w, 's, &'static ChildOf>,
    locals: Query<'w, 's, &'static Transform>,
    anchors: Query<'w, 's, Entity, With<StageAnchor>>,
    shells: Query<'w, 's, (Entity, Option<&'static ChildOf>), With<StageShell>>,
    ui: Query<'w, 's, Entity, With<KillCamUi>>,
    fonts: Option<Res<'w, UiFonts>>,
}

/// The killing record of `life` among `store`'s: the killed victim's projection whose report names
/// the knockout, its impact released.
fn killing(store: &ShotExplanations, life: TankLife) -> Option<&StoredExplanation> {
    store.live(life).find(|stored| {
        stored.explanation.lesson()
            && stored.report.killed.is_some()
            && stored.presence != ImpactPresence::Pending
    })
}

/// Follow the player's tank's knockout, take its killing record, run the beats on the real clock,
/// read the keys, keep the stage for the lesson, and say what the death screen may show.
#[expect(
    clippy::too_many_arguments,
    reason = "the kill cam's driver: the clock, the settings, the record store, the blueprint, the keys, the cursor, the run, the gate, the captive camera, the sight, the corner slot and its rig, the own tank and the scene"
)]
pub(super) fn drive(
    time: Res<Time>,
    settings: Option<Res<Settings>>,
    store: Option<Res<ShotExplanations>>,
    blueprint: Option<Res<TankBlueprint>>,
    keys: Keys,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut victim: ResMut<Victim>,
    mut gate: ResMut<KillCamGate>,
    mut captive: ResMut<CameraCaptive>,
    mut sight: Option<ResMut<SightMode>>,
    mut slot: ResMut<Slot>,
    mut rig: SlotRig,
    own: Query<OwnTank, With<Controlled>>,
    scene: Scene,
) {
    let now = time.elapsed_secs_f64();
    let settings = settings.as_deref().copied().unwrap_or_default();
    let by = (settings.kill_cam, settings.reduce_motion);
    let victim = &mut *victim;
    let tank = own.single().ok();
    let dead_life = tank.and_then(|(_, &combatant, &life, knocked_out, _)| {
        knocked_out.map(|_| TankLife { combatant, life })
    });
    let camera_entity = scene
        .camera
        .single()
        .ok()
        .map(|(entity, _, profile, _)| (entity, profile));

    // Whose death this is.
    if dead_life != victim.death.as_ref().map(|death| death.life) {
        if let Some(death) = victim.death.as_mut() {
            end_run(death, &mut captive, &mut rig, &mut slot, camera_entity);
        }
        victim.death = match dead_life {
            Some(life) => {
                // Back on a life whose record is in hand (a reconnect): the death screen, the
                // replay offered, nothing played on its own.
                let known = victim.record.as_ref().is_some_and(|r| r.life == life);
                if !known {
                    victim.record = None;
                }
                Some(Death {
                    life,
                    zero: now,
                    run: None,
                    resolved: known,
                    by,
                    yielded: false,
                })
            }
            None => {
                if tank.is_some() {
                    // A live tank: that death is over.
                    victim.record = None;
                }
                None
            }
        };
    }
    let Some(death) = victim.death.as_mut() else {
        for entity in &scene.ui {
            rig.commands.entity(entity).despawn();
        }
        victim.ui = false;
        *gate = KillCamGate::default();
        return;
    };
    if !victim.ui
        && let Some(fonts) = scene.fonts.as_deref()
    {
        spawn_ui(&mut rig.commands, fonts);
        victim.ui = true;
    }
    let (life, zero) = (death.life, death.zero);
    let since = (now - zero) as f32;
    let settled = since >= HIT_S;

    // A respawn or the map taken last frame ([`give_way`]): the run and the wait for one end here.
    if std::mem::take(&mut death.yielded) {
        end_run(death, &mut captive, &mut rig, &mut slot, camera_entity);
        death.resolved = true;
    }

    // The killing record, once in hand.
    if victim.record.is_none()
        && let Some(stored) = store.as_deref().and_then(|store| killing(store, life))
    {
        let report = &stored.report;
        let range_m = report
            .muzzle
            .map(|muzzle| Vec3::from(muzzle).distance(Vec3::from(report.impact.point)));
        let vehicle = scene
            .shooters
            .iter()
            .find(|(_, id, _, _)| **id == stored.explanation.shot.combatant)
            .and_then(|(_, _, _, spec)| spec?.0.path())
            .map(|path| path.path().to_string_lossy().into_owned());
        victim.record = Some(Killing {
            life,
            stored: stored.clone(),
            timely: stored.presence == ImpactPresence::Presented && since <= RECORD_WAIT_S,
            after_s: since,
            killer: Killer::of_vehicle(vehicle.as_deref(), &report.round),
            range_m,
        });
    }

    // Input: the cursor's, past the dead-zone.
    let input = settled && crate::state::cursor_locked(cursor);
    let skip = input && keys.just_pressed(|b| b.kill_cam_skip);
    let hold = input && keys.pressed(|b| b.kill_cam_hold);
    let replay = input && keys.just_pressed(|b| b.kill_cam_replay);
    let map = keys.map();
    let changed = death.by != by;
    death.by = by;

    // What ends a run, or the wait for one, here: the death screen. The respawn and the map end
    // them in `give_way`.
    if death.run.is_some() && (skip || changed) {
        end_run(death, &mut captive, &mut rig, &mut slot, camera_entity);
        death.resolved = true;
    }
    if !death.resolved && (skip || !by.0.on() || since >= RECORD_WAIT_S && death.run.is_none()) {
        death.resolved = true;
    }

    // A run starts: on its own once, or on the replay key at the death screen.
    let start = match (&victim.record, death.run) {
        (Some(record), None) if !death.resolved && record.timely => {
            death.resolved = true;
            // The lesson opens no sooner than the hit beat's end and the record's arrival.
            Some((
                zero,
                Beats {
                    impact: (record.after_s - HIT_S).max(0.0),
                    shooter: false,
                },
            ))
        }
        (Some(_), None) if death.resolved && replay && !map => Some((
            now,
            Beats {
                impact: -HIT_S,
                shooter: false,
            },
        )),
        _ => None,
    };
    if let (Some((run_zero, mut beats)), Some(record)) = (start, victim.record.as_ref()) {
        let stored = &record.stored;
        let report = &stored.report;
        let frame = match stored.explanation.hull_pose() {
            Ok(Some((at, turn))) => Some(Transform::from_translation(at).with_rotation(turn)),
            _ => tank
                .and_then(|(_, _, _, _, tank_rig)| tank_rig)
                .and_then(|tank_rig| {
                    let node = scene.view_nodes.get(tank_rig.hull).ok();
                    scene
                        .globals
                        .get(ViewNode::resolve(node, tank_rig.hull))
                        .ok()
                })
                .map(GlobalTransform::compute_transform),
        };
        let frame = frame.unwrap_or_default();
        let impact = frame.transform_point(Vec3::from(report.impact.point));
        let muzzle_point = report.muzzle.map(|m| frame.transform_point(Vec3::from(m)));
        let muzzle = muzzle_point
            .unwrap_or_else(|| impact - frame.rotation * Vec3::from(report.impact.incoming));
        let live = scene
            .shooters
            .iter()
            .find(|(_, id, _, _)| **id == stored.explanation.shot.combatant)
            .and_then(|(entity, _, shooter_rig, _)| {
                let (turret, _) = rig_world_pose(
                    shooter_rig?.turret,
                    entity,
                    Vec3::ZERO,
                    Quat::IDENTITY,
                    &scene.parents,
                    &scene.locals,
                )?;
                Some(ShooterAt::Tank(entity, turret))
            });
        let shooter = live.or(muzzle_point.map(ShooterAt::Point));
        beats.shooter = shooter.is_some();
        let radius = scene
            .camera
            .single()
            .ok()
            .and_then(|(_, _, _, orbit)| orbit)
            .map_or(crate::camera::ORBIT_FAR, OrbitCamera::radius);
        death.run = Some(Run {
            zero: run_zero,
            beats,
            held: 0.0,
            t: 0.0,
            real: 0.0,
            hit: None,
            frame,
            impact,
            muzzle,
            shooter,
            radius,
        });
        // The lesson's record, on the stage the slot keeps.
        rig.exhibits.0.clear();
        if let Some(tank_asset) = report
            .vehicle
            .as_deref()
            .and_then(|vehicle| slot.tank(vehicle, blueprint.as_deref()))
        {
            rig.exhibits.0.push(Exhibit {
                plan: Arc::new(Plan::new(report, &tank_asset.shapes(&report.servo_deg))),
                servo_deg: report.servo_deg.clone(),
            });
        }
        info!(
            "kill cam: plays {:?} from {:.2} s after the knockout (record at {:.2} s)",
            stored.explanation.shot,
            (run_zero - zero) as f32,
            record.after_s,
        );
    }

    // The run's clocks, and its end.
    let delta = time.delta_secs();
    if let Some(run) = death.run.as_mut() {
        let real = (now - run.zero) as f32;
        if hold {
            run.held += delta;
        }
        run.real = real;
        run.t = (real - run.held).max(0.0);
    }
    if death.run.is_some_and(|run| run.beat() == Beat::Death) {
        end_run(death, &mut captive, &mut rig, &mut slot, camera_entity);
    }

    // The camera: taken for the lesson and the killer beat, on the third-person view.
    let wants = death.run.is_some_and(|run| run.captive());
    if wants && !captive.0 {
        if let Some(run) = death.run.as_mut()
            && run.hit.is_none()
        {
            run.hit = scene
                .camera
                .single()
                .ok()
                .map(|(_, transform, _, _)| transform.rotation);
        }
        if let Some(sight) = sight.as_mut() {
            sight.set_if_neq(SightMode::ThirdPerson);
        }
        if let Some((entity, _)) = camera_entity {
            rig.commands.entity(entity).insert(WorldViewer);
        }
        captive.0 = true;
    } else if !wants && captive.0 {
        release(&mut captive, &mut rig.commands, camera_entity);
    }

    // The stage, from the run's start through its lesson.
    let staging = death
        .run
        .filter(|run| matches!(run.beat(), Beat::Hit | Beat::Lesson));
    match staging {
        Some(run) => {
            let placed = victim
                .record
                .as_ref()
                .and_then(|record| record.stored.report.vehicle.as_deref())
                .is_some_and(|vehicle| rig.ready(&mut slot, vehicle));
            if let Some(view) = rig.view.as_deref() {
                let anchor = match scene.anchors.iter().next() {
                    Some(anchor) => anchor,
                    None => rig
                        .commands
                        .spawn((run.frame, Visibility::Inherited, StageAnchor, CornerRig))
                        .id(),
                };
                rig.commands.entity(anchor).insert(run.frame);
                if scene.parents.get(view.root).ok().map(ChildOf::parent) != Some(anchor) {
                    rig.commands.entity(view.root).insert(ChildOf(anchor));
                }
                for (shell, parent) in &scene.shells {
                    if parent.map(ChildOf::parent) != Some(anchor) {
                        rig.commands.entity(shell).insert(ChildOf(anchor));
                    }
                }
            }
            let showing = lesson_showing(run.beats, run.t, 0).filter(|_| placed);
            rig.corner.set_if_neq(Corner {
                showing,
                fading: None,
            });
        }
        None => {
            rig.corner.set_if_neq(Corner::default());
        }
    }

    *gate = KillCamGate {
        holds_death_screen: death.holds_death_screen(),
        settling: !settled,
    };
}

/// Mark the game's respawn or spawn map, taken while dead: the next frame's [`drive`] ends the run
/// and the wait for one at the death screen. After the respawn key
/// ([`PlayerInputSet`](crate::state::PlayerInputSet)) and the map's toggle (`OverlaySet::Toggle`).
pub(super) fn give_way(
    overlays: Option<Res<Overlays>>,
    own: Query<&TankCommand, (With<Controlled>, With<TankKnockedOut>)>,
    mut victim: ResMut<Victim>,
) {
    let respawning = own.iter().any(|command| command.respawn);
    let map = overlays
        .as_deref()
        .is_some_and(|overlays| overlays.contains(Overlay::SpawnMap));
    if (respawning || map)
        && let Some(death) = victim.death.as_mut()
    {
        death.yielded = true;
    }
}

/// End the run: the camera handed back and the stage gone.
fn end_run(
    death: &mut Death,
    captive: &mut CameraCaptive,
    rig: &mut SlotRig,
    slot: &mut Slot,
    camera: Option<(Entity, &CameraProfile)>,
) {
    if death.run.take().is_none() {
        return;
    }
    release(captive, &mut rig.commands, camera);
    if rig.exists(slot) {
        rig.teardown(slot);
        slot.reset();
    }
}

/// The captive camera: the game's orbit about the victim's pivot, posed by the beat, at the radius
/// the player's zoom set, pulled in by the ground; the commander's naked-eye field; the lesson's
/// profile in the lesson.
#[expect(
    clippy::too_many_arguments,
    reason = "the captive camera's pose: the run, the settings, the turret pivot, the ground query, the victim and the other tanks, the views and the camera"
)]
fn place(
    captive: Res<CameraCaptive>,
    victim: Res<Victim>,
    settings: Option<Res<Settings>>,
    pivot: Res<TurretPivot>,
    spatial: SpatialQuery,
    tank: Query<&Transform, (With<Tank>, With<Controlled>, Without<PlayerView>)>,
    others: Query<&Transform, (With<Tank>, Without<PlayerView>)>,
    views: Query<&TankViews, With<Controlled>>,
    mut camera: Query<(Entity, &mut Transform, &mut Projection, &CameraProfile), With<PlayerView>>,
    mut commands: Commands,
) {
    if !captive.0 {
        return;
    }
    let (Some(run), Some(turret)) = (victim.run(), pivot.offset()) else {
        return;
    };
    let (Ok(tank), Ok((entity, mut transform, mut projection, profile))) =
        (tank.single(), camera.single_mut())
    else {
        return;
    };
    let reduce = settings.as_deref().is_some_and(|s| s.reduce_motion.on());
    let shooter_pivot = run.shooter.and_then(|shooter| match shooter {
        ShooterAt::Tank(entity, turret) => others
            .get(entity)
            .ok()
            .map(|root| orbit_pivot(root, turret)),
        ShooterAt::Point(point) => Some(point),
    });
    let rig = demo::Rig {
        victim_pivot: orbit_pivot(tank, turret),
        shooter_pivot,
        impact: run.impact,
        muzzle: run.muzzle,
        radius: run.radius,
        // Unread: the ground is the terrain's, taken below.
        ground_y: f32::NEG_INFINITY,
    };
    let hit = run.hit.unwrap_or(transform.rotation);
    let (centre, look) = rig.orbit_drifting(run.beats, run.t, run.real, hit, hit, reduce);
    let forward = Dir3::new(look * Vec3::NEG_Z).unwrap_or(Dir3::NEG_Z);
    let body = orbit_body(centre, forward, run.radius, |ray, max| {
        ground_distance(&spatial, ray, max)
    });
    let want = Transform::from_translation(body).with_rotation(look);
    if *transform != want {
        *transform = want;
    }
    let fov = view_fov(&views, ViewKind::Commander, std::f32::consts::FRAC_PI_4);
    if let Projection::Perspective(p) = projection.as_mut()
        && p.fov != fov
    {
        p.fov = fov;
    }
    let want = if run.beat() == Beat::Lesson {
        CameraProfile::KillCamLesson
    } else {
        CameraProfile::BattlefieldThirdPerson
    };
    if *profile != want {
        commands.entity(entity).insert(want);
    }
}

/// The kill cam's UI, hidden: the killer card, the replay prompt, and the death screen's stand-in.
fn spawn_ui(commands: &mut Commands, fonts: &UiFonts) {
    let roots = [
        demo::spawn_death_tint(commands),
        demo::spawn_card(commands, fonts),
        demo::spawn_death_panel(commands, fonts),
        demo::spawn_prompt(commands, fonts, REPLAY),
    ];
    for root in roots {
        commands.entity(root).insert(KillCamUi);
    }
}

/// Show the kill cam's UI: the killer card from the killer beat's arrival and on the death screen,
/// the replay prompt on the death screen with a record in hand, and the death screen's stand-in
/// where the composition has none of its own.
#[expect(
    clippy::too_many_arguments,
    reason = "the kill cam's UI: the run, the settings, the composition's death screen, the knockout, and the card, accent, text, prompt, panel, tint and root nodes"
)]
fn hud(
    victim: Res<Victim>,
    settings: Option<Res<Settings>>,
    game_death_screen: Option<Res<GameDeathScreen>>,
    own: Query<&TankKnockedOut, With<Controlled>>,
    mut card: Query<(&mut Visibility, &mut UiTransform), (With<Card>, With<KillCamUi>)>,
    mut accent: Query<&mut BackgroundColor, With<CardAccent>>,
    mut card_text: Query<(&Index, &mut Text, &mut TextColor), (With<CardText>, Without<DeathText>)>,
    mut panel_text: Query<(&Index, &mut Text), (With<DeathText>, Without<CardText>)>,
    mut prompt: Query<(&ChildOf, &mut Text), (With<Prompt>, Without<CardText>, Without<DeathText>)>,
    mut panel: Query<&mut Visibility, (With<DeathPanel>, With<KillCamUi>, Without<Card>)>,
    mut tint: Query<
        &mut Visibility,
        (
            With<DeathTint>,
            With<KillCamUi>,
            Without<Card>,
            Without<DeathPanel>,
        ),
    >,
    mut roots: Query<
        &mut Visibility,
        (
            With<KillCamUi>,
            Without<Card>,
            Without<DeathPanel>,
            Without<DeathTint>,
        ),
    >,
) {
    let visible = |on: bool| {
        if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    let dead = victim.death.is_some();
    let death_screen = victim
        .death
        .as_ref()
        .is_some_and(|death| !death.holds_death_screen());
    let run = victim.run();
    let reduce = settings.as_deref().is_some_and(|s| s.reduce_motion.on());

    // The killer card.
    let record = victim.record.as_ref().filter(|_| dead);
    let enter = match (record, run) {
        (Some(_), Some(run)) if run.t >= run.beats.arrive() => timeline::ease_enter(
            timeline::progress(run.t, run.beats.arrive(), demo::CARD_ENTER_S),
        ),
        (Some(_), None) if death_screen => 1.0,
        _ => 0.0,
    };
    let card_on = enter > 0.0;
    if let Ok((mut v, mut transform)) = card.single_mut() {
        v.set_if_neq(visible(card_on));
        let scale = Vec2::splat(if reduce { 1.0 } else { 0.96 + 0.04 * enter });
        if transform.scale != scale {
            transform.scale = scale;
        }
    }
    if let Some(record) = record {
        if let Ok(mut background) = accent.single_mut() {
            let color =
                timeline::verdict_color(CornerKind::of(record.stored.report.impact.verdict));
            background.0 = color.with_alpha(enter);
        }
        let lines = card_lines(&record.killer, record.range_m);
        for (index, mut text, mut color) in &mut card_text {
            let line = match index.0 {
                0 => "KNOCKED OUT BY",
                row => lines[row - 1].as_str(),
            };
            if text.0 != line {
                text.0 = line.to_owned();
            }
            color.0 = color.0.with_alpha(enter);
        }
    }

    // The death screen's stand-in, where the composition has no death screen.
    let stand_in = death_screen && game_death_screen.is_none();
    let reason = own.single().ok().map(|knocked_out| knocked_out.reason);
    for (index, mut text) in &mut panel_text {
        let line = match index.0 {
            0 => "KNOCKED OUT",
            _ => reason.map_or("", |r| r.label()),
        };
        if text.0 != line {
            text.0 = line.to_owned();
        }
    }
    for mut v in panel.iter_mut().chain(tint.iter_mut()) {
        v.set_if_neq(visible(stand_in));
    }

    // The replay prompt.
    let replay = death_screen && run.is_none() && victim.record.is_some();
    for (parent, mut text) in &mut prompt {
        if text.0 != REPLAY {
            text.0 = REPLAY.to_owned();
        }
        if let Ok(mut v) = roots.get_mut(parent.parent()) {
            v.set_if_neq(visible(replay));
        }
    }
}

#[cfg(test)]
mod tests;
