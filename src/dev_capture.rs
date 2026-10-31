//! In-engine frames to disk, for documentation. Inert unless `OVERMATCH_CAPTURE` is set; mounted
//! AFTER `DefaultPlugins` by the offline root and the damage cam demo, and by the windowed network
//! client only when the variable is set (that client then creates its window hidden itself).
//!
//! * `OVERMATCH_CAPTURE=<png>` — the frame's path. With several times the time goes before the
//!   extension, zero-padded to a fixed width: `shot.png` → `shot-00004.0000s.png`,
//!   `shot-00008.5000s.png`. Byte order of the names is time order, so a glob assembles a sequence
//!   as is: `ffmpeg -framerate 30 -pattern_type glob -i 'shot-*s.png' out.mp4`. Times that round
//!   to the same name are one frame.
//! * `OVERMATCH_CAPTURE_AT=<s>[,<s>…]` — app seconds (`Time` as `Update` sees it) of each frame;
//!   default 6.
//! * `OVERMATCH_CAPTURE_SIGHT=1` — switch to the gunner optic two seconds before the first frame,
//!   by writing the resource the Shift toggle writes; ignored where that resource is absent.
//! * `OVERMATCH_CAPTURE_POSE=<x>,<y>,<z>,<lx>,<ly>,<lz>` — hold the player's camera at `x,y,z`
//!   looking at `lx,ly,lz` (world metres) for the whole run, as a captor
//!   ([`crate::camera::CameraCaptive`]): the player's own camera writers stand aside, and the lens
//!   stays the one the camera last had. Ignored when malformed (logged).
//! * `OVERMATCH_CAPTURE_SIZE=WxH` — the frame in physical pixels, default 2560x1440. The target's
//!   scale factor is [`SCALE_FACTOR`], so the UI lays out at half that in logical pixels, as on a
//!   retina window.
//! * `OVERMATCH_CAPTURE_UI=0` — hide every UI tree, every frame, after whatever the app's own
//!   systems wrote to its roots' `Visibility` that frame.
//! * `OVERMATCH_CAPTURE_GIZMOS=0` — disable the default gizmo group when the cameras are
//!   retargeted.
//!
//! * `OVERMATCH_CAPTURE_STEP_S=<s>` — a positive, fixed virtual frame step for reproducible
//!   comparisons. Asset loading still runs asynchronously, so capture after the scene is ready.
//! * `OVERMATCH_CAPTURE_AMBIGUITIES=1` — report Update and PostUpdate schedule ambiguities.
//!
//! Every capture run seeds the view RNG (cosmetic effect variation, otherwise seeded from the wall
//! clock) with [`VIEW_SEED`], so two runs draw the same effects.
//! * `OVERMATCH_CAPTURE_FOLLOW=<x>,<y>,<z>` — hold the camera at this world-space offset from
//!   the controlled tank, looking at its root, for close belt comparisons.
//! * `OVERMATCH_CAPTURE_SETTINGS=<tab>` — hold the settings page open on that tab (`video` or
//!   `gameplay`), its first row selected, on a root that mounts the page. The values it shows are
//!   the run's `video.ron`; point `OVERMATCH_CONFIG_DIR` at a scratch directory to stage them
//!   without touching the player's own.
//!
//! * `OVERMATCH_CAPTURE_FELL=<instance id>,<s>,<cut>,<dir bin>` — on the first fixed tick at or
//!   after app second `s`, the standing tree of that map instance falls broken at segment `cut`
//!   toward azimuth bin `dir bin`, exactly as a law sets it ([`offline_fell_plugin`]; the offline
//!   root alone mounts it).
//! * `OVERMATCH_CAPTURE_IMPACT=<x>,<z>,<calibre m>@<s>[;…]` — on the first frame at or after app
//!   second `s`, a round of that calibre strikes the terrain under world `x,z`: the view's impact
//!   read (dust, smoke, the ground mark) is raised there with no round flown and no shot recorded
//!   ([`offline_impact_plugin`]; the offline root alone mounts it).
//! * `OVERMATCH_CAPTURE_SHOT=<round key>,<fuze mode index>,tree|ground@<s>` — on the first frame
//!   at or after app second `s`, that shipped round, in that fuze mode, is fired at a real tree or
//!   at the ground beside it, and the frames are timed from its burst, the first two seconds after
//!   it ([`offline_shot_plugin`]; the offline root alone mounts it).
//! * `OVERMATCH_CAPTURE_WALL=<round key>[,<fuze mode index>]|x<kg>|dust:<names>[,<m>]@<s>[;…]`
//!   — on the first frame at or after app second `s`, that shipped round is fired square at a
//!   house wall, a bare charge of that mass is set off at its foot, or the named elements or
//!   components of the house are shot out, each shot of the list at its own second, and the camera
//!   stands in front of the wall unless `OVERMATCH_CAPTURE_POSE` holds it ([`offline_wall_plugin`]
//!   on the offline root; the dedicated server fires the same cue for its hidden capture clients,
//!   [`server_wall_plugin`]).
//!   `OVERMATCH_CAPTURE_WALL_SUN=1` picks the wall facing the sun and fires along its rays.
//!
//! A capture run draws the view's cosmetic randomness (sprite roll, scale jitter, flipbook start)
//! from one fixed seed, so two runs of one scenario on a fixed step draw the same sprites.
//!
//! The process exits once every frame is on disk, with a failure code if any is missing.
//!
//! # Invariant: a capture run shows no window
//!
//! The primary window is created hidden and, on macOS, the process renounces activation. A root
//! that already created its window hidden keeps both duties ([`run_offline`](crate::run_offline),
//! whose `SPIKE_SIM_WINDOWED=1` mode also keeps a persisted fullscreen from re-applying — pass it
//! there). A hidden window has no swapchain texture, and a window screenshot reads back nothing
//! without one (`bevy_render::view::window::screenshot::submit_screenshot_commands` skips the
//! copy), so the frame is instead read back from an offscreen image that every window-targeting
//! camera is pointed at one second before the first frame.
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
mod wall;
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

