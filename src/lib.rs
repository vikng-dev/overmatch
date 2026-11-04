//! Shared simulation and runtime composition for Overmatch.
//!
//! Product binaries compose this crate as an authoritative server or network client.
//! Direct-simulation sandboxes are analytical tools, not alternate player runtimes. See
//! `.agents/PRODUCT.md` and ADR-0024 for the current product topology.

use avian3d::prelude::{
    PhysicsLayer, PhysicsPlugins, RigidBody, RotationInterpolation, TranslationInterpolation,
};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

mod aim;
/// The embedded armament files, written by `build.rs`: `ROUND_FILES` (`assets/rounds/`) and
/// `GUN_FILES` (`assets/guns/`), each `(file stem, file text)` in byte order of the stem. The round
/// and gun registries parse from these alone; no armament file is read at runtime.
mod armaments_index {
    include!(concat!(env!("OUT_DIR"), "/armaments_index.rs"));
}
/// Resolve `assets/` once for `AssetPlugin` (`net::client`) and bake, keeping their glb reads
/// on the same tree.
mod assets;
/// `extract(glb) → TankGeometry` constructs the sim skeleton; the shadow harness checks
/// equivalence at every view bind (design `sim-view-split-and-tank-bake.md` §8).
mod bake;
/// Shared consumer contract/report for [`verify_asset`] and runtime bake admission:
/// `bin/asset_verify` and startup must enforce the same laws.
pub use bake::{Check, Finding, canon_lists, has_error, render, verify_asset};
mod ballistics;
/// The §13.6 ray fuzzer's bake-scale entry point (`cargo run --bin ballistic_fuzzer`). The gate
/// itself also rides `cargo test` at CI scale — see `ballistics::fuzz`.
pub use ballistics::fuzz::run_ballistic_fuzzer;
#[cfg(feature = "bitprobe")]
mod bitprobe;
#[cfg(feature = "bitprobe")]
pub use bitprobe::run_bitprobe;
mod branding;
mod camera;
/// A collider moved onto another rigid body (a launched turret's) drops its contact pairs, so
/// Avian makes them again naming its new body. Mounted by `damage::plugin`, the only launcher.
mod collider_rehome;
/// Devices → bindings → serializable per-tank `TankCommand`: the multiplayer seam.
/// Sim modules consume commands, never devices.
mod command;
/// Constructions in the game's destruction model, as data: `construction::schema` is the
/// `<id>.construction.ron` format the asset tools write and the game reads. Public for the tools.
pub mod construction;
/// View-only brick/timber surfaces mapped in metres on every face through a shared `ExtendedMaterial`.
/// See the module doc.
mod construction_surface;
/// What the physics solver moved through each contact between a standing structure and a dynamic
/// body, per step (the game's environment destruction): the reading a body's contact load comes
/// from. Mounted by `SimPlugin`.
mod contact_transfer;
/// Fixed-tick wall clock/server stats and `SPIKE_COST_TRACE=<path>` JSONL recorder:
/// FixedUpdate time, `ballistics::integrate_projectiles` share and entity/projectile counts.
/// Measures the machine-gun march; idle at zero cost unless armed by a consumer.
/// Registered on network server/client roots.
mod cost;
/// The controlled tank's crew bar + swap input — a shared piece of the fixed player UI, mounted by
/// both `GamePlugin` and the sandbox (each scoped to the `Controlled` tank).
mod crew_ui;
pub(crate) mod damage;
/// The damage cam: the `PenetrationReport` record, the wire that carries it, and the pen cam — the
/// shipped view of a hit, mounted by both client roots. Its recorder runs in [`SimPlugin`] on the
/// authority.
pub mod damage_cam;
/// Server-simulated pieces at rest (the game's environment destruction): avian's island sleep and
/// the one wake it lacks, the rule every loose piece follows.
mod debris;
mod debug;
/// The controlled tank's standard drive row + F3 diagnostics — one view-only implementation mounted
/// by both the offline and network client roots.
mod drive_hud;
/// Exact integer arithmetic over `f32` polynomials for bake embedding certificates and the
/// corridor collector's parallel test: neither decision uses a tolerance.
mod exact;
/// The FNV-1a fold the compatibility tags are made with: the protocol's and the shot vocabulary's.
mod fingerprint;
/// Fire control: per-weapon superelevation range tables + the player-dialed range. Sits atop
/// `ballistics`; the aim commit reads it to lob the aim point so the bore elevates for range.
mod firecontrol;
/// Which screen a windowed client is on (`Flow`): view state that owns the UI trees, never read by
/// the fixed clock.
mod flow;
mod frame_cost;
/// Runtime ADR-0035 certificate: read `<id>.lod.json`, fingerprint the trio, attach coincident
/// one `VisibilityRange` sibling per certified primitive rung. Belts swap one mesh handle instead
/// (`track::link_view`). Render selection consumes build measurements without Rust transcription.
mod geometry_lod;
/// Which ground type is under each point of the map, sim side: the map's painted blend weights,
/// decoded once at startup, and each pack's ground. See the module doc.
pub(crate) mod ground_field;
/// The ground-type table (`assets/terrain/ground_types.ron`): each kind of ground the terrain packs
/// name: physical traction and motion resistance, and cosmetic prints and dust. See the module doc.
mod ground_types;
/// The gun registry (`assets/guns/`): every gun model and its ammunition table, the muzzle speed it
/// fires each round it chambers at. A [`rounds::Round`] is a round fired from a gun.
mod guns;
/// The dedicated-server guard: boots `SimPlugin` headless (no GPU/window/winit) and drives the
/// tank via `TankCommand` — fails first if sim code grows a hard render dependency.
#[cfg(test)]
mod headless_test;
/// The shared tank-state HUD (world-anchored capability/crew/damage readouts). Mounted by both
/// `GamePlugin` and the sandbox; each reprojects through the camera it declared `view::PlayerView`.
mod hud;
/// Hull marks, the visual layer of the game's damage model: the authority's per-vehicle record of
/// the scratches, dents and holes rounds leave, replicated to every client. The decals that draw
/// it are `vfx::hull_marks`.
mod hull_marks;
/// A data file's table keyed by name, refusing a duplicated key at parse.
mod keyed_map;
/// Capture-plan LOD inspection: certified fine/coarse shoe pairs at each reachable switch range.
mod lod_showcase;
/// Select/parse `assets/maps/<id>/level.json` once: terrain, placements and export coordinates.
/// `terrain_grid` reads at Startup; `scatter` uses the same struct.
mod map;
/// The networking implementation. Executables enter through [`run_client`] and [`run_server`];
/// the adapter tree is private to the library.
mod net;
pub(crate) use net::{env_flag, env_parse, env_text, env_value, refuse_start};
pub use net::{run_client, run_server};
/// Documentation frames to disk, from the capture plan `OVERMATCH_CAPTURE` names; inert without it.
/// Public so the sandbox and demo bins can mount it (module doc).
pub mod dev_capture;
/// Net-client overlay authority: active set and pure input/cursor/scrim rules for connect,
/// death, menu and view-death overlays. At the crate root because `sight` declares view state
/// into it without naming netcode (`src/net/tests/net_boundary.rs`). Mounted only by
/// [`NetClientPlugin`]; single-player uses `state::client_plugin`'s real pause.
mod overlay;
mod physical_laws;
/// OS-level process and window helpers that belong to no game layer.
mod platform;
/// Provenance of authored data values: each authored value names its fact, unit, taste,
/// resolution or stand-in job; an unlabelled value does not load.
mod provenance;
/// macOS `Cmd+Q` → [`AppExit`](bevy::app::AppExit) and render-`SubApp` recall to avoid teardown
/// deadlock against its render thread. Exit-only, no per-frame cost. Mounted once per windowed
/// root ([`ClientPlugin`] / [`NetClientPlugin`]); headless servers have neither render app nor menu.
mod quit;
/// `SPIKE_RENDER_COST=<path>` JSONL of Bevy per-pass `elapsed_cpu` / `elapsed_gpu` diagnostics.
/// Fresh samples only: no rolling averages, so spikes survive and dead passes go silent.
/// Mounts `RenderDiagnosticsPlugin` without needing tracy; covers main/prepass/bloom/tonemapping,
/// not shadows (use tracy). Windowed clients only; zero cost when unset.
/// Analyze with `scripts/render/analyze.py`.
mod render_cost;
/// Owns render-layer numbers and Bevy shadow markers: semantic channels, camera/light profiles
/// and per-object `VisualScope`. Others declare intent; a source scan enforces this ownership.
/// Every windowed root mounts it; sandboxes keep separate layering (ADR-0031).
mod render_policy;
/// Render-scale "Route A": fractional 3D main pass through `MainPassResolutionOverride` and
/// bilinear upscale; bloom, tonemapping and UI stay native. Window camera targeting is unchanged,
/// preserving `world_to_viewport` consumers (`aim`, `sight`, `hud`). The sole resource writer,
/// `settings::apply_settings`, mounts this owner through `settings` on every windowed root.
mod render_scale;
/// The round registry (`assets/rounds/`, one file per round) and the round-type table
/// (`assets/ballistics/round_types.ron`): every round's core material, nose, mass, calibre, crew
/// line rate and burst, resolved and validated at parse; its muzzle speed is the firing gun's
/// ([`guns`]).
mod rounds;
/// How a run shows its primary window (`OVERMATCH_WINDOW=headless|hidden|offscreen|visible`), and
/// what a hidden one implies. See the module doc.
mod run_window;
/// The armor ballistics sandbox (`bin/armor_sandbox`). Public so the binary can mount it; not part
/// of `GamePlugin`.
#[cfg(feature = "dev_ui")]
pub mod sandbox;
/// The map's object scatter (`level.json` resolved through `prototypes.ron`): shared placement
/// data, with stable map identities on both peers. Graybox blocks enter `world::TerrainMap`;
/// `construction::spawn` builds complete construction bodies, and `trees::spawn` builds the
/// segmented trees of the game's environment destruction model.
mod scatter;
/// Copy the player's main-pass depth into Bevy's prepass texture after opaque rendering for
/// soft sprites/hull marks, avoiding a second scene draw. `camera` mounts it and spawns its marker.
mod scene_depth;
/// Persisted graphics (`video.ron` in platform config), sole render-knob reconciler and Esc page.
/// Each windowed root mounts `settings::plugin(PageEntry::…)` once, bringing `render_scale` and
/// the page with its entry declarer. Never mounted on the headless server.
mod settings;
/// Ship-facing view-layer audio: render-only subscribers to the device and `FireShell` seams (the
/// trigger click and the 88's report) plus the RPM-driven engine loop every tank carries. Owns the
/// game's one spatial-falloff law and the listener the camera wears. Mounted by both windowed
/// clients (ADR-0014 — never the server, which has no audio device).
mod sfx;
mod shooting;
/// The SHOT-LIFECYCLE recorder (`SPIKE_SHOT_TRACE=<path>`): an env-gated JSONL log of what happens to
/// each [`ShotId`] on BOTH ends — the authority's fire/keyframe/terminal/damage emissions, and the
/// client's arrivals (with the dedup verdict) plus its marker and cosmetic shell/trail boundaries.
/// Net-neutral (plain `u32` ticks), so `ballistics` writes to it without naming the netcode. Off (zero
/// cost) unless the env var is set. Analyzed by `scripts/shot/analyze.py`.
mod shot_trace;
mod sight;
pub mod sim_tick;
mod spec;
mod state;
/// The global substance registry (`assets/materials/materials.ron`): the material library's numeric
/// half, keyed by the Blender material datablock name each ballistic mesh wears. BOUND: `bake`
/// resolves every primitive of a declared armour node against it at extraction — the lookup that
/// gives it its factor. Which nodes are armour is the tank sheet's declaration, never this lookup.
mod substances;
mod tank;
/// A tank's drawn surface: each material's visual from the shared library, then the vehicle's
/// livery (paint, camouflage), through one `ExtendedMaterial`. View-only. See the module doc.
pub(crate) mod tank_surface;
/// The ground's surface blend: four material packs mixed per fragment by the map's author-painted
/// weight masks. Its CPU weights also select ground facts; rendering uses `ExtendedMaterial`.
pub(crate) mod terrain_blend;
/// PNG → shared height grid for oracle, collider, client mesh and server spawn heights.
/// The module doc owns mapping constants.
pub(crate) mod terrain_grid;
/// View-only RTIN/Martini error-bounded terrain tiles, built at startup from the collider/oracle's
/// decoded grid and selected by `VisibilityRange`. Collider, oracle and `height_at` stay unchanged.
/// See the module doc.
mod terrain_lod;
/// The terrain's sun shadow, baked once from the canonical grid toward the fixed sun and sampled by
/// every lit fragment in place of terrain casters in the cascades. View-only. See the module doc.
mod terrain_shadow;
/// The repository-wide gates and the upstream tripwires on the engine.
#[cfg(test)]
mod tests;
/// `SPIKE_TRACE=<path>` rendered-vs-simulated pose JSONL for MP hull-jitter investigation.
/// Unconditionally compiled, zero cost when unset. Only MP roots register net rows;
/// optional net resources are absent in single-player.
mod trace;
/// Shared sandbox/game track model: route/oracle/wrap math and sim/view adapters.
/// See `.agents/docs/design/track-model/architecture.md`.
pub mod track;
/// Track prints in the ground: cosmetic terrain marks stamped from every grounded shoe of the
/// conformed belt view, stored around the camera and over the map, and drawn by the ground's
/// shader. View only (ADR-0014), client-local, mounted by the track view. See the module doc.
mod track_prints;
/// Track/suspension dev tool (`bin/track_sandbox`), public for the binary and outside `GamePlugin`.
/// Loads the selected tank's real blueprint/marker-derived running gear, then overlays live-tuned
/// cast/route/contact gizmos on a driving hull. Also owns suspension inspection.
#[cfg(feature = "dev_ui")]
pub mod track_sandbox;
/// Trees in the game's damage model: a hard body of segments, each with its own HP pool, and a
/// state, built at spawn from `assets/trees/` data (the glb is the view), dosed by the union walk
/// and felled at the segment whose pool empties.
mod trees;
/// Load both bundled Barlow Condensed weights once into `ui_font::UiFonts` for client `Text` spawners,
/// replacing Bevy's ASCII-only font. Mounted by every windowed root.
mod ui_font;
/// An unattended run (`OVERMATCH_UNATTENDED=1`): silent, and a focus loss never pauses it. Mounted by
/// [`windowed_runtime`], so every windowed root honours it. See the module doc.
mod unattended;
/// Ship-facing view-layer combat VFX: render-only subscribers to the sim's `Impact` and
/// `FireShell` seams (impact puffs, every gun's muzzle flash/light/smoke + shell smoke trail) plus
/// the shared billboard/erosion/gradient-LUT machinery they are built from. Mounted by both
/// windowed clients (ADR-0014 — never the server).
mod vfx;
/// The single 3D camera's field/main-pass pixel height, read by one system on human-rate events,
/// and exact projection (ADR-0033 §9). `terrain_lod` / `geometry_lod` share these facts but own
/// separate pixel-budget tuning.
/// Every windowed root mounts it; headless roots have no view.
mod view;
mod world;

