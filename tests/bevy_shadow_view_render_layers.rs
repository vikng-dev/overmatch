//! GUARD for the shadow-view behaviour `src/render_policy.rs` and `src/track/shadow_proxy.rs` rely on:
//! every shadow view carries the LIGHT's `RenderLayers`, so a caster on a non-zero layer still
//! casts from a light whose mask covers that layer. Bevy 0.19.1 ships this (bevyengine/bevy
//! #24797; mechanism record `upstream/bevy-shadow-view-ignores-light-render-layers.md`), and the
//! `bevy_pbr` this game links is still the path-patched copy in `vendor/`, so a re-vendor that
//! drops it would silently un-shadow the local tank and the track ribbon.
//!
//! ## Why the render-world half is source-scanned rather than executed
//!
//! The construction sites live in `prepare_lights`, a render-world system. With
//! `WgpuSettings { backends: None }` bevy 0.19 does not create the `RenderApp` at all, so no
//! shadow view is ever built. Executing that half needs a real wgpu adapter, which CI
//! (`ubuntu-latest`, no GPU) does not have — a GPU-gated test would silently no-op there. So the
//! render-world half is pinned by scanning the linked source for the fix's anchors, and the
//! main-world half — the light-mask filter the shadow views must agree with — is executed.

use std::any::TypeId;
use std::path::PathBuf;

use bevy::camera::visibility::{CascadesVisibleEntities, RenderLayers, VisibleEntities};
use bevy::prelude::*;
use bevy::render::settings::WgpuSettings;

/// The linked `bevy_pbr`'s light module (wired in via `[patch.crates-io]`).
const LINKED_LIGHT_RS: &str = "vendor/bevy_pbr-0.19.1-scalar-math/src/render/light.rs";

fn linked_light_rs() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(LINKED_LIGHT_RS);
    std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "cannot read the linked bevy_pbr light module at {}: {err}. If the vendor entry moved, \
             point this guard at the bevy_pbr the game now links.",
            path.display()
        )
    })
}

/// Every `commands.entity(..)..` statement in `src` that constructs a `ShadowView { .. }`.
///
/// Anchored on syntax that must exist for a shadow view to exist at all — the `ShadowView`
/// component and the `commands.entity(` that installs it — rather than on line numbers, so it
/// survives unrelated churn in the file. A statement runs from `commands.entity(` to the first
/// `;` outside any parentheses or string literal, so the whole chained `.insert((..))` tuple is
/// captured — and the `ShadowView` struct definition itself is not.
fn shadow_view_insert_calls(src: &str) -> Vec<&str> {
    src.match_indices("commands.entity(")
        .filter_map(|(start, _)| {
            let mut depth = 0usize;
            let mut in_string = false;
            let mut escaped = false;
            for (offset, ch) in src[start..].char_indices() {
                if in_string {
                    match ch {
                        _ if escaped => escaped = false,
                        '\\' => escaped = true,
                        '"' => in_string = false,
                        _ => {}
                    }
                    continue;
                }
                match ch {
                    '"' => in_string = true,
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ';' if depth == 0 => {
                        let statement = &src[start..start + offset + 1];
                        return statement.contains("ShadowView {").then_some(statement);
                    }
                    _ => {}
                }
            }
            panic!("unterminated commands.entity statement starting at byte {start}");
        })
        .collect()
}

/// Every shadow view — directional cascade, point cubemap face, spot — is built with the light's
/// mask. All three light types are checked: a directional-only fix looks like it works (the sun is
/// directional) and leaves point and spot lights broken.
#[test]
fn every_shadow_view_carries_the_lights_render_layers() {
    let src = linked_light_rs();
    let inserts = shadow_view_insert_calls(&src);
    assert_eq!(
        inserts.len(),
        3,
        "expected exactly 3 shadow view construction sites in the linked bevy_pbr (directional \
         cascade, point cubemap face, spot), found {}. A new site means a new light type whose \
         shadow view also needs the light's RenderLayers — audit it before relaxing this count.",
        inserts.len()
    );
    for insert in &inserts {
        assert!(
            insert.contains("(*light_render_layers).clone()"),
            "a shadow view is being built WITHOUT the light's RenderLayers: `queue_shadows` tests \
             casters against the shadow view's own mask, so the view defaults to layer 0 and drops \
             every caster on any other layer (see \
             upstream/bevy-shadow-view-ignores-light-render-layers.md). Offending site:\n{insert}"
        );
    }
}

