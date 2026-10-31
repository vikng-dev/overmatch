//! In-engine frames to disk, for documentation. A capture run has one input: the plan file that
//! `OVERMATCH_CAPTURE=<plan>.ron` names, one [`plan::CapturePlan`], whose struct docs are the
//! grammar's one home. `scripts/capture/example.ron` is a worked plan, checked by a test. Without
//! the variable every plugin here is inert.
//!
//! [`plugin`] takes the plan's `frames`. It is mounted after `crate::default_plugins` by every
//! windowed root: the offline route, both sandboxes, the pen cam demo, and the windowed network
//! client. A plan without `frames` runs its cues on a normal window and takes no frame (the scene
//! of a frame cost measurement). The cue plugins take the plan's other fields where an authority
//! can raise their events: [`offline_fell_plugin`], [`offline_impact_plugin`],
//! [`offline_shot_plugin`] and [`offline_wall_plugin`] on the offline root alone, and
//! [`server_wall_plugin`] on the dedicated server, which reads the same plan file so that it fires
//! the wall shots its capture clients frame.
//!
//! The target's scale factor is [`SCALE_FACTOR`], so the UI lays out at half the frame's size in
//! logical pixels, as on a retina window. Every capture run seeds the view RNG (cosmetic effect
//! variation, otherwise seeded from the wall clock) with [`VIEW_SEED`], so two runs of one scenario
//! on a fixed step draw the same sprites. The process exits once every frame is on disk, with a
//! failure code if any is missing.
//!
//! # Invariant: a capture run shows no window
//!
//! On every root, whenever the plan takes frames and with no other variable: the primary window is
//! created hidden and `Windowed`, settings never re-apply a persisted fullscreen to it
//! (`settings::CaptureWindowPinned`), and on macOS the process renounces activation unless the
//! root created the window hidden and renounced it itself. A focus loss never pauses the run
//! (`state::FocusPauseSuppressed`). A hidden window has no swapchain texture, and a window
//! screenshot reads back nothing without one
//! (`bevy_render::view::window::screenshot::submit_screenshot_commands` skips the copy), so the
//! frame is instead read back from an offscreen image that every window-targeting camera is
//! pointed at one second before the first frame.
//!
//! The screenshot names that target exactly — same handle AND same scale factor. `ScreenshotPlugin`
//! swaps the view's output attachment for its own texture by `NormalizedRenderTarget` equality,
//! which `ImageRenderTarget` defines over both fields; `Screenshot::image` normalizes to scale 1,
//! names a target no camera renders into, and reads back black.
//!
//! # Invariant: a capture run is silent
//!
//! The global volume is zero for the whole run, as under `OVERMATCH_MUTE=1` (`crate::sfx`), set or
//! not. It is written in `PreStartup`, after every plugin has built — `AudioPlugin` inserts its own
//! `GlobalVolume` at build — and before any audio sink exists (bevy reads the global volume when
//! it creates a sink), so no mount order of this plugin lets a capture make a sound.

mod fuze;
pub(crate) use fuze::offline_shot_plugin;
pub(crate) mod plan;
mod wall;
pub(crate) use plan::plan;
pub(crate) use wall::{offline_wall_plugin, server_wall_plugin};

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use bevy::audio::{GlobalVolume, Volume};
use bevy::camera::visibility::VisibilitySystems;
use bevy::camera::{Camera, ImageRenderTarget, RenderTarget};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::ui::IsDefaultUiCamera;
use bevy::window::PrimaryWindow;

use crate::sight::SightMode;

/// Logical-to-physical ratio of the frame; the UI lays out at `size / SCALE_FACTOR` logical pixels.
const SCALE_FACTOR: f32 = 2.0;
/// Seconds between the retarget and the first frame. The image's GPU texture, the camera's target
/// info and the UI's relayout each follow the retarget by a frame; one second covers them all.
const WARM_UP: f32 = 1.0;
/// Seconds before the first frame at which the sight switches.
const SIGHT_LEAD: f32 = 2.0;
/// Seconds after the last frame's time by which it must be on disk, or the run exits failed.
const GRACE: f32 = 3.0;
/// The view RNG's seed in a capture run: any fixed value.
const VIEW_SEED: u64 = 0x5EED;
/// Height a capture impact's ground probe starts from, world metres. ASSUMED above any terrain.
const IMPACT_PROBE_TOP_M: f32 = 1000.0;

