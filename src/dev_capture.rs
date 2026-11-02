//! In-engine frames to disk, for documentation. The plan file that
//! `OVERMATCH_CAPTURE=<plan>.ron` names holds one [`plan::CapturePlan`], whose struct docs are the
//! grammar's one home. `scripts/capture/example.ron` is a worked plan, checked by a test. Without
//! the variable every plugin here is inert. The operator's hand-driving lever `OVERMATCH_START`
//! and scenario levers with tracked performance-script users
//! (`OVERMATCH_TARGET`, `OVERMATCH_PROBE_FAR`, `OVERMATCH_PROBE_TANKS`,
//! `OVERMATCH_BATTLE`, `OVERMATCH_XRAY`) also select the scene; a capture inherits their values.
//!
//! [`plugin`] takes the plan's `frames` and `keys`. It is mounted after `crate::windowed_runtime`
//! by every windowed root: the offline route, both sandboxes, and the windowed network client. A
//! plan without `frames` runs its cues on a normal window and takes no frame (the scene of a frame
//! cost measurement). The cue plugins take the plan's other fields where an authority
//! can raise their events: [`offline_fell_plugin`], [`offline_impact_plugin`],
//! [`offline_shot_plugin`] and [`offline_wall_plugin`] on the offline root alone, and
//! [`server_wall_plugin`] on the dedicated server, which reads the same plan file so that it fires
//! the wall shots its capture clients frame.
//!
//! The plan's `frames.scale_factor` sets the target's UI ratio (ASSUMED default 2, as on a
//! retina window), so HUD evidence can also request an ordinary scale-1 window. Every capture run seeds the view RNG (cosmetic effect
//! variation, otherwise seeded from the wall clock) with [`VIEW_SEED`], so two runs of one scenario
//! on a fixed step draw the same sprites. The process exits once every frame is on disk, with a
//! failure code if any is missing.
//!
//! # Invariant: a capture run shows no window
//!
//! On every root, whenever the plan takes frames and with no other variable, the run is a hidden
//! one (`run_window::hide`): the primary window is created hidden and `Windowed`, settings never
//! re-apply a persisted fullscreen to it, on macOS the process renounces activation, and the run is
//! unattended (silent, never paused by a focus loss). A hidden window has no swapchain texture,
//! and a window screenshot reads back nothing without one
//! (`bevy_render::view::window::screenshot::submit_screenshot_commands` skips the copy), so the
//! frame is instead read back from an offscreen image that every window-targeting camera is
//! pointed at one second before the first frame.
//!
//! The screenshot names that target exactly — same handle AND same scale factor. `ScreenshotPlugin`
//! swaps the view's output attachment for its own texture by `NormalizedRenderTarget` equality,
//! which `ImageRenderTarget` defines over both fields; `Screenshot::image` normalizes to scale 1,
//! names a target no camera renders into, and reads back black.

mod fuze;
pub(crate) use fuze::offline_shot_plugin;
pub(crate) mod plan;
mod repair;
pub(crate) use repair::offline_repair_plugin;
mod wall;
pub(crate) use plan::plan;
pub(crate) use wall::{offline_wall_plugin, server_wall_plugin};

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use bevy::camera::visibility::VisibilitySystems;
use bevy::camera::{Camera, ImageRenderTarget, RenderTarget};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::ui::IsDefaultUiCamera;

use crate::sight::SightMode;

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
    scale_factor: f32,
    gizmos: bool,
    /// The cameras' target once retargeted; each screenshot names it verbatim.
    target: Option<ImageRenderTarget>,
    switched: bool,
    /// Frames requested so far: an index into `at`.
    requested: usize,
    saved: usize,
}

/// The capture of the plan's `frames` (module doc) and its `keys`; each is inert where the plan
/// lacks it.
pub fn plugin(app: &mut App) {
    if let Some(frames) = plan().and_then(|plan| plan.frames.as_ref()) {
        mount(app, frames);
    }
    if let Some(holds) = plan()
        .map(|plan| plan.keys.clone())
        .filter(|holds| !holds.is_empty())
    {
        info!("capture: keys {holds:?}");
        app.insert_resource(KeyHolds {
            holds,
            held: Vec::new(),
        })
        .add_systems(PreUpdate, hold_keys.after(bevy::input::InputSystems));
    }
}

/// The plan's key holds ([`plan::KeyHold`]), and the keys they hold down this frame.
#[derive(Resource, Debug)]
struct KeyHolds {
    holds: Vec<plan::KeyHold>,
    held: Vec<KeyCode>,
}