/// Shared `DefaultPlugins`: [`assets::asset_root`] and [`client_render_plugin`].
/// `tests::default_plugins_root` guards this sole declaration and its only entry points,
/// [`windowed_runtime`] / [`gpu_less_default_plugins`], so all readers open the same asset tree
/// and every window passes through run-mode handling.
///
/// `file_path` must be absolute: Bevy's `FileAssetReader::new` joins it onto `BEVY_ASSET_ROOT`,
/// `CARGO_MANIFEST_DIR` or the executable directory; an absolute path bypasses that choice.
/// Resolve against cwd like direct readers; unreadable cwd refuses startup (exit 2).
pub(crate) fn default_plugins() -> bevy::app::PluginGroupBuilder {
    let root = assets::asset_root();
    let root = std::path::absolute(&root).unwrap_or_else(|error| {
        refuse_start(format!(
            "cannot make the asset root {} absolute against the working directory: {error}; \
             set BEVY_ASSET_ROOT to an absolute path",
            root.display()
        ))
    });
    DefaultPlugins
        .set(bevy::asset::AssetPlugin {
            file_path: root.to_string_lossy().into_owned(),
            ..default()
        })
        .set(client_render_plugin())
}

/// Mount [`default_plugins`] with the primary `window`, apply [`run_window`]'s
/// `OVERMATCH_WINDOW` (default visible) before winit creates it, and honour
/// `OVERMATCH_UNATTENDED` through [`unattended`]. Offline/network clients and both sandboxes
/// share this path so hidden/silent runs stay hidden/silent on every windowed root.
///
/// # Failures
///
/// Malformed or headless `OVERMATCH_WINDOW` refuses startup before constructing the window,
/// naming the variable and supported modes.
pub fn windowed_runtime(app: &mut App, window: Window) {
    let mode = run_window::RunWindow::of_run(run_window::RunWindow::Visible)
        .windowed()
        .unwrap_or_else(refuse_start);
    app.add_plugins(default_plugins().set(WindowPlugin {
        primary_window: Some(window),
        ..default()
    }));
    run_window::mount(app, mode);
    app.add_plugins(unattended::plugin);
}