#[derive(Resource)]
struct Capture {
    path: PathBuf,
    /// Ascending and non-empty.
    at: Vec<f32>,
    sight: bool,
    size: UVec2,
    gizmos: bool,
    /// The cameras' target once retargeted; each screenshot names it verbatim.
    target: Option<ImageRenderTarget>,
    switched: bool,
    /// Frames requested so far: an index into `at`.
    requested: usize,
    saved: usize,
}

/// The capture of the plan's `frames` (module doc); inert without a plan or without frames.
pub fn plugin(app: &mut App) {
    if let Some(frames) = plan().and_then(|plan| plan.frames.as_ref()) {
        mount(app, frames);
    }
}

fn mount(app: &mut App, frames: &plan::Frames) {
    if frames.ambiguities {
        use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings, ScheduleLabel};
        for label in [Update.intern(), PostUpdate.intern()] {
            app.edit_schedule(label, |schedule| {
                schedule.set_build_settings(ScheduleBuildSettings {
                    ambiguity_detection: LogLevel::Warn,
                    ..schedule.get_build_settings()
                });
            });
        }
    }
    let size = frames.size();
    info!(
        "capture: {} at {:?} s, sight {}, {}x{}, ui {}, gizmos {}",
        frames.out.display(),
        frames.at,
        frames.sight,
        size.x,
        size.y,
        frames.ui,
        frames.gizmos
    );
    if let Some(dir) = frames
        .out
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir).unwrap_or_else(|error| {
            panic!("{}: frames.out: {}: {error}", plan::VARIABLE, dir.display())
        });
    }
    unattended(app);
    // The effects plugin only `init_resource`s the RNG, so this seed holds in either mount order.
    app.insert_resource(crate::vfx::ViewRng::seeded(VIEW_SEED));
    if let Some(step) = frames.step_s {
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(step),
        ));
    }
    match frames.camera {
        plan::Camera::Player => {}
        plan::Camera::Pose { eye, look } => {
            let pose = Pose {
                eye: eye.into(),
                look: look.into(),
            };
            info!(
                "capture: camera held at {} looking at {}",
                pose.eye, pose.look
            );
            // A root that mounts no orbit camera configures no captor set; the pose still lands
            // before the frame's transforms propagate.
            app.configure_sets(
                PostUpdate,
                crate::camera::CaptiveCameraPlaced.before(TransformSystems::Propagate),
            )
            .insert_resource(pose)
            .add_systems(
                PostUpdate,
                hold_pose.in_set(crate::camera::CaptiveCameraPlaced),
            );
        }
        plan::Camera::Follow { offset } => {
            let offset = Vec3::from(offset);
            app.add_systems(
                PostUpdate,
                (move |tanks: Query<&Transform, With<crate::tank::Controlled>>,
                       mut cameras: Query<
                    &mut Transform,
                    (
                        With<crate::view::PlayerView>,
                        Without<crate::tank::Controlled>,
                    ),
                >| {
                    if let Ok(tank) = tanks.single() {
                        for mut camera in &mut cameras {
                            *camera = Transform::from_translation(tank.translation + offset)
                                .looking_at(tank.translation, Vec3::Y);
                        }
                    }
                })
                .after(crate::camera::OrbitCameraSet)
                .after(crate::camera::CaptiveCameraPlaced)
                .before(TransformSystems::Propagate),
            );
        }
    }
    if let Some(tab) = &frames.settings {
        app.insert_resource(
            crate::settings::CapturePage::named(tab).expect("the plan's tab is validated"),
        );
    }
    app.insert_resource(Capture {
        path: frames.out.clone(),
        at: frames.at.clone(),
        sight: frames.sight,
        size,
        gizmos: frames.gizmos,
        target: None,
        switched: false,
        requested: 0,
        saved: 0,
    });
    app.add_systems(Update, capture);
    if !frames.ui {
        app.add_systems(
            PostUpdate,
            hide_ui.before(VisibilitySystems::VisibilityPropagate),
        );
    }
}

