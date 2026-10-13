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
//! * `OVERMATCH_CAPTURE_SIZE=WxH` — the frame in physical pixels, default 2560x1440. The target's
//!   scale factor is [`SCALE_FACTOR`], so the UI lays out at half that in logical pixels, as on a
//!   retina window.
//! * `OVERMATCH_CAPTURE_UI=0` — hide every UI tree, every frame, after whatever the app's own
//!   systems wrote to its roots' `Visibility` that frame.
//! * `OVERMATCH_CAPTURE_GIZMOS=0` — disable the default gizmo group when the cameras are
//!   retargeted.
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

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
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
    if !ui {
        app.add_systems(
            PostUpdate,
            hide_ui.before(VisibilitySystems::VisibilityPropagate),
        );
    }
}

/// Apps rewrite their UI roots' `Visibility` from `Update`; this runs after them and before the
/// value propagates, so the roots stay hidden whatever they wrote.
fn hide_ui(mut roots: Query<&mut Visibility, (With<Node>, Without<ChildOf>)>) {
    for mut visibility in &mut roots {
        visibility.set_if_neq(Visibility::Hidden);
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
fn hide_window(app: &mut App) {
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

fn capture(
    mut commands: Commands,
    time: Res<Time>,
    mut cap: ResMut<Capture>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(Entity, &Camera, &mut RenderTarget, Has<IsDefaultUiCamera>)>,
    gizmo_store: Option<ResMut<GizmoConfigStore>>,
    mode: Option<ResMut<SightMode>>,
    mut exit: MessageWriter<AppExit>,
) {
    let t = time.elapsed_secs();
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
    let image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    let target = ImageRenderTarget {
        handle: images.add(image),
        scale_factor: SCALE_FACTOR,
    };
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
    info!(
        "capture: {moved} camera(s) retargeted to a {}x{} image",
        size.x, size.y
    );
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_default_sort_and_drop_junk() {
        assert_eq!(parse_times(""), vec![6.0]);
        assert_eq!(parse_times("8, 2.5,x,4"), vec![2.5, 4.0, 8.0]);
    }

    #[test]
    fn one_frame_keeps_the_path_and_several_stamp_it() {
        let path = Path::new("/out/shot.png");
        assert_eq!(frame_path(path, 8.0, 1), path);
        assert_eq!(
            frame_path(path, 8.5, 2),
            Path::new("/out/shot-00008.5000s.png")
        );
    }

    /// A sequence assembled by name plays in time order: across whole seconds, across digit counts
    /// and between a short decimal and a long one (`1.1` before `1.1333`).
    #[test]
    fn frame_names_sort_in_time_order() {
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

    #[test]
    fn times_sharing_a_name_are_one_frame() {
        assert_eq!(parse_times("2.00001,2,3"), vec![2.0, 3.0]);
    }
}