/// Configure [`default_plugins`] for a GPU-less composition root.
///
/// This owns only the shared render-backend, primary-window, and winit-runner edits. Callers keep
/// ownership of their image workarounds, clocks, runners, and physics choices.
pub(crate) fn gpu_less_default_plugins(
    primary_window: Option<Window>,
) -> bevy::app::PluginGroupBuilder {
    default_plugins()
        .set(bevy::render::RenderPlugin {
            render_creation: bevy::render::settings::WgpuSettings {
                backends: None,
                ..default()
            }
            .into(),
            ..default()
        })
        .set(WindowPlugin {
            primary_window,
            exit_condition: bevy::window::ExitCondition::DontExit,
            ..default()
        })
        .disable::<bevy::winit::WinitPlugin>()
}

/// Server/bitprobe floor: [`gpu_less_default_plugins`] minus presentation/device plugins,
/// with `net::server_host::host_task_pools` sizing. The fixture uses the authority's floor.
/// No authority system reads the removed plugins' outputs: UI/text, sprites, picking, focus,
/// accessibility, gamepads, clipboard, audio, animation, gizmos, light clusters/frusta/cascades,
/// or scenes. `world::spawn_environment`'s `DirectionalLight` is unused on the server; only the
/// client's `settings::apply_settings` reads it.
/// `LightPlugin` must go with `GizmoPlugin`: its light-gizmo systems read the gizmo asset store.
/// Keep plugins that touch sim entities: `CameraPlugin` bounds inserts `Aabb` on the world's
/// `Mesh3d`s; that spawn system writes mesh/image/PBR stores. Keep `GltfPlugin` because
/// `PbrPlugin` registers a glTF extension handler at build.
pub(crate) fn server_plugin_floor() -> bevy::app::PluginGroupBuilder {
    use bevy::{
        a11y, animation, audio, clipboard, gilrs, gizmos, gizmos_render, input_focus, light,
        picking, scene, sprite, sprite_render, text, ui, ui_render, ui_widgets,
        world_serialization,
    };
    gpu_less_default_plugins(None)
        .set(net::server_host::host_task_pools())
        .disable::<ui::UiPlugin>()
        .disable::<ui_render::UiRenderPlugin>()
        .disable::<ui_widgets::popover::PopoverPlugin>()
        .disable::<ui_widgets::ButtonPlugin>()
        .disable::<ui_widgets::CheckboxPlugin>()
        .disable::<ui_widgets::ListBoxPlugin>()
        .disable::<ui_widgets::MenuPlugin>()
        .disable::<ui_widgets::RadioGroupPlugin>()
        .disable::<ui_widgets::ScrollAreaPlugin>()
        .disable::<ui_widgets::ScrollbarPlugin>()
        .disable::<ui_widgets::SliderPlugin>()
        .disable::<ui_widgets::EditableTextInputPlugin>()
        .disable::<text::TextPlugin>()
        .disable::<sprite::SpritePlugin>()
        .disable::<sprite_render::SpriteRenderPlugin>()
        .disable::<picking::input::PointerInputPlugin>()
        .disable::<picking::PickingPlugin>()
        .disable::<picking::InteractionPlugin>()
        .disable::<input_focus::InputFocusPlugin>()
        .disable::<input_focus::InputDispatchPlugin>()
        .disable::<a11y::AccessibilityPlugin>()
        .disable::<gilrs::GilrsPlugin>()
        .disable::<clipboard::ClipboardPlugin>()
        .disable::<audio::AudioPlugin>()
        .disable::<animation::AnimationPlugin>()
        .disable::<gizmos::GizmoPlugin>()
        .disable::<gizmos_render::GizmoRenderPlugin>()
        .disable::<light::LightPlugin>()
        .disable::<scene::ScenePlugin>()
        .disable::<world_serialization::WorldSerializationPlugin>()
}