/// The shipped round `key` in fuze mode `mode`, as the plan's `field` names it.
///
/// # Panics
///
/// When no shipped round has that key, or it has no such fuze mode (ADR-0011).
fn resolve_round(key: &str, mode: u8, field: &str) -> crate::rounds::Round {
    let round = crate::tank::WeaponRound::resolve(key)
        .unwrap_or_else(|error| panic!("{}: {field}: {error}", plan::VARIABLE));
    round.in_mode(mode).unwrap_or_else(|| {
        panic!(
            "{}: {field}: round {key:?} has no fuze mode {mode}",
            plan::VARIABLE
        )
    })
}

/// What a capture run implies on every root, with no variable of its own (module doc): the window
/// hidden and pinned, the run silent, and a focus loss never pausing it.
fn unattended(app: &mut App) {
    hide_window(app);
    pin_window(app);
    silence(app);
    app.insert_resource(crate::state::FocusPauseSuppressed);
}

/// Keep the primary window a plain window for the whole run: `Windowed` at creation, so a persisted
/// fullscreen never boots, and [`crate::settings::CaptureWindowPinned`], so settings never
/// re-applies one later. On macOS either would order the window front whatever `visible` says.
fn pin_window(app: &mut App) {
    let world = app.world_mut();
    let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
    if let Ok(mut window) = windows.single_mut(world) {
        window.mode = bevy::window::WindowMode::Windowed;
    }
    app.insert_resource(crate::settings::CaptureWindowPinned);
}

/// Zero the global volume for the run, after every plugin's build (module doc).
fn silence(app: &mut App) {
    app.add_systems(PreStartup, |mut commands: Commands| {
        commands.insert_resource(GlobalVolume::new(Volume::Linear(0.0)));
    });
}

/// The plan's `fell` cue ([`plan::Fell`]) and whether it has fired.
#[derive(Resource, Debug, Clone, PartialEq)]
struct FellCue {
    instance: String,
    at: f32,
    cut: u8,
    dir: u8,
    done: bool,
}

/// The capture's tree-fall cue, mounted by the offline root only: it writes the authority's
/// state, so a replica never runs it. Inert unless the plan has a `fell`.
pub(crate) fn offline_fell_plugin(app: &mut App) {
    let Some(fell) = plan().and_then(|plan| plan.fell.as_ref()) else {
        return;
    };
    let cue = FellCue {
        instance: fell.instance.clone(),
        at: fell.at,
        cut: fell.cut,
        dir: fell.dir,
        done: false,
    };
    info!("capture: {cue:?}");
    app.insert_resource(cue).add_systems(
        FixedPostUpdate,
        fell_on_cue
            .in_set(crate::trees::TreeFallSet)
            .run_if(not(resource_exists::<crate::ClientReplica>)),
    );
}

/// Fell the cued tree on the first fixed tick at or after its second, as the laws do: the state
/// and the transition's tick, in the fall set, so the body derives the same tick.
fn fell_on_cue(
    time: Res<Time>,
    clock: Option<Res<crate::ShotClock>>,
    mut cue: ResMut<FellCue>,
    mut trees: Query<(
        Entity,
        &crate::scatter::MapInstance,
        &crate::trees::TreeBody,
        &mut crate::trees::TreeState,
    )>,
    mut commands: Commands,
) {
    if cue.done || time.elapsed_secs() < cue.at {
        return;
    }
    cue.done = true;
    let Some((root, _, body, mut state)) = trees
        .iter_mut()
        .find(|(_, instance, _, _)| instance.id == cue.instance)
    else {
        warn!("capture: no tree is map instance {:?}", cue.instance);
        return;
    };
    if usize::from(cue.cut) >= body.kind.segments.len()
        || *state != crate::trees::TreeState::Standing
    {
        warn!("capture: {cue:?} cannot fall: the tree is {:?}", *state);
        return;
    }
    *state = crate::trees::TreeState::Down {
        cut: cue.cut,
        dir: cue.dir,
    };
    commands.entity(root).insert(crate::trees::Fell {
        tick: clock.map_or(0, |clock| clock.0),
    });
    info!(
        "capture: {} falls at {:.4} s, cut {}, bin {}",
        cue.instance,
        time.elapsed_secs(),
        cue.cut,
        cue.dir
    );
}