/// `queue_shadows` filters casters on the shadow view's own mask, and that mask is REQUIRED on the
/// view (a non-optional `&RenderLayers`), so a maskless view cannot silently fall back to layer 0.
#[test]
fn queue_shadows_filters_on_a_required_view_mask() {
    let src = linked_light_rs();
    assert!(
        src.contains("view_light_entities: Query<(&LightEntity, &ExtractedView, &RenderLayers)>"),
        "`queue_shadows` no longer requires a RenderLayers on every shadow view — re-derive \
         whether a maskless view can fall back to layer 0 (see \
         upstream/bevy-shadow-view-ignores-light-render-layers.md)"
    );
    assert!(
        src.contains("view_light_render_layers.intersects(mesh_layers)"),
        "`queue_shadows` no longer filters casters against the shadow view's mask — re-derive \
         what decides which layers cast"
    );
}

fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                render_creation: WgpuSettings {
                    backends: None,
                    ..default()
                }
                .into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>(),
    );
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    app
}

/// The executable half of the contradiction, and the exact scenario the upstream report repros:
/// a layer-0 camera, two identical casters on layers 1 and 2, and a shadow-casting directional
/// light masked for layers 0 and 1.
///
/// `bevy_light::check_dir_light_mesh_visibility` filters casters against the LIGHT's mask, so the
/// layer-1 caster is accepted for shadow rendering even though the camera cannot see it — that
/// asymmetry is deliberate upstream (a caster outside the camera's layers must still cast). The
/// layer-2 caster is the control: it proves the acceptance is genuinely mask-driven and not a
/// vacuous "everything is visible".
///
/// The shadow views must then agree with this mask (the source-scanned tests above); that second
/// stage cannot be reached without a GPU (see the module header), which is why this test stops
/// here.
#[test]
fn light_visibility_accepts_a_caster_the_camera_cannot_see() {
    let mut app = headless_app();

    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::from(Cuboid::default()));

    // Identical but for the layer, so the layer is the only variable between them.
    let caster_on_layer_1 = app
        .world_mut()
        .spawn((
            Mesh3d(mesh.clone()),
            Transform::from_xyz(0.0, 0.0, -10.0),
            RenderLayers::layer(1),
        ))
        .id();
    let caster_on_layer_2 = app
        .world_mut()
        .spawn((
            Mesh3d(mesh),
            Transform::from_xyz(0.0, 0.0, -10.0),
            RenderLayers::layer(2),
        ))
        .id();
    let light = app
        .world_mut()
        .spawn((
            DirectionalLight {
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_xyz(0.0, 10.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
            // Covers the camera's layer AND the caster's. The light itself must intersect the
            // camera's layers or it would not be view-visible and would cast nothing at all.
            RenderLayers::from_layers(&[0, 1]),
        ))
        .id();
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), Transform::default()))
        .id();

    // Two updates: the first spawns and computes bounds, the second runs visibility over them.
    app.update();
    app.update();

    let world = app.world();

    assert!(
        world
            .entity(light)
            .get::<ViewVisibility>()
            .is_some_and(|visibility| visibility.get()),
        "the light must be view-visible or it clears its own visible-entity lists, which would \
         make the assertions below vacuous",
    );

    let camera_visible = world
        .entity(camera)
        .get::<VisibleEntities>()
        .expect("cameras carry VisibleEntities");
    assert!(
        !camera_visible
            .get(TypeId::of::<Mesh3d>())
            .contains(&caster_on_layer_1),
        "the layer-1 caster must be INVISIBLE to the layer-0 camera — that is what makes the \
         shadow-view mask a distinct question from the camera's mask",
    );

    let cascades = world
        .entity(light)
        .get::<CascadesVisibleEntities>()
        .expect("shadow-casting directional lights carry CascadesVisibleEntities");
    let (mut accepted_layer_1, mut accepted_layer_2) = (false, false);
    for cascades_for_view in cascades.entities.values() {
        for cascade in cascades_for_view {
            accepted_layer_1 |= cascade.entities.contains(&caster_on_layer_1);
            accepted_layer_2 |= cascade.entities.contains(&caster_on_layer_2);
        }
    }

    assert!(
        accepted_layer_1,
        "bevy_light accepted no layer-1 caster for a light masked for layers 0 and 1 — the \
         premise the shadow views rely on (main-world visibility filters against the LIGHT's mask) \
         no longer holds",
    );
    assert!(
        !accepted_layer_2,
        "a caster outside the light's mask was accepted, so the main-world filter is not actually \
         mask-driven and the assertion above proves nothing",
    );
}