/// Windowed render settings, stock except macOS GPU-query features. [`default_plugins`]
/// mounts it; [`gpu_less_default_plugins`] replaces its backend settings for GPU-less roots.
///
/// MEASURED on M4 / macOS 26.5 / Metal / wgpu 29: Bevy's default
/// `WgpuSettingsPriority::Functionality` copies adapter features then applies `disabled_features`
/// before device creation (vendored bevy_render-0.19.0/src/renderer/mod.rs:298-315).
/// `bevy/trace_tracy` auto-mounts `RenderDiagnosticsPlugin` (vendored
/// bevy_render-0.19.0/src/lib.rs:379-380); device `TIMESTAMP_QUERY` makes `DiagnosticsRecorder`
/// allocate a 256-entry query set (`FrameData::new`, vendored
/// bevy_render-0.19.0/src/diagnostic/internal.rs:236-254). Metal refuses
/// `newCounterSampleBufferWithDescriptor` ("Cannot allocate sample buffer"); wgpu-hal reports
/// `DeviceLost`, and Bevy quits within ~1 s of the first frame. Removing the feature prevents
/// allocation while preserving tracy CPU spans (systems, render-graph nodes, shadow pass).
///
/// Bevy already no-ops macOS encoder timestamps for bevy#22257 Tahoe flicker
/// (`WriteTimestamp for CommandEncoder`, vendored
/// bevy_render-0.19.0/src/diagnostic/internal.rs:744-760); wgpu#9414 reports Metal4 queries return
/// zeros. `PIPELINE_STATISTICS_QUERY` also allocates in `FrameData::new`, and wgpu supports it
/// only on Vulkan/DX12. Windows/Linux retain both features, real `elapsed_gpu` and tracy GPU tracks.
fn client_render_plugin() -> bevy::render::RenderPlugin {
    #[cfg(target_os = "macos")]
    let disabled_features = Some(
        bevy::render::settings::WgpuFeatures::TIMESTAMP_QUERY
            | bevy::render::settings::WgpuFeatures::PIPELINE_STATISTICS_QUERY,
    );
    #[cfg(not(target_os = "macos"))]
    let disabled_features = None;
    bevy::render::RenderPlugin {
        render_creation: bevy::render::settings::WgpuSettings {
            disabled_features,
            ..default()
        }
        .into(),
        ..default()
    }
}

