//! Dev-only debug helpers (compiled behind the `dev_tools` feature — default-on, and deliberately
//! decoupled from `debug_assertions` so an optimized/`--release` playtest build still carries these
//! keys; see `Cargo.toml`). Press `G` to toggle the debug
//! gizmos: belt-contact force arrows (cyan = support load along the contact normal, orange =
//! traction) plus Avian's collider wireframes. Press `X` for the X-ray toggle: the tank
//! turns translucent so the gizmos that sit *inside* the model show through (Blend materials stop
//! writing depth, so the depth-tested gizmos behind them become visible). `F` detaches the camera.
//!
//! Mounted by BOTH client compositions — `ClientPlugin` (single-player) and `NetClientPlugin`
//! (the networked client bin) — always paired with Avian's `PhysicsDebugPlugin` (which registers
//! the `PhysicsGizmos` group this module configures). Strictly view-only: every system reads sim
//! state (`&TrackContacts`, `&GlobalTransform`) immutably and writes only render-side things (gizmos,
//! materials, camera follow), so it is safe on a predicting net client; the headless server never
//! mounts it.

use std::collections::VecDeque;

use avian3d::prelude::PhysicsGizmos;
use bevy::color::Alpha;
use bevy::prelude::*;

use crate::ballistics::{Impact, ImpactMarker};
use crate::camera::CameraFollow;
use crate::tank::{Controlled, Tank};
use crate::track::sim::TrackContacts;

/// How opaque the tank is in x-ray mode (0 = invisible, 1 = solid).
const XRAY_ALPHA: f32 = 0.2;
/// Metres of arrow per newton of contact force (so ~35 kN reads as a ~1.75 m arrow).
const FORCE_VIZ_SCALE: f32 = 1.0 / 20_000.0;
/// How many impact markers the game client keeps on screen before the oldest is despawned — the
/// markers are never explicitly cleared here (unlike the sandbox's `C` key), so this ring bounds
/// what would otherwise accumulate forever.
const IMPACT_MARKER_CAP: usize = 32;

