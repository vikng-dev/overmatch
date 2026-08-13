//! UPSTREAM TRIPWIRE for the vendored `bevy_pbr` patch that gives every shadow view the LIGHT's
//! `RenderLayers` mask. Full decode + upstream status:
//! `upstream/bevy-shadow-view-ignores-light-render-layers.md`. Upstream fixed this in
//! bevyengine/bevy #24797 (merged 2026-07-08, milestone 0.19.1) — unreleased, hence the vendor
//! patch and this guard.
//!
//! The defect being patched, in bevy_pbr 0.19 (`src/render/light.rs`): shadow views are spawned
//! with `spawn_empty()` and never receive a `RenderLayers`, yet `queue_shadows` tests every
//! candidate caster against *that view's* `RenderLayers` — absent, so `RenderLayers::default()`,
//! so layer 0. Any mesh on a non-zero layer therefore casts no shadow at all, even from a light
//! whose own mask covers that layer. That directly contradicts the main-world filter in
//! `bevy_light`, which accepted the caster against the *light's* mask one stage earlier;
//! `light_visibility_accepts_a_caster_the_camera_cannot_see` below pins that half of the
//! contradiction as executable fact.
//!
//! **These tests are NOT inverted** (unlike `tests/bevy_ktx2_uastc_fallback.rs`): they assert the
//! patch is present and correct. A failure means the patch was lost or narrowed — most likely by
//! re-vendoring `bevy_pbr` from a fresh registry copy — not that upstream fixed anything. When
//! 0.19.1 does ship, upstream's shape differs from ours (it attaches `RenderLayers` as a component
//! on the extracted light instead of carrying it in the extracted structs), so several of these
//! anchors move at once: that is the signal to drop the vendored patch, not to re-apply it.
//!
//! ## Why the render-world half is source-scanned rather than executed
//!
//! The construction sites live in `prepare_lights`, a render-world system. With
//! `WgpuSettings { backends: None }` bevy 0.19 does not create the `RenderApp` at all (it logs
//! "Render app did not exist when trying to add `extract_resource`"), so no shadow view is ever
//! built and nothing about extraction or `queue_shadows` is observable. Executing that half needs
//! a real wgpu adapter, which CI (`ubuntu-latest`, no GPU, see `.github/workflows/ci.yml`) does not
//! have — a GPU-gated test would silently no-op there, which is worse than none. So the render-world
//! half is pinned by scanning the vendored source for the patch's anchors, and the main-world half
//! — the part that makes the upstream behaviour a contradiction rather than a policy — is executed.

use std::any::TypeId;
use std::path::PathBuf;

use bevy::camera::visibility::{CascadesVisibleEntities, RenderLayers, VisibleEntities};
use bevy::prelude::*;
use bevy::render::settings::WgpuSettings;

/// The patched vendored crate. Wired in via `[patch.crates-io]`, so this really is the `bevy_pbr`
/// the game links.
const VENDORED_LIGHT_RS: &str = "vendor/bevy_pbr-0.19.0-scalar-math/src/render/light.rs";

fn vendored_light_rs() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(VENDORED_LIGHT_RS);
    std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "cannot read the vendored bevy_pbr light module at {}: {err}. If the vendor entry was \
             removed, re-check whether upstream fixed shadow-view render layers before deleting \
             this test (see the module header).",
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

/// THE TRIPWIRE. Every shadow view — directional cascade, point cubemap face, spot — must be given
/// the light's mask, or `queue_shadows` silently drops every caster off layer 0.
///
/// All three light types are checked on purpose: a directional-only patch looks like it works
/// (directional lights are what casts the sun's shadows) and leaves point and spot broken.
#[test]
fn every_shadow_view_carries_the_lights_render_layers() {
    let src = vendored_light_rs();
    let inserts = shadow_view_insert_calls(&src);

    assert_eq!(
        inserts.len(),
        3,
        "expected exactly 3 shadow view construction sites in the vendored bevy_pbr (directional \
         cascade, point cubemap face, spot), found {}. A new site means a new light type whose \
         shadow view also needs the light's RenderLayers — audit it before relaxing this count.",
        inserts.len()
    );

    for insert in &inserts {
        assert!(
            insert.contains("light.render_layers.clone()"),
            "a shadow view is being built WITHOUT the light's RenderLayers. `queue_shadows` tests \
             casters against the shadow view's own mask, so a maskless view defaults to layer 0 \
             and drops every caster on any other layer. Re-apply the `// OVERMATCH PATCH:` edits \
             in {VENDORED_LIGHT_RS} (see \
             upstream/bevy-shadow-view-ignores-light-render-layers.md). Offending site:\n{insert}"
        );
    }
}

