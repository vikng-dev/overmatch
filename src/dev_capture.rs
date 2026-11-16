//! In-engine frames to disk, for documentation of the game's presentation and damage model
//! (game state only). The plan file that
//! `OVERMATCH_CAPTURE=<plan>.ron` names holds one [`plan::CapturePlan`], whose struct docs are the
//! grammar's one home. `scripts/capture/example.ron` is a worked plan, checked by a test. Without
//! the variable every plugin here is inert. The operator's hand-driving lever `OVERMATCH_START`
//! and scenario levers with tracked performance-script users
//! (`OVERMATCH_TARGET`, `OVERMATCH_PROBE_FAR`, `OVERMATCH_PROBE_TANKS`,
//! `OVERMATCH_BATTLE`, `OVERMATCH_XRAY`) also select the scene; a capture inherits their values.
//!
//! [`CapturePlugin`] takes the plan's `frames` and `keys`. It is mounted after `crate::windowed_runtime`
//! by every windowed root: the offline route, both sandboxes, and the windowed network client. A
//! plan without `frames` runs its cues on a normal window and takes no frame (the scene of a frame
//! cost measurement). The cue plugins take the plan's other fields where an authority
//! can raise their events: [`offline_impact_plugin`],
//! [`offline_shot_plugin`] and [`offline_wall_plugin`] on the offline root alone, and
//! [`server_wall_plugin`] on the dedicated server, which reads the same plan file so that it fires
//! the wall shots its capture clients frame. [`offline_corner_plugin`] replays a recorded hit
//! through the offline shooter's corner for shader evidence, without changing simulated game state.
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
//! one (`windowed_runtime`): the primary window is created hidden, unfocused and `Windowed`,
//! settings never re-apply a persisted fullscreen to it, and the run is
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

mod clock;
pub use clock::Anchor;
pub(crate) use clock::Cues as CaptureCues;
mod corner;
pub(crate) use corner::offline_corner_plugin;
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

/// Resolution: seconds allowed for the GPU target, camera info and UI layout to follow retargeting.
const WARM_UP: f32 = 1.0;
/// Resolution: sight-camera settling time before the first frame, seconds.
const SIGHT_LEAD: f32 = 2.0;
/// Resolution: bound asynchronous frame-write waiting after the final frame, seconds.
const GRACE: f32 = 3.0;
/// Resolution: seconds for spawned burst views and GPU pipelines to prepare before retargeting.
const BURST_VIEW_WARM_UP: f32 = 1.0;
/// Resolution: a fixed seed makes cosmetic sampling reproducible.
const VIEW_SEED: u64 = 0x5EED;
/// Resolution: keep ray origins clear of rounded surface bounds for reliable hit normals, metres.
const IMPACT_PROBE_CLEARANCE_M: f32 = 1.0;

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
pub struct CapturePlugin(pub Anchor);

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        if let Some(plan) = plan() {
            build_plan(app, self.0, plan).unwrap_or_else(|error| plan::refuse(error));
        }
    }
}

/// The shipped plugin's plan-dependent build, also mounted with fixture plans by its tests.
pub(crate) fn build_plan(
    app: &mut App,
    anchor: Anchor,
    plan: &plan::CapturePlan,
) -> Result<(), plan::PlanError> {
    if plan
        .frames
        .as_ref()
        .is_some_and(|frames| matches!(frames.camera, plan::Camera::Follow { .. }))
        && !matches!(anchor, Anchor::Scenario | Anchor::Startup)
    {
        return Err(plan::PlanError::FollowWithoutTank(anchor));
    }
    // Frames choose their update strategy before the clock suspends it.
    mount_plan(app, plan);
    clock::install(app, anchor);
    Ok(())
}