pub fn plugin(app: &mut App) {
    app.init_resource::<XRay>()
        .init_resource::<ImpactMarkerRing>()
        // Off by default, like the x-ray — press G to bring all debug gizmos up.
        .insert_resource(ShowGizmos(false))
        .add_systems(Startup, (configure_physics_gizmos, setup_impact_marker))
        // The debug impact marker: a view-side subscriber to `ballistics`' sim `Impact` event
        // (ADR-0014), gated on `ShowGizmos` and ring-buffered to `IMPACT_MARKER_CAP`.
        .add_observer(spawn_impact_marker)
        .add_systems(Update, (toggle_xray, toggle_camera_follow, toggle_gizmos))
        // Mirror the on/off state onto Avian's own gizmos (collider wireframes).
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
/// draw our own synced arrows in `draw_wheel_forces`. Its collider gizmo uses `GlobalTransform` (so
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

/// Draw each belt contact's forces: cyan = elastic support load (along the contact's inward
/// normal), orange = traction. Reads the sim's per-tick contact telemetry ([`TrackContacts`],
/// world points at the tick pose) — a live load/traction readout of the phase-B belt model.
fn draw_wheel_forces(tanks: Query<&TrackContacts>, mut gizmos: Gizmos) {
    for contacts in &tanks {
        for side in &contacts.0 {
            for c in side {
                if c.load > 0.0 {
                    let tip = c.point + c.normal * (c.load * FORCE_VIZ_SCALE);
                    gizmos.arrow(c.point, tip, Color::srgb(0.1, 0.9, 1.0));
                }
                if c.traction != Vec3::ZERO {
                    let tip = c.point + c.traction * FORCE_VIZ_SCALE;
                    gizmos.arrow(c.point, tip, Color::srgb(1.0, 0.55, 0.1));
                }
            }
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

/// Preloaded mesh+material for the debug impact marker, cloned per hit by `spawn_impact_marker`.
#[derive(Resource)]
struct ImpactDebug {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// The live impact markers in spawn order, oldest at the front — a ring that evicts its front once
/// it passes `IMPACT_MARKER_CAP`, so markers don't accumulate forever in the game client.
#[derive(Resource, Default)]
struct ImpactMarkerRing(VecDeque<Entity>);

/// Small red sphere reused for every impact marker.
fn setup_impact_marker(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(ImpactDebug {
        mesh: meshes.add(Sphere::new(0.2)),
        material: materials.add(Color::srgb(1.0, 0.3, 0.1)),
    });
}

/// Drop a red sphere at each shell impact — but only while the `G` gizmo toggle is up, and keeping
/// at most `IMPACT_MARKER_CAP` on screen (the oldest is despawned when a new one pushes past the
/// cap). View-only, so it is safe on a predicting net client.
fn spawn_impact_marker(
    impact: On<Impact>,
    show: Res<ShowGizmos>,
    debug: Res<ImpactDebug>,
    mut ring: ResMut<ImpactMarkerRing>,
    mut commands: Commands,
) {
    if !show.0 {
        return;
    }
    let marker = commands
        .spawn((
            ImpactMarker,
            Mesh3d(debug.mesh.clone()),
            MeshMaterial3d(debug.material.clone()),
            Transform::from_translation(impact.position),
        ))
        .id();
    ring.0.push_back(marker);
    // Evict from the front until back under the cap. `try_despawn` is a silent no-op if the marker
    // is already gone (e.g. a scene reset despawned it out from under us).
    while ring.0.len() > IMPACT_MARKER_CAP {
        if let Some(old) = ring.0.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ballistics::ImpactSurface;

    /// Minimal app carrying just what `spawn_impact_marker` reads — no asset plugins needed, since
    /// the observer only *clones* the preloaded handles (default handles are fine for a headless run).
    fn harness(show: bool) -> App {
        let mut app = App::new();
        app.insert_resource(ShowGizmos(show))
            .init_resource::<ImpactMarkerRing>()
            .insert_resource(ImpactDebug {
                mesh: Handle::default(),
                material: Handle::default(),
            })
            .add_observer(spawn_impact_marker);
        app
    }

    fn marker_count(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<ImpactMarker>>()
            .iter(app.world())
            .count()
    }

    #[test]
    fn gizmos_off_spawns_no_marker() {
        let mut app = harness(false);
        app.world_mut().trigger(Impact {
            position: Vec3::ZERO,
            normal: Vec3::Y,
            caliber: 0.088,
            surface: ImpactSurface::Terrain,
            penetrated: false,
            deflection: None,
        });
        app.world_mut().flush();
        assert_eq!(marker_count(&mut app), 0);
    }

    #[test]
    fn ring_caps_markers_and_evicts_oldest() {
        let mut app = harness(true);
        // DERIVED: overflowing the cap by five distinct points must evict those five oldest
        // positions while the newest cap-sized suffix remains observable.
        for index in 0..IMPACT_MARKER_CAP + 5 {
            app.world_mut().trigger(Impact {
                position: Vec3::X * index as f32,
                normal: Vec3::Y,
                caliber: 0.088,
                surface: ImpactSurface::Terrain,
                penetrated: false,
                deflection: None,
            });
            app.world_mut().flush();
        }
        assert_eq!(marker_count(&mut app), IMPACT_MARKER_CAP);
        assert_eq!(
            app.world().resource::<ImpactMarkerRing>().0.len(),
            IMPACT_MARKER_CAP
        );
        let ring = &app.world().resource::<ImpactMarkerRing>().0;
        let first = app.world().get::<Transform>(ring[0]).unwrap();
        let last = app.world().get::<Transform>(*ring.back().unwrap()).unwrap();
        assert_eq!(first.translation, Vec3::X * 5.0, "oldest five were evicted");
        assert_eq!(
            last.translation,
            Vec3::X * (IMPACT_MARKER_CAP + 4) as f32,
            "newest marker survives the overflow"
        );
        for evicted in 0..5 {
            let still_live = app
                .world_mut()
                .query_filtered::<&Transform, With<ImpactMarker>>()
                .iter(app.world())
                .any(|transform| transform.translation == Vec3::X * evicted as f32);
            assert!(!still_live, "oldest marker at x={evicted} was evicted");
        }
    }
}