/// The point/spot half of the patch: the mask has to reach the render world at all before
/// `prepare_lights` can copy it onto a view. `ExtractedDirectionalLight` carries one upstream;
/// `ExtractedPointLight` (which backs BOTH point and spot lights) did not.
#[test]
fn extracted_point_light_carries_the_mask_from_the_main_world() {
    let src = vendored_light_rs();

    let struct_start = src
        .find("pub struct ExtractedPointLight {")
        .expect("bevy_pbr still defines ExtractedPointLight");
    let struct_end = struct_start
        + src[struct_start..]
            .find("\n}")
            .expect("ExtractedPointLight's definition is brace-terminated");
    assert!(
        src[struct_start..struct_end].contains("pub render_layers: RenderLayers,"),
        "ExtractedPointLight lost its `render_layers` field, so point and spot shadow views have \
         no mask to inherit. Re-apply the `// OVERMATCH PATCH:` edits in {VENDORED_LIGHT_RS}."
    );

    // One per light type. Copying the mask verbatim from the light entity is the whole point:
    // populating it with a constant (`RenderLayers::default()`) would compile and pass the field
    // check above while restoring the bug.
    assert_eq!(
        src.matches("render_layers: maybe_layers.unwrap_or_default().clone(),")
            .count(),
        3,
        "expected the light's own mask to be read into all 3 extracted lights (directional, point, \
         spot) in `extract_lights`. A missing one means that light type's shadow views inherit a \
         constant instead of the light's mask."
    );

    // Point and spot shadow views are CACHED: `prepare_lights` only rebuilds them when the view
    // list is empty or `Changed<ExtractedPointLight>` fires. Without `Changed<RenderLayers>` in the
    // extract filters, changing a light's mask at runtime would never reach its existing views.
    assert_eq!(
        src.matches("Changed<RenderLayers>").count(),
        3,
        "`extract_lights` must re-extract all 3 light types when their RenderLayers change, or a \
         runtime mask change never reaches the cached point/spot shadow views."
    );
}

/// The second filter is what makes the patch load-bearing. If upstream ever deletes it — or fixes
/// the bug by feeding it the light's mask themselves — this is the anchor that moves, and the
/// vendored patch becomes retirable.
#[test]
fn queue_shadows_still_filters_on_the_view_mask() {
    let src = vendored_light_rs();
    assert!(
        src.contains("let view_render_layers = maybe_view_render_layers.unwrap_or_default();"),
        "`queue_shadows` no longer re-filters casters against the shadow view's own RenderLayers. \
         If bevy restructured or removed that filter, the vendored patch is no longer needed as \
         written: re-derive it against the new source, or — if this is bevy 0.19.1+ carrying \
         bevyengine/bevy #24797 — delete the patch, this test, and the vendor entry's section 2 \
         (see upstream/bevy-shadow-view-ignores-light-render-layers.md)."
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
/// Stock 0.19 then drops the accepted layer-1 caster again in `queue_shadows`, because the shadow
/// view it is being tested against has no mask. That second stage cannot be reached without a GPU
/// (see the module header), which is why this test stops here.
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
         premise of the whole patch (main-world visibility filters against the LIGHT's mask) no \
         longer holds and the vendored fix must be re-derived",
    );
    assert!(
        !accepted_layer_2,
        "a caster outside the light's mask was accepted, so the main-world filter is not actually \
         mask-driven and the assertion above proves nothing",
    );
}