/// Press every key whose window holds the frame's second and release each one whose last window
/// has passed. After the input systems, which clear the edges and apply the devices' events, and
/// before `Update`'s readers.
fn hold_keys(time: Res<Time>, mut holds: ResMut<KeyHolds>, mut keys: ResMut<ButtonInput<KeyCode>>) {
    let now = time.elapsed_secs();
    let active: Vec<KeyCode> = holds
        .holds
        .iter()
        .filter(|hold| hold.from <= now && now < hold.to)
        .map(|hold| hold.key)
        .collect();
    for key in holds.held.iter().filter(|key| !active.contains(key)) {
        keys.release(*key);
    }
    for key in &active {
        keys.press(*key);
    }
    holds.held = active;
}

fn mount(app: &mut App, frames: &plan::Frames) {
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
    crate::run_window::hide(app);
    // The effects plugin only `init_resource`s the RNG, so this seed holds in either mount order.
    app.insert_resource(crate::vfx::ViewRng::seeded(VIEW_SEED));
    if let Some(step) = frames.step_s {
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(step.0));
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
                       cap: Res<Capture>,
                       mut cameras: Query<
                    &mut Transform,
                    (
                        With<crate::view::PlayerView>,
                        Without<crate::tank::Controlled>,
                    ),
                >| {
                    let Some(tank) = crate::tank::controlled_one(tanks.single()) else {
                        // No control owner while the scene loads, before the frame target arms.
                        assert!(cap.target.is_none(), "capture: frames.camera Follow needs a controlled tank before the first frame; choose Pose for a tankless scene");
                        return;
                    };
                    for mut camera in &mut cameras {
                        *camera = Transform::from_translation(tank.translation + offset)
                            .looking_at(tank.translation, Vec3::Y);
                    }
                })
                .after(crate::camera::OrbitCameraSet)
                .after(crate::camera::CaptiveCameraPlaced)
                .before(TransformSystems::Propagate),
            );
        }
    }
    app.insert_resource(Capture {
        path: frames.out.clone(),
        at: frames.at.clone(),
        sight: frames.sight,
        size,
        scale_factor: frames.scale_factor,
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
    app.insert_resource(cue)
        .add_systems(PostStartup, check_fell_target)
        .add_systems(
            FixedPostUpdate,
            fell_on_cue
                .in_set(crate::trees::TreeFallSet)
                .run_if(not(resource_exists::<crate::ClientReplica>)),
        );
}

/// The cued tree, resolved at boot: the map's trees spawn at `Startup`, from data, so the plan's
/// `fell.instance` and `fell.cut` are checked once they stand and a plan naming a tree the map
/// lacks, or a cut its trunk has not, fails the run before any frame.
fn check_fell_target(
    cue: Res<FellCue>,
    trees: Query<(&crate::scatter::MapInstance, &crate::trees::TreeBody)>,
) {
    let (_, body) = trees
        .iter()
        .find(|(instance, _)| instance.id == cue.instance)
        .unwrap_or_else(|| {
            panic!(
                "{}: fell.instance: the map has no tree {:?}",
                plan::VARIABLE,
                cue.instance
            )
        });
    let segments = body.kind.segments.len();
    assert!(
        usize::from(cue.cut) < segments,
        "{}: fell.cut: {} is past the {segments} segments of {:?}",
        plan::VARIABLE,
        cue.cut,
        cue.instance
    );
}

/// The environment capture cue frees its chosen section with an explicit fixture blow.
/// ASSUMED operator fixture: horizontal momentum for a free translation of 1 m/s, applied
/// at the freed group's centre. The runtime law never invents this launch momentum.
///
/// # Panics
///
/// When the tree is already down at its second: the cue cannot fall it, and a run that went on
/// would frame a fall that never happened.
fn fell_on_cue(
    time: Res<Time>,
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
    let (root, _, body, mut state) = trees
        .iter_mut()
        .find(|(_, instance, _, _)| instance.id == cue.instance)
        .expect("the cued tree was found at boot (`check_fell_target`), and trees never despawn");
    assert!(
        *state == crate::trees::TreeState::Standing,
        "{}: fell: {:?} is {:?} at its cue, so it cannot fall",
        plan::VARIABLE,
        cue.instance,
        *state
    );
    *state = crate::trees::TreeState::Down {
        cut: cue.cut,
        dir: cue.dir,
    };
    let (dx, dz) = crate::trees::azimuth(cue.dir);
    let mass: f32 = body.structure.chunks[usize::from(cue.cut)..]
        .iter()
        .map(|chunk| chunk.mass)
        .sum();
    commands.entity(root).insert(crate::trees::TrunkLoads(vec![
        crate::buildings::fracture::Load {
            chunk: usize::from(cue.cut),
            at: body.piece_centroid(cue.cut),
            blow: Vec3::new(dx, 0.0, dz) * mass,
            press: Vec3::ZERO,
            energy: 0.0,
            crushed: 0.0,
            peak: None,
            origin: crate::buildings::fracture::Origin::Field,
        },
    ]));
    info!(
        "capture: {} falls at {:.4} s, cut {}, bin {}",
        cue.instance,
        time.elapsed_secs(),
        cue.cut,
        cue.dir
    );
}

/// A plan's strike with its round's style and its surface resolved.
#[derive(Clone, Copy, Debug)]
struct ImpactCue {
    x: f32,
    z: f32,
    at: f32,
    style: crate::rounds::ImpactStyle,
    surface: crate::ballistics::ImpactSurface,
}

/// The plan's `impacts` ([`plan::Impact`]) not yet raised.
#[derive(Resource, Debug)]
struct ImpactCues(Vec<ImpactCue>);

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
    let cues = cues
        .iter()
        .map(|cue| {
            let (style, surface) = cue
                .resolved()
                .unwrap_or_else(|why| panic!("{}: {why}", plan::VARIABLE));
            ImpactCue {
                x: cue.x,
                z: cue.z,
                at: cue.at,
                style,
                surface,
            }
        })
        .collect();
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
                    style: cue.style,
                    surface: cue.surface,
                    outcome: crate::ballistics::ImpactOutcome::Struck,
                });
            }
            None => panic!("capture: impacts: no terrain under {cue:?}; place the cue inside the terrain collider"),
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
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("CapturePlan validates frames.out has a UTF-8 file stem");
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .expect("CapturePlan validates frames.out has a PNG extension");
    path.with_file_name(format!("{stem}-{}s.{ext}", stamp(at)))
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
        cap.target = Some(retarget(
            &mut commands,
            &mut images,
            &mut cameras,
            cap.size,
            cap.scale_factor,
        ));
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
            .observe(save_frame(frame_path(&cap.path, due, cap.at.len())));
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