/// The plan's `impacts` ([`plan::Impact`]) not yet raised.
#[derive(Resource, Debug)]
struct ImpactCues(Vec<plan::Impact>);

/// The capture's terrain-impact cue, mounted by the offline root only, where the impact seam has
/// no replica reading it. Inert unless the plan lists `impacts`.
pub(crate) fn offline_impact_plugin(app: &mut App) {
    let Some(cues) = plan()
        .map(|plan| plan.impacts.clone())
        .filter(|cues| !cues.is_empty())
    else {
        return;
    };
    info!("capture: impacts {cues:?}");
    app.insert_resource(ImpactCues(cues))
        .add_systems(Update, raise_impacts);
}

/// Raise each due strike on the ground under its point, with the ground's normal there.
fn raise_impacts(
    time: Res<Time>,
    mut cues: ResMut<ImpactCues>,
    spatial: avian3d::prelude::SpatialQuery,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    let filter = avian3d::prelude::SpatialQueryFilter::from_mask(crate::Layer::Terrain);
    cues.0.retain(|cue| {
        if cue.at > now {
            return true;
        }
        let origin = Vec3::new(cue.x, IMPACT_PROBE_TOP_M, cue.z);
        match spatial.cast_ray(origin, Dir3::NEG_Y, 2.0 * IMPACT_PROBE_TOP_M, true, &filter) {
            Some(hit) => {
                let position = origin - Vec3::Y * hit.distance;
                info!("capture: impact at {position} ({now:.4} s)");
                commands.trigger(crate::ballistics::Impact {
                    position,
                    normal: hit.normal,
                    caliber: cue.calibre_m,
                    surface: crate::ballistics::ImpactSurface::Terrain,
                    outcome: crate::ballistics::ImpactOutcome::Struck,
                });
            }
            None => warn!("capture: no terrain under {cue:?}"),
        }
        false
    });
}

/// Apps rewrite their UI roots' `Visibility` from `Update`; this runs after them and before the
/// value propagates, so the roots stay hidden whatever they wrote.
fn hide_ui(mut roots: Query<&mut Visibility, (With<Node>, Without<ChildOf>)>) {
    for mut visibility in &mut roots {
        visibility.set_if_neq(Visibility::Hidden);
    }
}

/// Where the plan's `Pose` camera ([`plan::Camera`]) holds the player's camera.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
struct Pose {
    eye: Vec3,
    look: Vec3,
}

/// Hold the camera as a captor: claim the camera every frame, and write its pose.
fn hold_pose(
    pose: Res<Pose>,
    captive: Option<ResMut<crate::camera::CameraCaptive>>,
    mut cameras: Query<&mut Transform, With<crate::view::PlayerView>>,
) {
    if let Some(mut captive) = captive {
        captive.set_if_neq(crate::camera::CameraCaptive(true));
    }
    for mut transform in &mut cameras {
        *transform = Transform::from_translation(pose.eye).looking_at(pose.look, Vec3::Y);
    }
}

/// Frame times in ascending order, times that share a [`stamp`] kept once, so every frame has its
/// own file.
fn normalize_times(mut at: Vec<f32>) -> Vec<f32> {
    at.sort_by(f32::total_cmp);
    at.dedup_by(|a, b| stamp(*a) == stamp(*b));
    at
}

/// A frame time as its file names it: fixed width, so byte order is numeric order for times in
/// [0, 100000) s, at 0.1 ms resolution.
fn stamp(at: f32) -> String {
    format!("{at:010.4}")
}