pub fn plugin(app: &mut App) {
    let Some(path) = std::env::var_os("OVERMATCH_CAPTURE") else {
        return;
    };
    if std::env::var("OVERMATCH_CAPTURE_AMBIGUITIES").as_deref() == Ok("1") {
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
    let at = parse_times(&std::env::var("OVERMATCH_CAPTURE_AT").unwrap_or_default());
    let sight = std::env::var("OVERMATCH_CAPTURE_SIGHT").as_deref() == Ok("1");
    let size = std::env::var("OVERMATCH_CAPTURE_SIZE")
        .ok()
        .and_then(|s| parse_size(&s))
        .unwrap_or(UVec2::new(2560, 1440));
    let ui = std::env::var("OVERMATCH_CAPTURE_UI").as_deref() != Ok("0");
    let gizmos = std::env::var("OVERMATCH_CAPTURE_GIZMOS").as_deref() != Ok("0");
    info!(
        "capture: {} at {at:?} s, sight {sight}, {}x{}, ui {ui}, gizmos {gizmos}",
        path.to_string_lossy(),
        size.x,
        size.y
    );
    hide_window(app);
    silence(app);
    // The effects plugin only `init_resource`s the RNG, so this seed holds in either mount order.
    app.insert_resource(crate::vfx::ViewRng::seeded(VIEW_SEED));
    if let Some(step) = std::env::var("OVERMATCH_CAPTURE_STEP_S")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
    {
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(step),
        ));
    }
    if let Some(offset) = std::env::var("OVERMATCH_CAPTURE_FOLLOW")
        .ok()
        .and_then(|s| {
            let values: Vec<f32> = s
                .split(',')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            let [x, y, z] = values.as_slice() else {
                return None;
            };
            Some(Vec3::new(*x, *y, *z))
        })
    {
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
    if let Ok(tab) = std::env::var("OVERMATCH_CAPTURE_SETTINGS") {
        match crate::settings::CapturePage::named(&tab) {
            Some(page) => {
                app.insert_resource(page);
            }
            None => panic!("OVERMATCH_CAPTURE_SETTINGS={tab}: no settings tab has that name"),
        }
    }
    app.insert_resource(Capture {
        path: path.into(),
        at,
        sight,
        size,
        gizmos,
        target: None,
        switched: false,
        requested: 0,
        saved: 0,
    });
    app.add_systems(Update, capture);
    if let Ok(raw) = std::env::var("OVERMATCH_CAPTURE_POSE") {
        match parse_pose(&raw) {
            Some(pose) => {
                info!(
                    "capture: camera held at {} looking at {}",
                    pose.eye, pose.look
                );
                app.insert_resource(pose).add_systems(
                    PostUpdate,
                    hold_pose.in_set(crate::camera::CaptiveCameraPlaced),
                );
            }
            None => warn!(
                "capture: OVERMATCH_CAPTURE_POSE={raw:?} is not x,y,z,lx,ly,lz with distinct \
                 finite points; the camera is the player's"
            ),
        }
    }
    if !ui {
        app.add_systems(
            PostUpdate,
            hide_ui.before(VisibilitySystems::VisibilityPropagate),
        );
    }
}