/// Count a frame only after its PNG reaches disk. The engine's save observer logs I/O failures
/// without reporting them to the capture, so the run owns this visible failure outcome.
fn save_frame(
    path: PathBuf,
) -> impl FnMut(On<ScreenshotCaptured>, ResMut<Capture>, MessageWriter<AppExit>) {
    move |frame, mut cap, mut exit| {
        let image = frame.image.clone().try_into_dynamic().expect(
            "capture targets are initialized Rgba8UnormSrgb images, supported by image conversion",
        );
        match image
            .to_rgb8()
            .save_with_format(&path, image::ImageFormat::Png)
        {
            Ok(()) => cap.saved += 1,
            Err(error) => {
                error!("capture: frames.out {}: {error}", path.display());
                exit.write(AppExit::error());
            }
        }
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
    scale_factor: f32,
) -> ImageRenderTarget {
    let target = offscreen_target(images, size, scale_factor);
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

    /// A raised strike reaches the seam as the plan resolved it: on the ground its probe found,
    /// carrying the cue's own round and surface. The impact stills judged against ADR-0023 rely on
    /// it, since no round flies. Mutant: `surface: ImpactSurface::Terrain` hard-coded in
    /// [`raise_impacts`] shows every substance's still as soil and fails here.
    #[test]
    fn a_raised_strike_emits_its_resolved_read_on_the_probed_ground() {
        use crate::ballistics::Impact;
        use avian3d::prelude::{Collider, CollisionLayers, LayerMask, PhysicsPlugins, RigidBody};
        use bevy::time::TimeUpdateStrategy;

        #[derive(Resource, Default)]
        struct Raised(Vec<(Vec3, Vec3, crate::rounds::ImpactStyle, ImpactSurface)>);
        use crate::ballistics::ImpactSurface;

        let cue = plan::Impact {
            x: 3.0,
            z: -4.0,
            round: "88mm_AP".into(),
            surface: plan::StruckSurface::Substance("Wood".into()),
            at: 0.0,
        };
        let (style, surface) = cue.resolved().expect("a shipped round and substance");
        assert!(matches!(surface, ImpactSurface::Material(_)));

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        // DERIVED: 1 s / 64, one physics step per update.
        .insert_resource(TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_micros(15_625),
        ))
        .init_resource::<Raised>()
        .add_observer(|impact: On<Impact>, mut raised: ResMut<Raised>| {
            raised
                .0
                .push((impact.position, impact.normal, impact.style, impact.surface));
        });
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        // FIXTURE: a slab whose top is the plane y = 2 m.
        app.world_mut().spawn((
            RigidBody::Static,
            Transform::from_xyz(0.0, 1.0, 0.0),
            Collider::cuboid(100.0, 2.0, 100.0),
            CollisionLayers::new(crate::Layer::Terrain, LayerMask::ALL),
        ));
        app.update();
        app.update();
        app.insert_resource(ImpactCues(vec![ImpactCue {
            x: cue.x,
            z: cue.z,
            at: cue.at,
            style,
            surface,
        }]))
        .add_systems(Update, raise_impacts);
        app.update();
        let raised = &app.world().resource::<Raised>().0;
        assert_eq!(
            raised.as_slice(),
            [(Vec3::new(3.0, 2.0, -4.0), Vec3::Y, style, surface)]
        );
        assert!(app.world().resource::<ImpactCues>().0.is_empty());
    }

    /// A FELL CUE NAMING A TREE THE MAP LACKS FAILS THE RUN AT BOOT: the cue used to log and let the
    /// run exit `Success` with frames of a fall that never happened. A map with no tree at all is
    /// the class case (the check reads the spawned trees and nothing else). Mutant: a check that
    /// skips a missing tree returns here.
    #[test]
    #[should_panic(expected = "fell.instance: the map has no tree")]
    fn a_fell_cue_naming_no_tree_fails_at_boot() {
        let mut app = App::new();
        app.insert_resource(FellCue {
            instance: "tree_9999".into(),
            at: 1.0,
            cut: 0,
            dir: 0,
            done: false,
        })
        .add_systems(PostStartup, check_fell_target);
        app.update();
    }

    /// A frame read back into memory is not a saved frame. Capture automation relies on a
    /// failing status when the output directory cannot be written; a mutant counting readback
    /// alone reports success here. The synthetic image never reaches a file.
    #[test]
    fn a_failed_frame_write_does_not_count_as_a_saved_frame() {
        use bevy::asset::RenderAssetUsages;
        use bevy::render::render_resource::{Extent3d, TextureDimension};
        let mut app = App::new();
        app.add_message::<AppExit>().insert_resource(Capture {
            path: PathBuf::from("unused.png"),
            at: vec![0.0],
            sight: false,
            size: UVec2::ONE,
            scale_factor: 2.0,
            gizmos: false,
            target: None,
            switched: false,
            requested: 1,
            saved: 0,
        });
        // Writing a PNG to an existing directory fails on every supported OS.
        let path = std::env::temp_dir();
        let entity = app.world_mut().spawn_empty().observe(save_frame(path)).id();
        let image = Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0; 4],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD,
        );
        app.world_mut()
            .trigger(ScreenshotCaptured { entity, image });
        assert_eq!(app.world().resource::<Capture>().saved, 0);
        assert!(
            app.world_mut()
                .resource_mut::<Messages<AppExit>>()
                .drain()
                .any(|exit| exit == AppExit::error())
        );
    }

    /// A KEY HOLD READS AS ONE HELD PRESS: pressed with one press edge on the first frame of its
    /// window, still pressed with no new edge inside it, and released with one release edge on the
    /// first frame past it. A view gated on `pressed` (the Battle board on Tab) shows for exactly
    /// the window; a mutant that only taps the key, or never releases it, fails here.
    #[test]
    fn a_key_hold_presses_once_and_releases_after_its_window() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(KeyHolds {
                holds: vec![plan::KeyHold {
                    key: KeyCode::Tab,
                    from: 1.0,
                    to: 2.0,
                }],
                held: Vec::new(),
            })
            .add_systems(Update, hold_keys);
        let at = |app: &mut App, seconds: f32| {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
            let mut time = app.world_mut().resource_mut::<Time>();
            let step = seconds - time.elapsed_secs();
            time.advance_by(std::time::Duration::from_secs_f32(step));
            app.update();
            let keys = app.world().resource::<ButtonInput<KeyCode>>();
            (
                keys.pressed(KeyCode::Tab),
                keys.just_pressed(KeyCode::Tab),
                keys.just_released(KeyCode::Tab),
            )
        };
        assert_eq!(
            at(&mut app, 0.5),
            (false, false, false),
            "before the window"
        );
        assert_eq!(at(&mut app, 1.0), (true, true, false), "the window opens");
        assert_eq!(at(&mut app, 1.5), (true, false, false), "held, no new edge");
        assert_eq!(at(&mut app, 2.0), (false, false, true), "the window closes");
        assert_eq!(at(&mut app, 3.0), (false, false, false), "after the window");
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