/// One frame keeps `path`; several put the time before the extension.
fn frame_path(path: &Path, at: f32, frames: usize) -> PathBuf {
    if frames == 1 {
        return path.to_owned();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
    path.with_file_name(format!("{stem}-{}s.{ext}", stamp(at)))
}

/// Create the primary window hidden. `WindowPlugin` has spawned the entity by now and `bevy_winit`
/// reads `visible` when it creates the OS window, which is after every plugin's build.
pub(crate) fn hide_window(app: &mut App) {
    let world = app.world_mut();
    let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
    let Ok(mut window) = windows.single_mut(world) else {
        warn!("capture: no primary window at mount; one created later will be visible");
        return;
    };
    if !window.visible {
        return;
    }
    window.visible = false;
    #[cfg(target_os = "macos")]
    app.add_systems(Startup, crate::platform::revoke_macos_activation);
}

#[expect(
    clippy::too_many_arguments,
    reason = "a dev capture spanning time, its state, images, cameras, gizmo config, sight mode and exit"
)]
fn capture(
    shot_ready: Option<Res<fuze::ShotReady>>,
    mut commands: Commands,
    time: Res<Time>,
    mut cap: ResMut<Capture>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(Entity, &Camera, &mut RenderTarget, Has<IsDefaultUiCamera>)>,
    gizmo_store: Option<ResMut<GizmoConfigStore>>,
    mode: Option<ResMut<SightMode>>,
    mut exit: MessageWriter<AppExit>,
) {
    // Under a shot cue the frames are timed from the burst, which no app second predicts: the first
    // is taken `WARM_UP` + 1 s after it, the target retargeted `WARM_UP` before that.
    let t = if let Some(ready) = shot_ready {
        let Some(at) = ready.0 else {
            return;
        };
        time.elapsed_secs() - at + cap.at[0] - WARM_UP - 1.0
    } else {
        time.elapsed_secs()
    };

    let first = cap.at[0];
    if cap.sight && !cap.switched && t >= first - SIGHT_LEAD {
        if let Some(mut mode) = mode {
            *mode = SightMode::Gunner;
        }
        cap.switched = true;
    }
    if cap.target.is_none() && t >= first - WARM_UP {
        cap.target = Some(retarget(&mut commands, &mut images, &mut cameras, cap.size));
        if !cap.gizmos
            && let Some(mut store) = gizmo_store
        {
            store.config_mut::<DefaultGizmoConfigGroup>().0.enabled = false;
        }
    }
    let Some(target) = cap.target.clone() else {
        return;
    };
    if let Some(&due) = cap.at.get(cap.requested)
        && t >= due
    {
        commands
            .spawn(Screenshot(RenderTarget::Image(target)))
            .observe(save_to_disk(frame_path(&cap.path, due, cap.at.len())))
            .observe(|_: On<ScreenshotCaptured>, mut cap: ResMut<Capture>| cap.saved += 1);
        cap.requested += 1;
    }
    let last = cap.at[cap.at.len() - 1];
    if cap.saved == cap.at.len() {
        exit.write(AppExit::Success);
    } else if t >= last + GRACE {
        error!("capture: {} of {} frames saved", cap.saved, cap.at.len());
        exit.write(AppExit::error());
    }
}

/// Point every window-targeting camera at a fresh `size` image and make the highest-order one the
/// UI's default camera — the one `bevy_ui` picks for the window on its own, but only among window
/// targets. A camera already marked keeps the role.
fn retarget(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    cameras: &mut Query<(Entity, &Camera, &mut RenderTarget, Has<IsDefaultUiCamera>)>,
    size: UVec2,
) -> ImageRenderTarget {
    let target = offscreen_target(images, size, SCALE_FACTOR);
    let moved = retarget_to(commands, cameras, &target);
    info!(
        "capture: {moved} camera(s) retargeted to a {}x{} image",
        size.x, size.y
    );
    target
}

/// A render-target image of `size` physical pixels whose UI lays out at `size / scale_factor`.
pub(crate) fn offscreen_target(
    images: &mut Assets<Image>,
    size: UVec2,
    scale_factor: f32,
) -> ImageRenderTarget {
    let image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    ImageRenderTarget {
        handle: images.add(image),
        scale_factor,
    }
}