/// Cleanup uses `try_despawn` because another lifetime owner may already have removed an evictee.
pub(crate) fn push_capped_entity(
    commands: &mut Commands,
    ring: &mut std::collections::VecDeque<Entity>,
    entity: Entity,
    cap: usize,
) {
    ring.push_back(entity);
    while ring.len() > cap {
        if let Some(old) = ring.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
}

#[cfg(test)]
mod drive_row_tests {
    use super::*;
    use track::transmission::{DriveReadout, TransmissionState};

    /// Both clients share this row: `P` means park, missing readout hides gear/rpm but retains
    /// horizontal ground speed.
    #[test]
    fn standard_drive_row_format_is_shared_and_compact() {
        fn fields(row: String) -> Vec<String> {
            row.split_whitespace().map(str::to_owned).collect()
        }
        let speed_mps = 12.5; // DERIVED 45 km/h.
        let mut state = TransmissionState {
            gear: 1,
            park: true,
            ..TransmissionState::stopped()
        };
        let mut operating = DriveReadout {
            rpm: 2_600.0,
            gear_label: "F1".to_string(),
        };
        assert_eq!(
            fields(drive_hud::standard_drive_row(
                Some((&state, &operating)),
                speed_mps
            )),
            ["Gear", "F1P", "RPM", "2.6k", "Speed", "45", "km/h"]
        );

        state.park = false;
        state.gear = 8;
        operating.gear_label = "F8".to_string();
        assert_eq!(
            fields(drive_hud::standard_drive_row(
                Some((&state, &operating)),
                speed_mps
            )),
            ["Gear", "F8", "RPM", "2.6k", "Speed", "45", "km/h"]
        );

        state.gear = 2;
        state.reverse = true;
        operating.gear_label = "R2".to_string();
        assert_eq!(
            fields(drive_hud::standard_drive_row(
                Some((&state, &operating)),
                speed_mps
            )),
            ["Gear", "R2", "RPM", "2.6k", "Speed", "45", "km/h"]
        );
        assert_eq!(
            fields(drive_hud::standard_drive_row(None, speed_mps)),
            ["Speed", "45", "km/h"],
            "with no readout the row retains speed and omits the gear and rpm fields"
        );
        assert_eq!(
            drive_hud::horizontal_ground_speed(Vec3::new(3.0, 99.0, 4.0)),
            5.0,
            "vertical velocity is excluded"
        );
    }
}

#[cfg(test)]
mod easing_tests {
    use super::*;
    use avian3d::prelude::{
        AngularInertia, AngularVelocity, Gravity, LinearVelocity, Mass, Position, Rotation,
    };
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// Offline moving bodies must render translation/yaw between the last two fixed poses,
    /// behind the latest. The scenario's tanks and launched turret rely on this smoothness;
    /// removing the interpolation plugin or either [`eased_dynamic_body`] component must fail.
    #[test]
    fn a_moving_eased_body_is_drawn_behind_its_latest_fixed_pose() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`.
            AssetPlugin::default(),
            offline_physics(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity::ZERO)
        // 10 ms frames against the 64 Hz fixed step: frames end between fixed steps.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            10,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        let body = app
            .world_mut()
            .spawn((
                Transform::default(),
                eased_dynamic_body(),
                Mass(1.0),
                AngularInertia::from_shape(&Cuboid::new(1.0, 1.0, 1.0), 1.0),
                LinearVelocity(Vec3::X * 10.0),
                AngularVelocity(Vec3::Y),
            ))
            .id();
        for _ in 0..20 {
            app.update();
        }
        for _ in 0..16 {
            app.update();
            let world = app.world();
            let drawn = world.get::<Transform>(body).unwrap();
            let (position, rotation) = (
                world.get::<Position>(body).unwrap().0,
                world.get::<Rotation>(body).unwrap().0,
            );
            // One fixed step's travel at 10 m/s and 64 Hz, plus rounding.
            let lag = position.x - drawn.translation.x;
            assert!(
                lag > 0.0 && lag <= 10.0 / 64.0 + 1e-4,
                "the drawn pose must lie within one step behind the fixed pose (lag {lag} m)"
            );
            assert!(
                drawn.rotation.to_euler(EulerRot::YXZ).0 < rotation.to_euler(EulerRot::YXZ).0,
                "the drawn yaw must lag the fixed pose's"
            );
        }
    }
}

/// Marks a network-client replica. Ballistics uses it to suppress authority-only damage and impulse
/// writes while retaining cosmetic flight and impacts.
#[derive(Resource, Default)]
pub(crate) struct ClientReplica;

/// Net-neutral current tick published before gameplay, by the network compositions and
/// single-player. Local fire uses it to construct a [`ShotId`] before shell spawn; replica
/// ballistics ages sanctioned outcomes and stamps lifecycle rows against it; sandbox shells, with no
/// clock, are unkeyed.
#[derive(Resource, Default)]
pub(crate) struct ShotClock(pub sim_tick::SimTick);

/// Current simulation tick used by the tick-correlated weapon gate. Network compositions publish
/// `LocalTimeline` into it before gameplay; non-network compositions
/// advance it once after each playing fixed tick, wrapping freely. Deadline ordering belongs
/// to [`sim_tick::SimTick`], never the numeric order of this clock's counter.
#[derive(Resource, Default)]
pub(crate) struct WeaponClock(pub sim_tick::SimTick);

/// The composition publishes the weapon clock; the local fallback must not advance it.
/// Network compositions install this ownership marker alongside their timeline publisher.
#[derive(Resource)]
pub(crate) struct PublishedWeaponClock;

/// Physics collision layers. View/aim queries that want the ground (camera terrain ray, sight
/// probes) filter to `Terrain` only, so they ignore vehicle colliders. Shared infra, hence at
/// the crate root.
#[derive(PhysicsLayer, Default, Clone, Copy, Debug)]
pub(crate) enum Layer {
    #[default]
    Default,
    Terrain,
    Vehicle,
    /// Ballistic volumes (armor plates + modules): what the penetration march raycasts against,
    /// distinct from `Vehicle` (the dynamic collision proxy). "Same geometry, two layers" (ADR-0008).
    Armor,
    /// Witness volumes (crew, ADR-0040): never cast against, never in the armour walk; read only
    /// by the witness pass (`ballistics::wound`).
    Witness,
    /// Server-simulated pieces (`crate::debris`): a construction's fallen chunks and the debris stress
    /// lever's fragments. They collide with `Terrain`, `Vehicle` and each other, and no ground
    /// query reads them. A construction piece's separate occupied-hull sensors carry `Armor`
    /// and ballistic volumes, so the game's damage-model walk meets its substance.
    Debris,
    /// Track side faces: physical contacts with bodies and structures, excluded from terrain.
    TrackSide,
}

/// A non-zero, match-local identity assigned synchronously when a combatant spawns.
///
/// Entity ids are an ECS implementation detail: a respawn receives a new entity and every client
/// maps that entity independently. This value stays with the player or bot across respawn, making
/// delayed outcomes addressable without depending on either lifetime or mapping.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(crate) struct CombatantId(pub(crate) u64);

/// Which of its combatant's lives a tank root is: 0 its first spawn, one more each respawn. With the
/// root's [`CombatantId`] it names one life, so an outcome about a tank that has since respawned is
/// never read against its next life. Assigned at spawn, never changed.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(crate) struct CombatantLife(pub(crate) u32);

/// A combatant that fires rounds without a tank: its entity is the [`ShotSource::tank`] its rounds
/// name, and each of its weapons a [`tank::Weapon`] whose [`tank::TankRoot`] names it. It carries
/// its own [`CombatantId`] and [`CombatantLife`]; the pen cam treats it as the player's combatant,
/// a shooter that no round knocks out.
///
/// [`ShotSource::tank`]: ballistics::ShotSource::tank
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct FreeShooter;

/// Canonical, net-neutral identity for one shot: `(combatant, weapon, fire_tick)`.
///
/// Invariant: `fire_tick` distinguishes successive rounds from one weapon, while `combatant` is
/// stable plain data rather than an entity mapping. Every shot-scoped wire and cosmetic outcome keys
/// on this triple, so it remains usable across client mappings and a firing tank's despawn.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(crate) struct ShotId {
    pub(crate) combatant: CombatantId,
    pub(crate) weapon: u8,
    pub(crate) fire_tick: u32,
}

/// Authoritative fixed-tick rules consuming `TankCommand`s, never devices. Network clients
/// also mount them for derived shell/servo/recoil cosmetics. The dedicated server adds netcode
/// on [`server_plugin_floor`]; offline [`GamePlugin`] pairs them with [`ClientPlugin`].
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        // Composition roots own physics: offline roots opt bodies into [`offline_physics`]
        // easing; network roots disable Avian interpolation for `LightyearAvianPlugin`.
        app.add_plugins((
            state::sim_plugin,
            world::plugin,
            // Shared oracle built from `TerrainMap` for sim forces and client track views
            // on server, offline and net client.
            track::terrain_plugin,
            // Register `.tank.ron` loading before `tank` requests it (ADR-0010).
            spec::plugin,
            // The trio's certificate, and the fingerprint of the ONE artifact both ends walk
            // (`<id>.sim.glb`). Mounted before `bake`, which opens that artifact.
            geometry_lod::sim_plugin,
            bake::plugin,
            tank::sim_plugin,
            // Put commands on every tank and consume latched edges each tick.
            command::core_plugin,
            // Belt-force locomotion (ADR-0025).
            track::sim_plugin,
            aim::sim_plugin,
            // `ballistics` owns the shell trajectory + impact seam; `shooting` is the gun control
            // that drives it (the sandbox drives the same `FireShell` from its camera). SIM half
            // only: the shell scene, the tracer streak and shell visibility are
            // `ballistics::view_plugin`, which the client roots mount and the server never does.
            ballistics::sim_plugin,
            damage::plugin,
            trees::sim_plugin,
            shooting::plugin,
            // The pen cam's records of the authority's main-gun hits, inert on a net client, and
            // the vocabulary both ends explain them in.
            (damage_cam::recorder::plugin, damage_cam::delivery::plugin),
        ));
        // Constructions breaking (the game's environment destruction): the authority's laws and
        // record, every peer's colliders from it, the rule loose pieces rest by, and the solver's
        // reading of every structure contact. Separate call: the tuple above is at bevy's
        // 15-plugin arity limit.
        app.add_plugins((
            construction::breakage::sim_plugin,
            debris::sim_plugin,
            contact_transfer::sim_plugin,
        ));
    }
}

/// Gate [`state::PlayerInputSet`] on `state::cursor_locked` in its three schedules:
/// Update aim/view toggle, PostUpdate free-look and RunFixedMainLoop gunner/range/drive gather.
/// Both windowed clients share this `grab_mode == Locked` rule; server/scripted harness mount
/// neither client root and remain unaffected.
fn gate_player_input(app: &mut App) {
    use bevy::ecs::schedule::ScheduleLabel;
    use state::{PlayerInputSet, cursor_locked};
    for schedule in [
        Update.intern(),
        PostUpdate.intern(),
        RunFixedMainLoop.intern(),
    ] {
        app.configure_sets(schedule, PlayerInputSet.run_if(cursor_locked));
    }
}

/// Offline devices → `TankCommand` and state → screen. Requires [`SimPlugin`] in the same app;
/// network presentation uses [`NetClientPlugin`].
pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        gate_player_input(app);
        // The screen state, derived from `AppState` (`Boot` while loading, then `InMatch`), mounted
        // before every plugin that spawns or gates on it.
        app.add_plugins(flow::offline_plugin);
        app.add_plugins((
            // Load the bundled UI font first: it inserts `UiFonts` at build time, so the HUD/crew
            // spawn systems below always find it (see `ui_font`).
            ui_font::plugin,
            branding::plugin,
            // Shared windowed exit handling; see `quit`.
            quit::plugin,
            // Pause/cursor handling (drives the states that `state::sim_plugin` owns).
            state::client_plugin,
            command::client_plugin,
            tank::client_plugin,
            // Mount before consumer declarations for readable dependency order; actual PostUpdate
            // resolution is order-independent.
            render_policy::plugin,
            camera::plugin,
            aim::client_plugin,
            // `sight` owns the gunner-view toggle/mode that `camera` and `aim` branch on.
            sight::plugin,
            // The player's range dial (rides to the sim inside the command).
            firecontrol::client_plugin,
            // The tank-state HUD and the controlled tank's crew bar + `1`–`5` swap input.
            hud::plugin,
            crew_ui::plugin,
            // Impact dust puffs — every landed round reads at the target (view-only, ADR-0014).
            vfx::plugin,
            // Live tracks: the kinematic-wrap belt + wheel/sprocket animation on the presented
            // pose (view-only, ADR-0014 — the server never mounts this).
            track::view_plugin,
        ));
        // The live view, the render half of the certificate (the view artifact's fingerprint, the
        // chain resolution and the two range writers), and the render half of ballistics (the shell
        // glb, the tracer streak, and the visibility a hold draws with — never mounted by
        // `SimPlugin`, which births a bare shell). Separate call — the tuple above is at bevy's
        // 15-plugin arity limit.
        //
        // `view` FIRST and always beside it: both LOD ladders select through the facts it owns, and
        // a windowed root that mounts a ladder without it has a terrain layer that never reselects.
        app.add_plugins((
            view::plugin,
            geometry_lod::view_plugin,
            // Each tank's surface: its materials' visuals under the livery (view-only, ADR-0014).
            tank_surface::plugin,
            ballistics::view_plugin,
            // The trees' glb binding and fall presentation (view-only, ADR-0014).
            trees::view::plugin,
            // The broken constructions and their fallen pieces (view-only, ADR-0014).
            construction::view::plugin,
        ));
        app.add_plugins(drive_hud::plugin);
        // The pen cam's shooter corner over the player's own hits (view-only; the store it reads
        // is mounted by `GamePlugin`).
        app.add_plugins(damage_cam::pen_cam::game::plugin);
        // View-layer audio (view-only, ADR-0014 — the server never mounts this): the trigger click,
        // each weapon's authored report on the fire seam, and the per-tank engine loop. An
        // unattended run (`windowed_runtime`) silences it.
        app.add_plugins(sfx::plugin);
        // Offline frame-budget sweeps need no server; both client roots mount this recorder.
        // Idle unless `SPIKE_FRAME_COST` is set.
        app.add_plugins(frame_cost::client_plugin);
        // `scripts/perf/run-fire-capture.sh` requests both recorders without a server.
        // Keep the render half mounted too (`SPIKE_RENDER_COST`, idle when unset).
        app.add_plugins(render_cost::client_plugin);
        // Offline Esc uses `AppState::Paused`, not overlay authority. `PageEntry` requires
        // the page's visibility declarer, making a mount without one unrepresentable.
        app.add_plugins(settings::plugin(settings::PageEntry::PauseState));

        // Physics visualization (collider/ray wireframes) + debug toggles, in every build.
        // Offline SP registers no diagnostic plugins by default, which is accepted: the settings page
        // covers the render knobs, `cargo tracy` is the frame-cost instrument, and the net client
        // still has `net::debug_hud`'s fps/frame-time card.
        app.add_plugins((avian3d::prelude::PhysicsDebugPlugin, debug::plugin));
    }
}

/// Network presentation/device gather. Excludes `state::client_plugin`: online menus must not
/// pause local clocks while the server keeps ticking, so the net overlay owns cursor release.
/// Excludes `tank::client_plugin`: Tab possession swap is an offline scenario tool;
/// server `ControlledBy` assigns network possession.
pub struct NetClientPlugin;

impl Plugin for NetClientPlugin {
    fn build(&self, app: &mut App) {
        gate_player_input(app);
        // The screen state, mounted before every plugin that spawns or gates on it; its writer is
        // `net::client`'s, which owns the link facts it derives from.
        app.add_plugins(flow::plugin);
        app.add_plugins((
            // Build-time `UiFonts` must precede UI plugins, as in `ClientPlugin`.
            ui_font::plugin,
            branding::plugin,
            command::client_plugin,
            camera::plugin,
            aim::client_plugin,
            sight::plugin,
            firecontrol::client_plugin,
            hud::plugin,
            crew_ui::plugin,
            // One cursor system; connect/death/sight owners declare presence into the shared
            // overlay priority/z-order and input/scrim rules.
            overlay::plugin,
            // Bottom-right ping/FPS/frame-time debug panel — net-client only (ping is meaningless
            // in SP), for testing against the deployed server.
            net::debug_hud_plugin,
            // The death screen + respawn key — net-client only (SP has no respawn flow): shows
            // "YOU DIED" when the player's own tank is knocked out and latches the respawn edge.
            net::death_screen_plugin,
            // View-layer combat feedback (net-client only): the damage flash when the player is
            // hit, and the hit-marker when the player's shell drops an opponent's health.
            net::hit_feel_plugin,
            // Impact dust puffs — every landed round reads at the target (view-only, ADR-0014; the
            // replica's cosmetic shells spark the same `Impact` seam, so remote fire puffs too).
            vfx::plugin,
            // Live tracks on the presented pose — own AND remote tanks (one code path).
            track::view_plugin,
        ));
        // Both LOD ladders require `view`; see `ClientPlugin`.
        app.add_plugins((
            view::plugin,
            geometry_lod::view_plugin,
            tank_surface::plugin,
        ));
        app.add_plugins((
            // The render half of ballistics (see `ClientPlugin`): the shell glb, the tracer streak
            // and hold visibility. `SimPlugin` mounts only the sim half.
            ballistics::view_plugin,
            // The trees' glb binding and fall presentation (view-only, ADR-0014).
            trees::view::plugin,
            // The broken constructions and their fallen pieces (view-only, ADR-0014).
            construction::view::plugin,
            drive_hud::plugin,
            // Required by every scope/profile declarer; see `render_policy`.
            render_policy::plugin,
            // Shared windowed exit handling; see `quit`.
            quit::plugin,
            // The `M` spawn map — net-client only: a top view of the terrain whose click asks the
            // authority to place this player's NEXT respawn there (nothing teleports now).
            net::spawn_map_plugin,
            // The Battle's match UI — net-client only: the time left, the scoreboard held on Tab
            // and the results, read from the replicated Battle and score.
            net::battle_hud_plugin,
            // The page is `Overlay::Menu` content; the overlay owns input/cursor/scrim.
            // Required `PageEntry` names its visibility declarer.
            settings::plugin(settings::PageEntry::OverlayMenu),
        ));
        // View-layer audio (see `ClientPlugin`): the trigger click, each weapon's authored report,
        // and the engine loop on own AND remote tanks — every root carries the public `EngineRpm`,
        // so one code path. An unattended run (`windowed_runtime`) silences it.
        app.add_plugins(sfx::plugin);
        // The pen cam's shooter corner over the player's own hits (view-only; the store it reads
        // is the link's, `net::shot_explanation`).
        app.add_plugins(damage_cam::pen_cam::game::plugin);

        // The game's damage-model debug view, the same pair ClientPlugin mounts offline:
        // G shows force arrows and collider wireframes. It reads TrackContacts/GlobalTransform
        // and writes only render-side gizmos; the headless server mounts SimPlugin alone.
        app.add_plugins((avian3d::prelude::PhysicsDebugPlugin, debug::plugin));
    }
}

/// Physics for offline game/sandboxes: default interpolation eases only opted-in
/// [`eased_dynamic_body`] bodies. Bodies without easing markers take no easing writes.
/// Rejected `PhysicsInterpolationPlugin::interpolate_all()` rewrites static-body transforms
/// each fixed step/frame, re-propagating hierarchies and re-extracting hidden meshes
/// (`.agents/scratch/perf-2026-10-29/bookkeeping-diagnosis.md`).
pub(crate) fn offline_physics() -> bevy::app::PluginGroupBuilder {
    PhysicsPlugins::default().build()
}

/// Every moving offline body opts translation/rotation into [`offline_physics`] easing in the
/// insertion that makes it dynamic. Inert on network roots, which mount no easing plugin.
pub(crate) fn eased_dynamic_body() -> (RigidBody, TranslationInterpolation, RotationInterpolation) {
    (
        RigidBody::Dynamic,
        TranslationInterpolation,
        RotationInterpolation,
    )
}

/// Every gameplay feature, composed — the single-player configuration: the full sim plus the
/// local client, one app, no netcode. Add to an `App` that already has the runtime plugins.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // The single-player physics choice. The networked bins mount lightyear's config instead.
            offline_physics(),
            SimPlugin,
            // The single-player scenario: two-tank duel spawn, first tank controlled.
            tank::sp_spawn_plugin,
            // `LodShowcase` capture selection enables shoe clamps/camera aim and makes the
            // scenario above spawn pairs instead of the duel. Otherwise installs no systems.
            lod_showcase::plugin,
            ClientPlugin,
            // The shot clock, and each pen cam record explained to the player without a hop.
            damage_cam::delivery::offline_plugin,
            // The local sim is the authority: its strikes mark the hulls directly.
            hull_marks::offline_plugin,
            // Passive jitter-trace recorder (frame + tick rows; no net extras in this build). Idle
            // unless `SPIKE_TRACE` is set.
            trace::sp_plugin,
        ));
    }
}

/// `overmatch --offline`: [`run_client`](net::run_client)'s windowed runtime (exe-relative
/// asset root, continuous winit updates) plus [`GamePlugin`]. No Lightyear/connection entities
/// or path to connect.
pub fn run_offline() {
    let mut app = App::new();
    // Read the player's settings BEFORE the window is described — see `load_at_boot`, which also
    // inserts the values and the boot report into the app.
    let settings = settings::load_at_boot(&mut app);
    windowed_runtime(
        &mut app,
        Window {
            title: "Overmatch - offline".into(),
            // `Unprobed` on purpose: the capability probe needs a surface, which needs this
            // window — the mapping self-negotiates until the probe lands.
            present_mode: settings.present_mode(settings::PresentCaps::Unprobed),
            // Apply persisted fullscreen at creation to avoid a window flash. Use
            // `at_window_creation`, not the stored display rung: Bevy caches an unresolved
            // indexed display and prevents Startup correction (see `settings::DisplaySelection`).
            // Hidden runs stay windowed (`run_window`).
            mode: settings
                .window_mode
                .to_window_mode(settings.display.at_window_creation()),
            ..default()
        },
    );
    // Same policy as the net client: never drop below the 64 Hz tick when unfocused.
    app.insert_resource(bevy::winit::WinitSettings::continuous());
    app.add_plugins(GamePlugin);
    // Documentation frames; inert without `OVERMATCH_CAPTURE`. The tree-fall cue writes the
    // authority's tree state and the impact cue raises the impact seam with no shot behind it, so
    // this root, the only one with no replica, alone mounts them.
    app.add_plugins((
        dev_capture::plugin,
        dev_capture::offline_fell_plugin,
        dev_capture::offline_impact_plugin,
        dev_capture::offline_shot_plugin,
        dev_capture::offline_wall_plugin,
        dev_capture::offline_repair_plugin,
    ));
    // The scripted-trigger capture hook (`SPIKE_AUTO_FIRE`), mounted HERE and nowhere else: this
    // offline root has no netcode at all — no lightyear plugins, no connection entity — so the
    // commands it writes cannot leave the process. Mounting it inside the shared device-gather
    // plugin would put it in the packaged network client too, where scripted trigger edges would
    // ride the input bridge to a live server. See `command::offline_auto_fire_plugin`.
    app.add_plugins(command::offline_auto_fire_plugin);
    // The same boundary: the parked target firing at the player (`OVERMATCH_TARGET=fires` or
    // `kills`) and the scripted x-ray preference (`OVERMATCH_XRAY`), for captures of the player's
    // own tank struck.
    app.add_plugins((
        tank::scenario::offline_target_fires_plugin,
        damage_cam::pen_cam::game::scripted_xray_plugin,
    ));
    // The same boundary: the battle scene's driver (`OVERMATCH_BATTLE`) writes every tank's
    // command, the controlled tank's included.
    app.add_plugins(tank::battle::offline_plugin);
    // A capture plan's `scene.demo`: the game's cosmetic hull-mark demo fabricates marks and takes
    // the camera, the cosmetic track-print demo drives the player's tank and takes the camera.
    // Inert unless the plan names one.
    app.add_plugins((
        hull_marks::demo::offline_plugin,
        track_prints::demo::offline_plugin,
    ));
    // The ground-type tuning panel, shown while paused; egui compiles only into `dev_ui` builds.
    #[cfg(feature = "dev_ui")]
    app.add_plugins(track_prints::tuning::offline_plugin);
    app.run();
}
