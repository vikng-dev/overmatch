//! Dev-only debug helpers (compiled only with `debug_assertions`). Currently an X-ray toggle:
//! press `X` to make the tank translucent so the physics gizmos that sit *inside* the model —
//! suspension rays, the hull collider — show through. Translucent (Blend) materials stop
//! writing depth, so the depth-tested gizmos behind them become visible.

use avian3d::prelude::PhysicsGizmos;
use bevy::color::Alpha;
use bevy::prelude::*;

use crate::camera::CameraFollow;
use crate::driving::Suspension;
use crate::tank::{Controlled, Tank};

/// How opaque the tank is in x-ray mode (0 = invisible, 1 = solid).
const XRAY_ALPHA: f32 = 0.2;
/// Metres of arrow per newton of suspension force (so ~35 kN reads as a ~1.75 m arrow).
const FORCE_VIZ_SCALE: f32 = 1.0 / 20_000.0;

pub fn plugin(app: &mut App) {
    app.init_resource::<XRay>()
        // Off by default, like the x-ray — press G to bring all debug gizmos up.
        .insert_resource(ShowGizmos(false))
        .add_systems(Startup, configure_physics_gizmos)
        .add_systems(Update, (toggle_xray, toggle_camera_follow, toggle_gizmos))
        // Mirror the on/off state onto Avian's own gizmos (colliders, suspension rays).
        .add_systems(
            Update,
            sync_avian_gizmos.run_if(resource_changed::<ShowGizmos>),
        )
        // Draw after propagation so the arrows anchor to the tank's *interpolated* pose and stay
        // glued to the rendered wheels, instead of stepping at the physics tick rate.
        .add_systems(
            PostUpdate,
            draw_wheel_forces
                .after(TransformSystems::Propagate)
                .run_if(|show: Res<ShowGizmos>| show.0),
        );
}

/// Master switch for all debug gizmos — our force arrows plus Avian's colliders/rays. Toggled `G`.
#[derive(Resource)]
struct ShowGizmos(bool);

fn toggle_gizmos(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowGizmos>) {
    if keys.just_pressed(KeyCode::KeyG) {
        show.0 = !show.0;
    }
}

/// Enable/disable Avian's `PhysicsGizmos` group to match `ShowGizmos`.
fn sync_avian_gizmos(show: Res<ShowGizmos>, mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<PhysicsGizmos>().0.enabled = show.0;
}

/// Avian's raycast gizmo samples at the physics tick and can't interpolate, so we silence it and
/// draw our own synced ray in `draw_wheel_forces`. Its collider gizmo uses `GlobalTransform` (so
/// it's already interpolated) — we keep that one. The result: all gizmos move with the rendered tank.
fn configure_physics_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (_, gizmos) = store.config_mut::<PhysicsGizmos>();
    gizmos.raycast_color = None;
    gizmos.raycast_point_color = None;
    gizmos.raycast_normal_color = None;
}

/// `F` detaches/re-attaches the camera from the tank (holds it static) — lets you drive past a
/// fixed view to tell camera-follow jitter from physics jitter.
fn toggle_camera_follow(keys: Res<ButtonInput<KeyCode>>, mut follow: ResMut<CameraFollow>) {
    if keys.just_pressed(KeyCode::KeyF) {
        follow.0 = !follow.0;
    }
}

/// Draw each grounded wheel's forces: cyan = suspension load (up), orange = horizontal drive +
/// friction. Anchored to the wheel's *interpolated* `GlobalTransform` (read after propagation) so
/// the arrows stay glued to the rendered tank rather than stepping at the physics rate. Arrow
/// length is proportional to force — a live load/traction readout.
fn draw_wheel_forces(wheels: Query<(&GlobalTransform, &Suspension)>, mut gizmos: Gizmos) {
    for (transform, suspension) in &wheels {
        // The wheel's real ground contact (the ray's hit point); airborne wheels have none.
        let Some(contact) = suspension.contact else {
            continue;
        };
        let hub = transform.translation();
        // Our synced replacement for Avian's physics-rate suspension ray (hub → hit point).
        gizmos.line(hub, contact, Color::srgb(0.9, 0.2, 0.2));
        if suspension.load > 0.0 {
            // Normal load along the (interpolated) hull-up suspension axis, not world up — so it
            // leans with the hull on a slope. (Wheel nodes share the hull's orientation.)
            let tip = contact + transform.up() * (suspension.load * FORCE_VIZ_SCALE);
            gizmos.arrow(contact, tip, Color::srgb(0.1, 0.9, 1.0));
        }
        if suspension.drive_force != Vec3::ZERO {
            let tip = contact + suspension.drive_force * FORCE_VIZ_SCALE;
            gizmos.arrow(contact, tip, Color::srgb(1.0, 0.55, 0.1));
        }
    }
}

/// Whether the tank is currently rendered translucent for debug viewing.
#[derive(Resource, Default)]
struct XRay(bool);

fn toggle_xray(
    keys: Res<ButtonInput<KeyCode>>,
    mut xray: ResMut<XRay>,
    tank: Single<Entity, (With<Tank>, With<Controlled>)>,
    children: Query<&Children>,
    mesh_mats: Query<&MeshMaterial3d<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !keys.just_pressed(KeyCode::KeyX) {
        return;
    }
    xray.0 = !xray.0;
    let (alpha, mode) = if xray.0 {
        (XRAY_ALPHA, AlphaMode::Blend)
    } else {
        (1.0, AlphaMode::Opaque)
    };

    // Walk the tank's mesh descendants and retint their (shared) materials. Mutating an asset
    // touches every entity using it, which is exactly what we want — the whole tank fades.
    for entity in children.iter_descendants(*tank) {
        let Ok(handle) = mesh_mats.get(entity) else {
            continue;
        };
        if let Some(mut material) = materials.get_mut(&handle.0) {
            material.base_color = material.base_color.with_alpha(alpha);
            material.alpha_mode = mode;
        }
    }
}