fn mount_plan(app: &mut App, plan: &plan::CapturePlan) {
    if let Some(frames) = &plan.frames {
        mount(app, frames);
    }
    if !plan.keys.is_empty() {
        let holds = plan.keys.clone();
        info!("capture: keys {holds:?}");
        app.insert_resource(KeyHolds {
            holds,
            held: Vec::new(),
        })
        .add_systems(
            PreUpdate,
            hold_keys
                .after(bevy::input::InputSystems)
                .in_set(clock::Cues),
        );
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

#[derive(Resource)]
struct BoundOrbit(Entity);

fn bind_orbit(world: &mut World) -> Result<Entity, plan::PlanError> {
    world
        .query_filtered::<Entity, (
            With<crate::view::PlayerView>,
            With<crate::camera::OrbitCamera>,
        )>()
        .single(world)
        .map_err(plan::PlanError::OrbitCamera)
}

fn prepare_output(path: &Path) -> Result<(), plan::PlanError> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|source| plan::PlanError::Output {
            path: dir.to_owned(),
            source,
        })?;
    }
    let output_error = |source| plan::PlanError::Output {
        path: path.to_owned(),
        source,
    };
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => {
            drop(file);
            std::fs::remove_file(path).map_err(output_error)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(output_error)?;
        }
        Err(error) => return Err(output_error(error)),
    }
    Ok(())
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
    for at in &frames.at {
        prepare_output(&frame_path(&frames.out, *at, frames.at.len()))
            .unwrap_or_else(|error| plan::refuse(error));
    }
    // The effects plugin only `init_resource`s the RNG, so this seed holds in either mount order.
    app.insert_resource(crate::vfx::ViewRng::seeded(VIEW_SEED));
    if let Some(step) = frames.step_s {
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(step.0));
    }
    match frames.camera {
        plan::Camera::Player => {}
        plan::Camera::Orbit { direction } => {
            let rotation = Transform::IDENTITY.looking_to(direction, Dir3::Y).rotation;
            app.add_systems(PostStartup, |world: &mut World| {
                let camera = bind_orbit(world).unwrap_or_else(|error| plan::refuse(error));
                world.insert_resource(BoundOrbit(camera));
            });
            app.add_systems(
                PostUpdate,
                (move |camera: Res<BoundOrbit>, mut transforms: Query<&mut Transform>| {
                    transforms
                        .get_mut(camera.0)
                        .expect("the orbit camera bound at startup retains its transform")
                        .rotation = rotation;
                })
                .before(crate::camera::OrbitCameraSet)
                .before(crate::camera::GunnerCameraPlaced)
                .after(crate::state::PlayerInputSet),
            );
        }
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
    app.add_systems(Update, capture.in_set(clock::Cues));
    if !frames.ui {
        app.add_systems(
            PostUpdate,
            hide_ui.before(VisibilitySystems::VisibilityPropagate),
        );
    }
}

/// The game's damage-model cue refuses unknown rounds and modes at plan admission.
#[derive(Debug)]
enum RoundCueError {
    Round {
        field: &'static str,
        error: crate::guns::GunError,
    },
    Mode {
        field: &'static str,
        round: String,
        mode: u8,
    },
}

impl std::fmt::Display for RoundCueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Round { field, error } => write!(
                f,
                "{field}.round: {error}; name a round chambered by one shipped gun"
            ),
            Self::Mode { field, round, mode } => write!(
                f,
                "{field}.mode: round {round:?} has no mode {mode}; choose 0 for an inert round or one of its authored fuze modes"
            ),
        }
    }
}

fn resolve_round(
    key: &str,
    mode: u8,
    field: &'static str,
) -> Result<crate::rounds::Round, RoundCueError> {
    let round = crate::tank::WeaponRound::resolve(key)
        .map_err(|error| RoundCueError::Round { field, error })?;
    round.in_mode(mode).ok_or_else(|| RoundCueError::Mode {
        field,
        round: key.into(),
        mode,
    })
}

/// The game's damage-model terrain cue, admitted inside the terrain span.
#[derive(Debug)]
struct ImpactCue(plan::Impact);

/// The plan's `impacts` ([`plan::Impact`]) not yet raised.
#[derive(Resource, Debug)]
struct ImpactCues(Vec<ImpactCue>);

/// The capture's terrain-impact cue, mounted by the offline root only, where the impact seam has
/// no replica reading it. Inert unless the plan lists `impacts`.
pub(crate) fn offline_impact_plugin(app: &mut App) {
    if let Some(plan) = plan() {
        install_impacts(app, &plan.impacts);
    }
}