/// Zero the global volume for the run, after every plugin's build (module doc).
fn silence(app: &mut App) {
    app.add_systems(PreStartup, |mut commands: Commands| {
        commands.insert_resource(GlobalVolume::new(Volume::Linear(0.0)));
    });
}

/// `OVERMATCH_CAPTURE_FELL`'s cue: which tree falls, when, and how.
#[derive(Resource, Debug, Clone, PartialEq)]
struct FellCue {
    instance: String,
    at: f32,
    cut: u8,
    dir: u8,
    done: bool,
}

/// `<instance id>,<s>,<cut>,<dir bin>`; `None` when malformed.
fn parse_fell(s: &str) -> Option<FellCue> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let [instance, at, cut, dir] = parts.as_slice() else {
        return None;
    };
    let at = at.parse::<f32>().ok().filter(|at| at.is_finite())?;
    Some(FellCue {
        instance: (*instance).to_owned(),
        at,
        cut: cut.parse().ok()?,
        dir: dir.parse().ok()?,
        done: false,
    })
}

/// The capture's tree-fall lever, mounted by the offline root only: it writes the authority's
/// state, so a replica never runs it. Inert unless both `OVERMATCH_CAPTURE` and
/// `OVERMATCH_CAPTURE_FELL` are set.
pub(crate) fn offline_fell_plugin(app: &mut App) {
    if std::env::var_os("OVERMATCH_CAPTURE").is_none() {
        return;
    }
    let Ok(raw) = std::env::var("OVERMATCH_CAPTURE_FELL") else {
        return;
    };
    let Some(cue) = parse_fell(&raw) else {
        warn!("capture: OVERMATCH_CAPTURE_FELL={raw:?} is not <instance id>,<s>,<cut>,<dir bin>");
        return;
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

/// One `OVERMATCH_CAPTURE_IMPACT` strike.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ImpactCue {
    at: f32,
    x: f32,
    z: f32,
    caliber: f32,
}

/// The strikes not yet raised.
#[derive(Resource, Debug)]
struct ImpactCues(Vec<ImpactCue>);

/// `<x>,<z>,<calibre m>@<s>` entries separated by `;`, every number finite and the calibre
/// positive; `None` when any entry is malformed.
fn parse_impacts(s: &str) -> Option<Vec<ImpactCue>> {
    s.split(';')
        .map(|entry| {
            let (point, at) = entry.split_once('@')?;
            let numbers: Vec<f32> = point
                .split(',')
                .map(|n| n.trim().parse().ok())
                .collect::<Option<_>>()?;
            let [x, z, caliber] = numbers[..] else {
                return None;
            };
            let at: f32 = at.trim().parse().ok()?;
            ([x, z, caliber, at].iter().all(|n| n.is_finite()) && caliber > 0.0)
                .then_some(ImpactCue { at, x, z, caliber })
        })
        .collect()
}

/// The capture's terrain-impact lever, mounted by the offline root only, where the impact seam has
/// no replica reading it. Inert unless both `OVERMATCH_CAPTURE` and `OVERMATCH_CAPTURE_IMPACT` are
/// set.
pub(crate) fn offline_impact_plugin(app: &mut App) {
    if std::env::var_os("OVERMATCH_CAPTURE").is_none() {
        return;
    }
    let Ok(raw) = std::env::var("OVERMATCH_CAPTURE_IMPACT") else {
        return;
    };
    let Some(cues) = parse_impacts(&raw) else {
        warn!("capture: OVERMATCH_CAPTURE_IMPACT={raw:?} is not <x>,<z>,<calibre m>@<s>[;…]");
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
                    caliber: cue.caliber,
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

/// Where `OVERMATCH_CAPTURE_POSE` holds the player's camera.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
struct Pose {
    eye: Vec3,
    look: Vec3,
}

/// Six comma-separated finite numbers, eye then target, two distinct points.
fn parse_pose(s: &str) -> Option<Pose> {
    let numbers: Vec<f32> = s
        .split(',')
        .map(|n| n.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, z, lx, ly, lz] = numbers[..] else {
        return None;
    };
    let (eye, look) = (Vec3::new(x, y, z), Vec3::new(lx, ly, lz));
    (eye.is_finite() && look.is_finite() && eye != look).then_some(Pose { eye, look })
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

/// Comma-separated seconds; unparsable entries are dropped; empty means the default. Times that
/// share a [`stamp`] are kept once, so every frame has its own file.
fn parse_times(s: &str) -> Vec<f32> {
    let mut at: Vec<f32> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
    if at.is_empty() {
        at.push(6.0);
    }
    at.sort_by(f32::total_cmp);
    at.dedup_by(|a, b| stamp(*a) == stamp(*b));
    at
}

/// A frame time as its file names it: fixed width, so byte order is numeric order for times in
/// [0, 100000) s, at 0.1 ms resolution.
fn stamp(at: f32) -> String {
    format!("{at:010.4}")
}

fn parse_size(s: &str) -> Option<UVec2> {
    let (w, h) = s.split_once('x')?;
    Some(UVec2::new(w.parse().ok()?, h.parse().ok()?))
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

    /// The capture times default to 6 s, sort, drop junk, and fold times sharing a frame name into
    /// one frame, so two captures never race for one file.
    #[test]
    fn times_default_sort_and_drop_junk() {
        assert_eq!(parse_times(""), vec![6.0]);
        assert_eq!(parse_times("8, 2.5,x,4"), vec![2.5, 4.0, 8.0]);
        assert_eq!(parse_times("2.00001,2,3"), vec![2.0, 3.0]);
    }

    /// A pose is six finite numbers naming two distinct points; anything else leaves the camera
    /// to the player rather than pointing it at a guess.
    #[test]
    fn a_pose_is_two_distinct_finite_points() {
        assert_eq!(
            parse_pose("1, 2,3,4,5,6"),
            Some(Pose {
                eye: Vec3::new(1.0, 2.0, 3.0),
                look: Vec3::new(4.0, 5.0, 6.0),
            })
        );
        for refused in [
            "",
            "1,2,3,4,5",
            "1,2,3,4,5,6,7",
            "1,2,3,1,2,3",
            "1,2,x,4,5,6",
            "nan,0,0,1,1,1",
        ] {
            assert_eq!(parse_pose(refused), None, "{refused}");
        }
    }

    /// A fell cue is an instance id, a finite second and two bytes; anything else leaves every
    /// tree standing rather than felling a guess.
    #[test]
    fn a_fell_cue_is_an_instance_a_second_a_cut_and_a_bin() {
        assert_eq!(
            parse_fell("fir_c_0160, 10,3,64"),
            Some(FellCue {
                instance: "fir_c_0160".to_owned(),
                at: 10.0,
                cut: 3,
                dir: 64,
                done: false,
            })
        );
        for refused in [
            "",
            "fir,10,3",
            "fir,10,3,64,1",
            "fir,inf,3,64",
            "fir,10,256,64",
            "fir,10,3,-1",
        ] {
            assert_eq!(parse_fell(refused), None, "{refused}");
        }
    }

    /// An impact cue is a ground point, a positive calibre and a second; one bad entry refuses the
    /// whole list rather than raising some of the strikes asked for.
    #[test]
    fn impact_cues_are_points_calibres_and_seconds() {
        assert_eq!(
            parse_impacts("-400, -430,0.088@17.5;1,2,0.0079@18"),
            Some(vec![
                ImpactCue {
                    at: 17.5,
                    x: -400.0,
                    z: -430.0,
                    caliber: 0.088,
                },
                ImpactCue {
                    at: 18.0,
                    x: 1.0,
                    z: 2.0,
                    caliber: 0.0079,
                },
            ])
        );
        for refused in [
            "",
            "1,2,0.088",
            "1,2@3",
            "1,2,0@3",
            "1,2,0.088@inf",
            "1,2,0.088@3;x",
        ] {
            assert_eq!(parse_impacts(refused), None, "{refused}");
        }
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
        let spec: Vec<String> = (0..3750)
            .map(|k| format!("{:.4}", 1.0 + k as f32 / 30.0))
            .collect();
        let at = parse_times(&spec.join(","));
        assert_eq!(at.len(), 3750);
        let path = Path::new("/out/f.png");
        let names: Vec<PathBuf> = at.iter().map(|&t| frame_path(path, t, at.len())).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, names);
    }
}
