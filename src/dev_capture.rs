//! One in-engine frame to disk, for documentation. `OVERMATCH_CAPTURE=<png path>` saves a frame
//! `OVERMATCH_CAPTURE_AT` seconds after startup (default 6) and exits shortly after.
//! `OVERMATCH_CAPTURE_SIGHT=1` switches to the gunner optic two seconds before the capture,
//! writing the same resource the Shift toggle writes. `OVERMATCH_CAPTURE_SIZE=WxH` sets the
//! frame (default 2560x1440, rendered at scale factor 2 like a retina window). Mounted by the
//! offline root and the damage cam demo only, never by the network client; without the variable
//! it mounts nothing.
//!
//! One second before the capture every camera that targets the window is retargeted to an
//! offscreen image, and the frame is read back from that image. This is what lets the run stay
//! hidden: a hidden window has no swapchain, and the client's scaled main pass does not reach a
//! window screenshot even when the window is visible.

use bevy::app::AppExit;
use bevy::camera::{Camera, ImageRenderTarget, RenderTarget};
use bevy::prelude::*;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::sight::SightMode;

#[derive(Resource)]
struct Capture {
    path: String,
    at: f32,
    sight: bool,
    size: UVec2,
    target: Option<Handle<Image>>,
    switched: bool,
    shot: bool,
}

pub fn plugin(app: &mut App) {
    let Ok(path) = std::env::var("OVERMATCH_CAPTURE") else {
        return;
    };
    let at = std::env::var("OVERMATCH_CAPTURE_AT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6.0);
    let sight = std::env::var("OVERMATCH_CAPTURE_SIGHT").as_deref() == Ok("1");
    let size = std::env::var("OVERMATCH_CAPTURE_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some(UVec2::new(w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or(UVec2::new(2560, 1440));
    info!(
        "capture: {path} at {at:.1} s, sight {sight}, {}x{}",
        size.x, size.y
    );
    app.insert_resource(Capture {
        path,
        at,
        sight,
        size,
        target: None,
        switched: false,
        shot: false,
    });
    app.add_systems(Update, capture);
}

fn capture(
    mut commands: Commands,
    time: Res<Time>,
    mut cap: ResMut<Capture>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<&mut RenderTarget, With<Camera>>,
    mode: Option<ResMut<SightMode>>,
    mut exit: MessageWriter<AppExit>,
) {
    let t = time.elapsed_secs();
    if cap.sight && !cap.switched && t >= cap.at - 2.0 {
        if let Some(mut mode) = mode {
            *mode = SightMode::Gunner;
        }
        cap.switched = true;
    }
    if cap.target.is_none() && t >= cap.at - 1.0 {
        let mut image =
            Image::new_target_texture(cap.size.x, cap.size.y, TextureFormat::Rgba8UnormSrgb, None);
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let handle = images.add(image);
        let mut moved = 0;
        for mut target in &mut cameras {
            if matches!(*target, RenderTarget::Window(_)) {
                *target = RenderTarget::Image(ImageRenderTarget {
                    handle: handle.clone(),
                    scale_factor: 2.0,
                });
                moved += 1;
            }
        }
        info!("capture: {moved} camera(s) retargeted to the offscreen frame");
        cap.target = Some(handle);
    }
    if !cap.shot && t >= cap.at {
        if let Some(handle) = cap.target.clone() {
            commands
                .spawn(Screenshot::image(handle))
                .observe(save_to_disk(cap.path.clone()));
        }
        cap.shot = true;
    }
    if cap.shot && t >= cap.at + 1.5 {
        exit.write(AppExit::Success);
    }
}