fn admit_impact(
    cue: &plan::Impact,
    grid: Option<&crate::terrain_grid::HeightGrid>,
) -> Result<ImpactCue, plan::PlanError> {
    let half_extent = crate::terrain_grid::world_extent(grid).half_extent();
    if cue.x.abs() > half_extent || cue.z.abs() > half_extent {
        return Err(plan::PlanError::ImpactOffTerrain {
            x: cue.x,
            z: cue.z,
            half_extent,
        });
    }
    Ok(ImpactCue(cue.clone()))
}

fn install_impacts(app: &mut App, cues: &[plan::Impact]) {
    if cues.is_empty() {
        return;
    }
    let cues = cues.to_vec();
    app.add_systems(
        PostStartup,
        move |grid: Option<Res<crate::terrain_grid::HeightGrid>>, mut commands: Commands| {
            let cues = cues
                .iter()
                .map(|cue| admit_impact(cue, grid.as_deref()))
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|error| plan::refuse(error));
            commands.insert_resource(ImpactCues(cues));
        },
    )
    .add_systems(Update, raise_impacts.in_set(clock::Cues));
}

/// Raise each due strike on the ground under its point, with the ground's normal there.
fn raise_impacts(
    time: Res<Time>,
    mut cues: ResMut<ImpactCues>,
    spatial: avian3d::prelude::SpatialQuery,
    terrain: Query<(
        &avian3d::prelude::ColliderAabb,
        &avian3d::prelude::CollisionLayers,
    )>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    if !cues.0.iter().any(|ImpactCue(cue)| cue.at <= now) {
        return;
    }
    let (bottom, top) = terrain
        .iter()
        .filter(|(_, layers)| layers.memberships.has_all(crate::Layer::Terrain))
        .fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(bottom, top), (bounds, _)| (bottom.min(bounds.min.y), top.max(bounds.max.y)),
        );
    assert!(
        bottom.is_finite() && top.is_finite() && bottom <= top,
        "capture impacts require prepared terrain collider bounds before their cue"
    );
    let origin_y = top + IMPACT_PROBE_CLEARANCE_M;
    let distance = origin_y - bottom + IMPACT_PROBE_CLEARANCE_M;
    let filter = avian3d::prelude::SpatialQueryFilter::from_mask(crate::Layer::Terrain);
    cues.0.retain(|ImpactCue(cue)| {
        if cue.at > now {
            return true;
        }
        let origin = Vec3::new(cue.x, origin_y, cue.z);
        match spatial.cast_ray(origin, Dir3::NEG_Y, distance, true, &filter) {
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
            None => panic!("capture: terrain collider must cover the impacts.x/z admitted inside its span: {cue:?}"),
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
    // The game's damage-model shot frames count from the observed burst, after target warm-up
    // and burst-view preparation; the plan's relative frame spacing remains intact.
    let t = if let Some(ready) = shot_ready {
        let Some(at) = ready.0 else {
            return;
        };
        time.elapsed_secs() - at + cap.at[0] - WARM_UP - BURST_VIEW_WARM_UP
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

    use bevy::ecs::schedule::{NodeId, ScheduleLabel, SystemSet};

    fn assert_cues(app: &App, label: impl ScheduleLabel, systems: &[&str]) {
        let schedules = app.world().resource::<bevy::ecs::schedule::Schedules>();
        let graph = schedules.get(label).unwrap().graph();
        let set = graph
            .system_sets
            .get_key(clock::Cues.intern())
            .expect("cue set exists");
        let hierarchy = graph.hierarchy().clone().analyze().unwrap();
        for name in systems {
            let (key, _, _) = graph
                .systems
                .iter()
                .find(|(_, system, _)| system.name().to_string().ends_with(name))
                .expect(name);
            assert!(
                hierarchy
                    .transitive_closure()
                    .contains_edge(NodeId::Set(set), NodeId::System(key)),
                "{name} must wait for the capture anchor"
            );
        }
    }

    /// The shipped builder must freeze the plan-selected frame strategy and every zero-time
    /// consumer until readiness; captures and scripted key input rely on the same release update.
    #[test]
    fn shipped_plan_builder_holds_frames_and_keys_until_placement() {
        use bevy::time::TimeUpdateStrategy;
        let path = std::env::temp_dir().join(format!(
            "overmatch-clock-fixture-{}.png",
            std::process::id()
        ));
        let fixture = plan::CapturePlan::parse(&format!(r#"(frames: (out: {path:?}, at: [10.0], step_s: 0.01), keys: [(key: KeyW, from: 0.0, to: 1.0)])"#)).unwrap();
        let step = fixture.frames.as_ref().unwrap().step_s.unwrap().0;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::state::app::StatesPlugin,
            bevy::input::InputPlugin,
        ))
        .init_state::<crate::state::AppState>()
        .init_resource::<Assets<Image>>()
        .add_plugins(move |app: &mut App| {
            build_plan(app, Anchor::Scenario, &fixture).unwrap();
        });
        assert_cues(&app, PreUpdate, &["hold_keys"]);
        assert_cues(&app, Update, &["capture"]);
        for _ in 0..3 {
            // Fixture loading frames.
            app.update();
            assert_eq!(
                app.world().resource::<Time>().elapsed(),
                std::time::Duration::ZERO
            );
            assert!(
                !app.world()
                    .resource::<ButtonInput<KeyCode>>()
                    .pressed(KeyCode::KeyW)
            );
            assert!(app.world().resource::<Capture>().target.is_none());
        }
        app.world_mut()
            .spawn((crate::tank::Tank, crate::tank::TankViewReady));
        app.insert_resource(State::new(crate::state::AppState::Playing));
        app.update();
        assert!(
            app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyW)
        );
        assert!(
            matches!(*app.world().resource::<TimeUpdateStrategy>(), TimeUpdateStrategy::ManualDuration(actual) if actual == step)
        );
        app.update();
        assert_eq!(app.world().resource::<Time>().elapsed(), step);
    }

    /// The game's damage-model cues (game state only) must share the clock gate even at zero.
    /// This reads production registrations, so removing membership from any consumer fails.
    #[test]
    fn all_offline_cue_consumers_belong_to_the_clock_gate() {
        let fixture = plan::CapturePlan::parse(r#"(impacts: [(x: 0.0, z: 0.0, round: "88mm_AP", surface: Terrain, at: 0.0)], shot: (round: "88mm_HE", mode: 0, target: Ground, at: 0.0), wall: (shots: [Round(round: "88mm_AP", at: 0.0)]))"#).unwrap();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        build_plan(&mut app, Anchor::Scenario, &fixture).unwrap();
        install_impacts(&mut app, &fixture.impacts);
        fuze::install_shot(&mut app, fixture.shot.unwrap(), plan::refuse);
        wall::install(
            &mut app,
            wall::cue_for(fixture.wall.as_ref().unwrap()),
            true,
        );
        repair::install(&mut app);
        corner::install(
            &mut app,
            &plan::CornerReplay {
                at: 0.0,
                report: corner::ReplayReport::try_from(PathBuf::from(
                    "assets/dev/pen_cam/s8b_mg_stopped.ron",
                ))
                .unwrap(),
            },
        );
        assert_cues(
            &app,
            Update,
            &[
                "raise_impacts",
                "fuze::fire_on_cue",
                "wall::fire_on_cue",
                "start_panel_repair",
                "replay",
            ],
        );
        let mut server = App::new();
        server.add_plugins(MinimalPlugins);
        wall::install_server(&mut server, wall::cue_for(fixture.wall.as_ref().unwrap()));
        assert_cues(&server, Update, &["wall::fire_on_cue"]);
    }

    /// A Follow camera cannot bind tankless roots; operators get the field and the repair before
    /// a frame or cue runs, rather than an assertion when the target arms.
    #[test]
    fn follow_is_refused_at_composition_admission() {
        // Fixture camera on a root with no admitted controlled tank.
        let fixture = plan::CapturePlan::parse(r#"(frames: (out: "/tmp/overmatch-follow-fixture.png", camera: Follow(offset: (1.0, 2.0, 3.0))))"#).unwrap();
        for anchor in [Anchor::TankView, Anchor::TrackRig] {
            let mut app = App::new();
            let error = build_plan(&mut app, anchor, &fixture).unwrap_err();
            assert!(matches!(error, plan::PlanError::FollowWithoutTank(_)));
            let message = error.to_string();
            assert!(
                message.contains("frames.camera") && message.contains("Pose"),
                "{message}"
            );
            assert!(
                !app.world().contains_resource::<Capture>(),
                "refusal precedes installation"
            );
        }
    }

    /// Offline and online capture operators can follow their controlled tank. The shipped
    /// builder must admit both roots and keep following after the frame target arms.
    #[test]
    fn follow_tracks_the_controlled_tank_on_scenario_and_startup_roots() {
        for anchor in [Anchor::Scenario, Anchor::Startup] {
            let path = std::env::temp_dir().join(format!(
                "overmatch-follow-{anchor:?}-{}.png",
                std::process::id()
            ));
            let fixture = plan::CapturePlan::parse(&format!(
                r#"(frames: (out: {path:?}, at: [0.0], camera: Follow(offset: (1.0, 2.0, 3.0))))"#
            ))
            .unwrap();
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .init_resource::<Assets<Image>>()
                .insert_resource(State::new(crate::state::AppState::Playing));
            build_plan(&mut app, anchor, &fixture).unwrap();
            let tank = app
                .world_mut()
                .spawn((
                    crate::tank::Tank,
                    crate::tank::Controlled,
                    crate::tank::TankViewReady,
                    Transform::IDENTITY,
                ))
                .id();
            let camera = app
                .world_mut()
                .spawn((crate::view::PlayerView, Transform::IDENTITY))
                .id();
            for position in [Vec3::ZERO, Vec3::new(4.0, 5.0, 6.0)] {
                // Fixture movement: the camera must follow the owner, not its initial pose.
                app.world_mut()
                    .get_mut::<Transform>(tank)
                    .unwrap()
                    .translation = position;
                app.update();
                assert!(app.world().resource::<Capture>().target.is_some());
                let transform = app.world().get::<Transform>(camera).unwrap();
                assert_eq!(transform.translation, position + Vec3::new(1.0, 2.0, 3.0));
                assert!(
                    transform
                        .forward()
                        .dot((position - transform.translation).normalize())
                        > 0.999
                );
            }
        }
    }

    /// Startup also serves tankless servers; Follow must still fail when retargeting finds no
    /// controlled tank, rather than silently capturing an unrelated view.
    #[test]
    #[should_panic(
        expected = "frames.camera Follow needs a controlled tank before the first frame"
    )]
    fn startup_follow_requires_a_controlled_tank_when_the_target_arms() {
        let path = std::env::temp_dir().join(format!(
            "overmatch-follow-tankless-{}.png",
            std::process::id()
        ));
        let fixture = plan::CapturePlan::parse(&format!(
            r#"(frames: (out: {path:?}, at: [0.0], camera: Follow(offset: (1.0, 2.0, 3.0))))"#
        ))
        .unwrap();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<Image>>();
        build_plan(&mut app, Anchor::Startup, &fixture).unwrap();
        app.update();
    }

    /// The game's damage-model terrain cues must refuse coordinates outside either the loaded
    /// grid or the flat world's slab before any cue time is consumed.
    #[test]
    fn impacts_admit_their_terrain_domain_before_play() {
        let cue = plan::CapturePlan::parse(
            r#"(impacts: [(x: 0.0, z: 0.0, round: "88mm_AP", surface: Terrain, at: 0.0)])"#,
        )
        .unwrap()
        .impacts
        .remove(0);
        let grid = crate::terrain_grid::HeightGrid::new(
            vec![0.0; 4].into(),
            2,
            crate::terrain_grid::FIXTURE_EXTENT,
        );
        for grid in [None, Some(&grid)] {
            assert!(admit_impact(&cue, grid).is_ok());
            let half = crate::terrain_grid::world_extent(grid).half_extent();
            for (x, z) in [(half * 2.0, 0.0), (0.0, -half * 2.0)] {
                let error = admit_impact(
                    &plan::Impact {
                        x,
                        z,
                        ..cue.clone()
                    },
                    grid,
                )
                .unwrap_err();
                assert!(matches!(error, plan::PlanError::ImpactOffTerrain { .. }));
                assert!(
                    error.to_string().contains("impacts.x/z")
                        && error.to_string().contains("inside")
                );
            }
        }
    }

    /// Capture operators get a field-level refusal on a free-fly root before frame systems run.
    #[test]
    fn orbit_binding_refuses_a_root_without_an_orbit_camera() {
        let mut world = World::new();
        world.spawn((crate::view::PlayerView, Transform::IDENTITY));
        let error = bind_orbit(&mut world).unwrap_err().to_string();
        assert!(
            error.contains("frames.camera") && error.contains("Pose"),
            "{error}"
        );
    }

    /// A file blocking the parent directory must refuse startup through the plan error boundary.
    #[test]
    fn output_parent_failure_preserves_the_path_and_os_cause() {
        let parent =
            std::env::temp_dir().join(format!("overmatch-wall-output-{}", std::process::id()));
        std::fs::write(&parent, b"fixture").unwrap();
        let error = prepare_output(&parent.join("frame.png")).unwrap_err();
        std::fs::remove_file(&parent).unwrap();
        assert!(matches!(&error, plan::PlanError::Output { path, .. } if path == &parent));
        let message = error.to_string();
        assert!(
            message.contains("frames.out") && message.contains("permissions"),
            "{message}"
        );
    }

    /// Output preflight refuses a directory as a frame and preserves existing frame bytes.
    #[test]
    fn output_preflight_checks_the_file_without_truncating_it() {
        let dir = std::env::temp_dir().join(format!("overmatch-wall-frame-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            prepare_output(&dir),
            Err(plan::PlanError::Output { .. })
        ));
        let path = dir.join("frame.png");
        prepare_output(&path).unwrap();
        assert!(!path.exists(), "preflight removes its new probe file");
        std::fs::write(&path, b"fixture").unwrap();
        prepare_output(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fixture");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A raised strike reaches the seam as the plan resolved it: on the ground its probe found,
    /// carrying the cue's own round and surface. The impact stills judged against ADR-0023 rely on
    /// it, since no round flies. Mutant: `surface: ImpactSurface::Terrain` hard-coded in
    /// [`raise_impacts`] shows every substance's still as soil and fails here.
    #[test]
    fn a_raised_strike_emits_its_resolved_read_on_the_probed_ground() {
        use crate::ballistics::Impact;
        use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
        use bevy::time::TimeUpdateStrategy;

        #[derive(Resource, Default)]
        struct Raised(Vec<(Vec3, Vec3, crate::rounds::ImpactStyle, ImpactSurface)>);
        use crate::ballistics::ImpactSurface;

        // Fixture: a cue resolved through the same admission as an operator's plan.
        let cue = plan::CapturePlan::parse(r#"(impacts: [(x: 3.0, z: -4.0, round: "88mm_AP", surface: Substance("Wood"), at: 0.0)])"#).unwrap().impacts.remove(0);
        let (style, surface) = (cue.style, cue.surface);
        assert!(matches!(surface, ImpactSurface::Material(_)));

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            crate::offline_physics(),
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
        // Fixture: elevated terrain; the probe must derive its range from the collider.
        let terrain_y = 2000.0;
        let thickness = 2.0;
        app.world_mut().spawn((
            RigidBody::Static,
            Transform::from_xyz(0.0, terrain_y, 0.0),
            Collider::cuboid(100.0, thickness, 100.0),
            CollisionLayers::new(crate::Layer::Terrain, LayerMask::ALL),
        ));
        app.update();
        app.update();
        app.insert_resource(ImpactCues(vec![admit_impact(&cue, None).unwrap()]))
            .add_systems(Update, raise_impacts);
        app.update();
        let raised = &app.world().resource::<Raised>().0;
        assert_eq!(
            raised.as_slice(),
            [(
                Vec3::new(cue.x, terrain_y + thickness / 2.0, cue.z),
                Vec3::Y,
                style,
                surface
            )]
        );
        assert!(app.world().resource::<ImpactCues>().0.is_empty());
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