/// [`retarget`] onto an existing `target`; returns how many cameras moved. Cameras already off the
/// window are left alone, so calling it every frame moves each camera once, when it first appears.
pub(crate) fn retarget_to(
    commands: &mut Commands,
    cameras: &mut Query<(Entity, &Camera, &mut RenderTarget, Has<IsDefaultUiCamera>)>,
    target: &ImageRenderTarget,
) -> usize {
    let mut moved = 0;
    let mut ui_camera: Option<(isize, Entity)> = None;
    let mut marked = false;
    for (entity, camera, mut render_target, is_ui) in cameras.iter_mut() {
        marked |= is_ui;
        if !matches!(*render_target, RenderTarget::Window(_)) {
            continue;
        }
        *render_target = RenderTarget::Image(target.clone());
        moved += 1;
        if ui_camera.is_none_or(|(order, _)| camera.order > order) {
            ui_camera = Some((camera.order, entity));
        }
    }
    if let Some((_, entity)) = ui_camera
        && !marked
    {
        commands.entity(entity).insert(IsDefaultUiCamera);
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CAPTURE RUN TAKES NO SCREEN AND NEVER PAUSES, on any root, with no variable of its own:
    /// capture drivers run on a machine its owner is working at. The window a root described
    /// visible and fullscreen is hidden and `Windowed` before winit creates it, settings are told
    /// to leave it so, the run is silent, and a focus loss does not pause it. Mutants: dropping any
    /// one of [`hide_window`], [`pin_window`], [`silence`] or the focus marker fails one assert.
    #[test]
    fn a_capture_run_is_hidden_pinned_silent_and_never_paused() {
        let mut app = App::new();
        let window = app
            .world_mut()
            .spawn((
                Window {
                    mode: bevy::window::WindowMode::BorderlessFullscreen(
                        bevy::window::MonitorSelection::Primary,
                    ),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        unattended(&mut app);
        // What a later `AudioPlugin` build writes. Only `PreStartup` runs: `Startup` holds the
        // macOS activation revocation, which needs the process's main thread.
        app.insert_resource(GlobalVolume::new(Volume::Linear(1.0)));
        app.world_mut().run_schedule(PreStartup);
        let window = app.world().get::<Window>(window).expect("the window");
        assert!(!window.visible);
        assert_eq!(window.mode, bevy::window::WindowMode::Windowed);
        assert!(
            app.world()
                .contains_resource::<crate::settings::CaptureWindowPinned>()
        );
        assert!(
            app.world()
                .contains_resource::<crate::state::FocusPauseSuppressed>()
        );
        assert_eq!(
            app.world().resource::<GlobalVolume>().volume.to_linear(),
            0.0
        );
    }

    /// A CAPTURE RUN IS SILENT WHATEVER MOUNTS AFTER IT. Capture runs are unattended, on a machine
    /// its owner is working at; `AudioPlugin` inserts its own `GlobalVolume` when it builds, so a
    /// silence written at build would be undone by an audio plugin mounted later.
    #[test]
    fn a_capture_run_is_silent_whatever_mounts_after_it() {
        let mut app = App::new();
        silence(&mut app);
        // What a later `AudioPlugin` build writes.
        app.insert_resource(GlobalVolume::new(Volume::Linear(1.0)));
        app.update();
        assert_eq!(
            app.world().resource::<GlobalVolume>().volume.to_linear(),
            0.0
        );
    }

    /// The capture times sort and fold times sharing a frame name into one frame, so two captures
    /// never race for one file.
    #[test]
    fn times_sort_and_fold_onto_their_frame_names() {
        assert_eq!(normalize_times(vec![8.0, 2.5, 4.0]), vec![2.5, 4.0, 8.0]);
        assert_eq!(normalize_times(vec![2.00001, 2.0, 3.0]), vec![2.0, 3.0]);
    }

    /// One frame keeps the given path; several stamp it. A sequence assembled by name plays in
    /// time order: across whole seconds, across digit counts and between a short decimal and a
    /// long one (`1.1` before `1.1333`).
    #[test]
    fn frame_names_sort_in_time_order() {
        let single = Path::new("/out/shot.png");
        assert_eq!(frame_path(single, 8.0, 1), single);
        assert_eq!(
            frame_path(single, 8.5, 2),
            Path::new("/out/shot-00008.5000s.png")
        );
        let spec: Vec<f32> = (0..3750)
            .map(|k| {
                format!("{:.4}", 1.0 + k as f32 / 30.0)
                    .parse()
                    .expect("a number")
            })
            .collect();
        let at = normalize_times(spec);
        assert_eq!(at.len(), 3750);
        let path = Path::new("/out/f.png");
        let names: Vec<PathBuf> = at.iter().map(|&t| frame_path(path, t, at.len())).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, names);
    }
}
