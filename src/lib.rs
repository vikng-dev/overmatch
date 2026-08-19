//! Shared simulation and runtime composition for Overmatch.
//!
//! Product binaries compose this crate as an authoritative server or network client.
//! Direct-simulation sandboxes are analytical tools, not alternate player runtimes. See
//! `.agents/PRODUCT.md` and ADR-0024 for the current product topology.

// Two clippy lints fight Bevy's ECS paradigm and are allowed crate-wide (as Bevy's own codebase
// does): `type_complexity` fires on ordinary multi-component query tuples, and `too_many_arguments`
// on systems that legitimately need many params. We de-duplicate the genuinely-repeated query shapes
// behind named `QueryData`/`SystemParam` (e.g. `damage::VolumeFacets`, `damage::ControlledTank`);
// what remains is irreducible ECS shape, not a smell.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

use avian3d::prelude::{PhysicsInterpolationPlugin, PhysicsLayer, PhysicsPlugins};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

mod aim;
/// The runtime asset-root resolver (`asset_root`) — where `assets/` lives, resolved once and shared
/// by both `AssetPlugin` (`net::client`) and the tank bake (`bake`) so they never open different
/// `.glb` files. Always compiled: `bake` builds under `--no-default-features`, where `net` is off.
mod assets;
/// The tank-geometry extractor + shadow harness (sim/view split — design
/// `sim-view-split-and-tank-bake.md` §8). `extract(glb) → TankGeometry` IS the sim skeleton's
/// construction source; the shadow harness keeps proving it
/// equivalent to the instantiated scene on every view bind.
mod bake;
/// The consumer contract, and the one report shape it answers in. `bin/asset_verify` is a thin
/// adapter over [`verify_asset`]; the runtime bake calls the same implementation at startup, so a
/// law cannot hold at one door and not the other.
pub use bake::{Check, Finding, canon_lists, has_error, render, verify_asset};
mod ballistics;
/// The §13.6 ray fuzzer's bake-scale entry point (`cargo run --bin ballistic_fuzzer`). The gate
/// itself also rides `cargo test` at CI scale — see `ballistics::fuzz`.
#[cfg(any(feature = "dev_tools", test))]
pub use ballistics::fuzz::run_ballistic_fuzzer;
#[cfg(feature = "bitprobe")]
mod bitprobe;
#[cfg(feature = "bitprobe")]
pub use bitprobe::run_bitprobe;
mod branding;
mod camera;
/// The command layer: device reads → player bindings → per-tank serializable `TankCommand`. The
/// seam authoritative multiplayer hangs off; sim modules consume commands, never devices.
mod command;
/// The per-fixed-tick sim-COST recorder (`SPIKE_COST_TRACE=<path>`): an env-gated JSONL log of
/// FixedUpdate tick time, the `ballistics::integrate_projectiles` share of it, and entity/projectile
/// counts — the reusable measurement rig for the machine-gun-march cost spike. Off (zero cost) unless
/// the env var is set; registered on the net server and client composition roots.
mod cost;
/// The controlled tank's crew bar + swap input — a shared piece of the fixed player UI, mounted by
/// both `GamePlugin` and the sandbox (each scoped to the `Controlled` tank).
mod crew_ui;
pub(crate) mod damage;
#[cfg(feature = "dev_tools")]
mod debug;
/// The controlled tank's standard drive row + F3 diagnostics — one view-only implementation mounted
/// by both the offline and network client roots.
mod drive_hud;
/// Exact integer arithmetic over `f32`-sourced polynomials — the arithmetic the bake's embedding
/// certificate and the corridor collector's parallel test are decided in, so neither has a
/// tolerance.
mod exact;
/// Fire control: per-weapon superelevation range tables + the player-dialed range. Sits atop
/// `ballistics`; the aim commit reads it to lob the aim point so the bore elevates for range.
mod firecontrol;
mod frame_cost;
/// The tank build's certificate applied at runtime (ADR-0035): `<id>.lod.json` read as data, the
/// trio fingerprinted, and one coincident `VisibilityRange` sibling per certified rung on every
/// scene primitive (the track's pooled shoes swap a mesh handle instead — `track::link_view`). The
/// single seam between what the build measured and what the renderer selects — no measurement is
/// transcribed into Rust.
mod geometry_lod;
/// The dedicated-server guard: boots `SimPlugin` headless (no GPU/window/winit) and drives the
/// tank via `TankCommand` — fails first if sim code grows a hard render dependency.
#[cfg(test)]
mod headless_test;
/// The shared tank-state HUD (world-anchored capability/crew/damage readouts). Mounted by both
/// `GamePlugin` and the sandbox; each tags its own world camera with `hud::HudCamera`.
mod hud;
/// `OVERMATCH_LOD_SHOWCASE=1`: a flat map, the player at one edge, and a clamped PAIR of Tigers at
/// every switch distance in the shoe LOD chain — the two meshes the pipeline's rendered-difference
/// gate compared, side by side, at the range it compared them. A dev eyeball harness for a gate
/// verdict (the L2→L3 switch scores 1.674 against an 0.5 allowance); adds nothing to a process that
/// does not set the variable.
mod lod_showcase;
/// The map manifest (`assets/maps/<id>/level.json`): map selection, the ONE parse of the file that
/// declares a map's terrain block, its object placement and the coordinate conventions it was
/// exported in. `terrain_grid` reads it at Startup; `scatter` places out of the same struct.
mod map;
/// The networking implementation. Executables enter through [`run_client`] and [`run_server`];
/// the adapter tree is private to the library.
mod net;
pub(crate) use net::{env_flag, env_parse, env_value};
pub use net::{run_client, run_server};
/// The net client's single overlay authority (active-set resource + pure input/cursor/scrim rules for
/// the connect / death / menu / view-death overlays). Lives at the crate root, NOT under `net`,
/// because it is pure view-state that the always-sim `sight` module also declares into — putting it
/// here keeps `sight` from naming `crate::net` (the `tests/net_boundary.rs` guard). Mounted only by
/// [`NetClientPlugin`]; single-player has `state::client_plugin`'s real pause instead.
mod overlay;
/// Shutting down: the macOS `Cmd+Q` route into [`AppExit`](bevy::app::AppExit), and the recall of
/// bevy's render `SubApp` that keeps its exit teardown from deadlocking against its own render
/// thread. Both are pure exit-path plumbing with no per-frame cost; mounted once per windowed root
/// ([`ClientPlugin`] and [`NetClientPlugin`] — the headless server has no render app and no menu to
/// be quit from).
mod quit;
/// The RENDER-PASS cost recorder (`SPIKE_RENDER_COST=<path>`): an env-gated JSONL log of bevy's
/// built-in per-pass `elapsed_cpu`/`elapsed_gpu` diagnostics — fresh raw measurements per sample
/// window only, never rolling averages, so spikes survive and dead passes go silent. Mounts
/// `RenderDiagnosticsPlugin` itself, so it works without a tracy build; covers the main
/// passes/prepass/bloom/tonemapping span sites but NOT the shadow pass (tracy is the shadow
/// instrument). Off (zero cost) unless the env var is set; windowed clients only. Analyzed by
/// `scripts/render/analyze.py`.
mod render_cost;
/// The ONE module that knows a render-layer number or writes a bevy shadow marker: semantic
/// channels, camera/light profiles, and the per-object `VisualScope` that resolves into both. Every
/// other module declares intent and never touches `RenderLayers` — a source scan in that module
/// enforces it. Mounted by each windowed root; the sandboxes deliberately keep their own layering
/// (ADR-0031).
mod render_policy;
/// Render scale (research brief "Route A"): the 3D main pass renders at a fraction of the window
/// through `MainPassResolutionOverride` and one bilinear upscale node, while bloom, tonemapping and
/// the whole UI stay native. Ships — NOT `dev_tools`-gated; `settings::apply_settings` is the one
/// writer of its resource. Render-app blast radius only: the camera still targets the window, so
/// every `world_to_viewport` consumer (`aim`, `sight`, `hud`) is untouched. Mounted by `settings`
/// itself (the one writer of its resource mounts the resource's owner), so it reaches every windowed
/// root and no other.
mod render_scale;
/// The armor ballistics sandbox (`bin/armor_sandbox`). Public so the binary can mount it; not part
/// of `GamePlugin`.
pub mod sandbox;
/// Player-facing graphics settings: the persisted model (`video.ron` in the platform config
/// directory), the ONE reconciler that applies it to the rendering world, and the `Esc` settings
/// page. Ships — NOT `dev_tools`-gated, and the only writer of the renderer's knobs. ONE mount per
/// windowed root — `settings::plugin(PageEntry::…)` pulls in `render_scale`, the page and the page's
/// entry declarer — and never the headless server.
/// The map's object scatter (the map's own `level.json`): authored building and tree placement
/// spawned as graybox proxies with static colliders, from shared data on both binaries rather than
/// over the wire. Buildings join `world::TerrainMap`'s block list; firs carry a trunk collider only.
mod scatter;
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
mod spec;
mod state;
/// The global substance registry (`assets/materials/materials.ron`): the material library's numeric
/// half, keyed by the Blender material datablock name each ballistic mesh wears. BOUND: `bake`
/// resolves every glTF primitive against it at extraction — the lookup that both classifies a
/// primitive as armour at all (§12 membership) and gives it its factor.
mod substances;
mod tank;
/// The ground's surface blend: four material packs mixed per fragment by the map's author-painted
/// weight masks, through one `ExtendedMaterial`. View-only. See the module doc.
pub(crate) mod terrain_blend;
/// The world heightmap: PNG → shared height grid (oracle ground term, heightfield collider,
/// client render mesh, server spawn heights). See the module doc for the mapping constants.
pub(crate) mod terrain_grid;
/// The terrain render LOD ladder: RTIN/Martini error-bounded levels per render tile, generated at
/// startup from the SAME decoded grid the collider and oracle read, selected by `VisibilityRange`.
/// View-only — the oracle, the collider and `height_at` are untouched. See the module doc.
mod terrain_lod;
/// The jitter-trace recorder (`SPIKE_TRACE=<path>`): an env-gated JSONL log of rendered vs.
/// simulated pose — passive instrumentation for the MP hull-jitter investigation. Off (zero cost) unless the env var is set. Everything compiles unconditionally;
/// net-specific rows exist only where the MP client/server plugin registers them, and the net
/// extras read their resources through `Option`, so they are absent in single-player.
mod trace;
/// The track model's pure core (route/oracle/wrap math) — consumed by the sandbox lab and, in
/// phase A, the game's track view. See `.agents/docs/design/track-model/architecture.md`.
pub mod track;
/// The track-model sandbox (`bin/track_sandbox`) — the single dev tool for the track/suspension
/// work, having absorbed the retired suspension editor. Public so the binary can mount it; not part
/// of `GamePlugin`. Tiger-only: it loads the real blueprint (geometry + spec), derives the running
/// gear from the glb markers, and overlays the derived suspension/track geometry (cast shapes,
/// route, contact) as live-tweakable gizmo layers on a hull that actually drives.
pub mod track_sandbox;
/// The bundled UI typeface (Barlow Condensed): loads the two weights once and exposes them as a
/// `ui_font::UiFonts` resource that every `Text`-spawning client plugin reads. Mounted by each
/// windowed composition root; retires Bevy's ASCII-only default font.
mod ui_font;
/// Ship-facing view-layer combat VFX: render-only subscribers to the sim's `Impact` and
/// `FireShell` seams (impact puffs, the 88's muzzle flash/light/smoke + shell smoke trail) plus
/// the shared billboard/erosion/gradient-LUT machinery they are built from. Mounted by both
/// windowed clients (ADR-0014 — never the server).
mod vfx;
/// THE LIVE VIEW, once: the single 3-D camera's field and the pixel height the main pass renders
/// at, read by ONE system on human-rate events, plus the exact screen-space projection (ADR-0033
/// §9) both LOD ladders derive their switch distances through. `terrain_lod` and `geometry_lod` are
/// consumers of it — each pairing the shared facts with its OWN pixel budget, which is a tuning
/// knob and not view state. Mounted by every windowed root; a headless composition has no view.
mod view;
mod world;

/// Configure Bevy's default plugin group for a GPU-less composition root.
///
/// This owns only the shared render-backend, primary-window, and winit-runner edits. Callers keep
/// ownership of their asset/image workarounds, clocks, runners, and physics choices.
pub(crate) fn gpu_less_default_plugins(
    primary_window: Option<Window>,
) -> bevy::app::PluginGroupBuilder {
    DefaultPlugins
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

/// The windowed clients' `RenderPlugin`: stock settings everywhere except macOS, where the two
/// GPU-query wgpu features are removed before device creation. Mounted by every windowed client
/// composition root (the net client and the offline route) — never by the GPU-less server, which
/// has its own [`gpu_less_default_plugins`] render settings.
///
/// WHY (the whole causal chain, MEASURED 2026-07-26 on the M4 / macOS 26.5 / Metal / wgpu 29):
/// under the default `WgpuSettingsPriority::Functionality`, bevy takes the ADAPTER's feature set
/// wholesale and only then applies `disabled_features`, before device creation (vendored
/// bevy_render-0.19.0/src/renderer/mod.rs:298-315). A `bevy/trace_tracy` build auto-mounts
/// `RenderDiagnosticsPlugin` (vendored bevy_render-0.19.0/src/lib.rs:379-380), whose
/// `DiagnosticsRecorder` sees `TIMESTAMP_QUERY` on the device and allocates a 256-entry timestamp
/// query set (`FrameData::new`, vendored bevy_render-0.19.0/src/diagnostic/internal.rs:236-254).
/// Metal refuses `newCounterSampleBufferWithDescriptor` ("Cannot allocate sample buffer"),
/// wgpu-hal turns that into a `DeviceLost` render error, and bevy's error handler quits the app
/// within ~1 s of the first frame. Removing the feature HERE means the recorder never allocates
/// the query set, so a tracy session survives on this Mac and keeps every CPU span (per-system,
/// render-graph nodes, the shadow pass).
///
/// On macOS this costs nothing real: bevy already no-ops every encoder timestamp on macOS citing
/// the bevy#22257 Tahoe flicker (`WriteTimestamp for CommandEncoder`, vendored
/// bevy_render-0.19.0/src/diagnostic/internal.rs:744-760), and wgpu#9414 reports Metal4 timestamp
/// queries return zeros anyway — the GPU-timestamp path is dead upstream on this platform.
/// `PIPELINE_STATISTICS_QUERY` rides along because the same `FrameData::new` allocates a second
/// query set for it and wgpu implements pipeline statistics only on Vulkan/DX12.
///
/// The cfg gate is the point: on Windows/Linux (Vulkan/DX12) both features stay enabled, so
/// `elapsed_gpu` diagnostics and the tracy GPU track remain real there.
pub(crate) fn client_render_plugin() -> bevy::render::RenderPlugin {
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

/// Push an entity onto a capped FIFO, then evict the oldest entities until the cap is restored.
///
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
mod offline_feel_tests {
    use super::*;
    use track::sim::{TankTransmission, TransmissionFeelTest};
    use track::transmission::{DriveReadout, TransmissionMode, TransmissionState};

    /// The offline `T` dial: each press advances governor → hybrid → L600 → governor and
    /// resets every tank's transmission state to a freshly-constructed one (the mode flip
    /// must never inherit another adapter's gear/shift leftovers). This is the scripted
    /// stand-in for the interactive cycle proof — macOS blocks synthetic keystrokes into the
    /// windowed launch, so the cycle path is pinned here instead.
    #[test]
    fn t_key_cycles_transmission_mode_and_resets_state() {
        let mut app = App::new();
        app.insert_resource(TransmissionFeelTest(TransmissionMode::FixedRadii));
        app.init_resource::<ButtonInput<KeyCode>>();
        app.add_systems(Update, cycle_transmission_feel);
        let tank = app
            .world_mut()
            .spawn(TankTransmission(TransmissionState {
                gear: 5,
                shift_ticks: 3,
                steer_step: 2,
                reverse: true,
                park: true,
                last_shift_dir: 1,
                dwell_ticks: 7,
                omega_e: 250.0,
                clutch_out: true,
                demand_n: 42_000.0,
                demand_initialized: true,
                grade_confirm_ticks: 9,
                band_confirm_ticks: 4,
                grade_target: 3,
                scheduler: track::transmission::SchedulerState::GradeShift { from: 5, to: 3 },
                hill_hold: true,
                hold_reengage_ticks: 11,
            }))
            .id();

        let press = |app: &mut App| {
            let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            input.clear();
            input.release(KeyCode::KeyT);
            input.clear();
            input.press(KeyCode::KeyT);
            app.update();
            app.world().resource::<TransmissionFeelTest>().0
        };
        assert_eq!(press(&mut app), TransmissionMode::Governor);
        assert_eq!(
            app.world().get::<TankTransmission>(tank).unwrap().0,
            TransmissionState::for_governor(),
            "a mode flip must reset the transmission state"
        );
        assert_eq!(press(&mut app), TransmissionMode::Hybrid);
        assert_eq!(press(&mut app), TransmissionMode::FixedRadii);
    }

    #[test]
    fn reverse_grade_shift_hud_uses_reverse_ladder_letter() {
        let state = TransmissionState {
            reverse: true,
            scheduler: track::transmission::SchedulerState::GradeShift { from: 4, to: 2 },
            ..TransmissionState::for_governor()
        };
        assert_eq!(drive_hud::scheduler_hud_line(&state), "sched GRADE R4->R2");
    }

    #[test]
    fn fixed_radii_steering_hud_names_detent_and_authored_radius() {
        let radii = [
            (3.44, 10.2),
            (5.28, 15.6),
            (7.62, 22.5),
            (11.30, 33.4),
            (17.32, 51.2),
            (25.68, 76.0),
            (37.47, 110.8),
            (55.78, 165.0),
        ];
        let mut state = TransmissionState {
            gear: 1,
            steer_step: 2,
            ..TransmissionState::for_governor()
        };
        assert_eq!(
            drive_hud::steering_hud_line(TransmissionMode::FixedRadii, &state, Some(&radii)),
            "STEER II R~3m"
        );

        state.gear = 8;
        state.steer_step = 1;
        assert_eq!(
            drive_hud::steering_hud_line(TransmissionMode::FixedRadii, &state, Some(&radii)),
            "STEER I R~165m"
        );

        state.steer_step = 0;
        assert_eq!(
            drive_hud::steering_hud_line(TransmissionMode::FixedRadii, &state, Some(&radii)),
            "",
            "released steering leaves the visibility field blank"
        );
        state.steer_step = 2;
        assert_eq!(
            drive_hud::steering_hud_line(TransmissionMode::Hybrid, &state, Some(&radii)),
            "",
            "the authored-detent readout is FixedRadii-only"
        );
    }

    /// One formatter owns the exact normal row in both client roots. `P` and `*` are independent
    /// park/hill-hold markers; cruise/reverse leave both marker columns blank; Governor hides the
    /// inapplicable gear/rpm fields without hiding speed.
    #[test]
    fn standard_drive_row_format_is_shared_and_compact() {
        let speed_mps = 12.5; // DERIVED 45 km/h.
        let mut state = TransmissionState {
            gear: 1,
            park: true,
            hill_hold: true,
            ..TransmissionState::for_governor()
        };
        let mut operating = DriveReadout {
            rpm: 2_600.0,
            gear_label: "F1".to_string(),
        };
        assert_eq!(
            drive_hud::standard_drive_row(Some((&state, &operating)), speed_mps),
            "Gear F1P*  RPM 2.6k  Speed  45 km/h"
        );

        state.hill_hold = false;
        assert_eq!(
            drive_hud::standard_drive_row(Some((&state, &operating)), speed_mps),
            "Gear F1P   RPM 2.6k  Speed  45 km/h",
            "park owns P independently of hill hold"
        );
        state.park = false;
        state.hill_hold = true;
        assert_eq!(
            drive_hud::standard_drive_row(Some((&state, &operating)), speed_mps),
            "Gear F1*   RPM 2.6k  Speed  45 km/h",
            "hill hold owns * independently of park"
        );

        state.park = false;
        state.hill_hold = false;
        state.gear = 8;
        operating.gear_label = "F8".to_string();
        assert_eq!(
            drive_hud::standard_drive_row(Some((&state, &operating)), speed_mps),
            "Gear F8    RPM 2.6k  Speed  45 km/h"
        );

        state.gear = 2;
        state.reverse = true;
        operating.gear_label = "R2".to_string();
        assert_eq!(
            drive_hud::standard_drive_row(Some((&state, &operating)), speed_mps),
            "Gear R2    RPM 2.6k  Speed  45 km/h"
        );
        assert_eq!(
            drive_hud::standard_drive_row(None, speed_mps),
            "Speed  45 km/h",
            "Governor/spec-less vehicles retain speed and omit inapplicable fields"
        );
    }

    #[test]
    fn standard_drive_row_rounds_rpm_and_horizontal_ground_speed() {
        let state = TransmissionState::for_governor();
        let operating = DriveReadout {
            rpm: 2_649.0,
            gear_label: "F1".to_string(),
        };
        let ground = drive_hud::horizontal_ground_speed(Vec3::new(3.0, 99.0, 4.0));
        assert_eq!(ground, 5.0, "vertical velocity is excluded");
        assert_eq!(
            drive_hud::standard_drive_row(Some((&state, &operating)), ground),
            "Gear F1    RPM 2.6k  Speed  18 km/h"
        );
        let rounded = drive_hud::standard_drive_row(Some((&state, &operating)), 12.7);
        assert_eq!(rounded, "Gear F1    RPM 2.6k  Speed  46 km/h");

        let narrow = DriveReadout {
            rpm: 849.0,
            gear_label: "F1".to_string(),
        };
        let narrow = drive_hud::standard_drive_row(Some((&state, &narrow)), 2.0);
        assert_eq!(narrow, "Gear F1    RPM 0.8k  Speed   7 km/h");
        assert_eq!(
            rounded.len(),
            narrow.len(),
            "compact rpm and speed fields retain a stable row width"
        );
    }

    #[test]
    fn f3_debug_toggle_defaults_closed_and_latches_each_press() {
        let mut visible = false;
        visible = drive_hud::debug_visible_after_f3(visible, false);
        assert!(!visible, "default/no press stays hidden");
        visible = drive_hud::debug_visible_after_f3(visible, true);
        assert!(visible, "first F3 press opens the drive diagnostics");
        visible = drive_hud::debug_visible_after_f3(visible, false);
        assert!(visible, "the view state latches between presses");
        visible = drive_hud::debug_visible_after_f3(visible, true);
        assert!(!visible, "second F3 press closes the drive diagnostics");
    }
}

/// Marks a network-client replica. Ballistics uses it to suppress authority-only damage and impulse
/// writes while retaining cosmetic flight and impacts.
#[derive(Resource, Default)]
pub(crate) struct ClientReplica;

/// Net-neutral current tick published before gameplay. Local network fire uses it to construct a
/// [`ShotId`] before shell spawn; replica ballistics ages sanctioned outcomes and stamps lifecycle
/// rows against it; authority/sandbox shells may be unkeyed.
#[derive(Resource, Default)]
pub(crate) struct ShotClock(pub u32);

/// Current simulation tick used by the tick-correlated weapon gate. Network compositions publish
/// `LocalTimeline` into it before gameplay; non-network compositions
/// advance it once after each playing fixed tick and saturate at `u32::MAX` like Lightyear.
#[derive(Resource, Default)]
pub(crate) struct WeaponClock(pub u32);

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
}

/// A non-zero, match-local identity assigned synchronously when a combatant spawns.
///
/// Entity ids are an ECS implementation detail: a respawn receives a new entity and every client
/// maps that entity independently. This value stays with the player or bot across respawn, making
/// delayed outcomes addressable without depending on either lifetime or mapping.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(crate) struct CombatantId(pub(crate) u64);

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

/// The simulation — the authority layer, in the client/server sense (see the memory note and
/// bevy_replicon's "abstracting over configurations"): everything the server must run to be the
/// truth; the net client mounts the same rules for its derived cosmetics (shells, servos, recoil
/// response). Consumes `TankCommand`s, never devices;
/// steps on the fixed clock. A dedicated server mounts exactly this (plus netcode) on
/// `MinimalPlugins`; the single-player game mounts it alongside [`ClientPlugin`].
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: physics (avian `PhysicsPlugins`) is deliberately NOT mounted here — its
        // configuration is the one thing that legitimately differs per composition root:
        // single-player wants `PhysicsInterpolationPlugin::interpolate_all()` (ADR-0004), the
        // networked bins must disable exactly that plugin for `LightyearAvianPlugin` (spike log,
        // increment 5). The composition root (GamePlugin / the net bins) owns the choice.
        app.add_plugins((
            state::sim_plugin,
            world::plugin,
            // The shared analytic terrain field (track architecture §5): built from
            // `TerrainMap` for the sim force systems (phase B) and the client track view —
            // one oracle on server, SP, and net client alike.
            track::terrain_plugin,
            // `spec` registers the `.tank.ron` data-asset loader before `tank` spawns the tank
            // and requests one (ADR-0010).
            spec::plugin,
            // The trio's certificate, and the fingerprint of the ONE artifact both ends walk
            // (`<id>.sim.glb`). Mounted before `bake`, which opens that artifact.
            geometry_lod::sim_plugin,
            // Sim/view split: extract the tank's SIM glb as data at startup — the sim skeleton's
            // spawn source on every composition (SP, net client, net server) — and shadow-verify it
            // against every instantiated scene.
            bake::plugin,
            tank::sim_plugin,
            // Commands are the sim's only input: `core_plugin` puts a `TankCommand` on every tank
            // and consumes latched edges each tick; `driving`/`shooting`/`aim` read it.
            command::core_plugin,
            // Phase-B locomotion: the track model's belt forces ARE the driving sim
            // (ADR-0025; replaces the raycast-roadwheel model of ADR-0005).
            track::sim_plugin,
            aim::sim_plugin,
            // `ballistics` owns the shell trajectory + impact seam; `shooting` is the gun control
            // that drives it (the sandbox drives the same `FireShell` from its camera). SIM half
            // only: the shell scene, the tracer streak and shell visibility are
            // `ballistics::view_plugin`, which the client roots mount and the server never does.
            ballistics::sim_plugin,
            damage::plugin,
            shooting::plugin,
        ));
    }
}

/// Gate the device-reading [`state::PlayerInputSet`] on a captured cursor (`state::cursor_locked`),
/// in each schedule its members live in: `Update` (aim commit, view toggle), `PostUpdate` (free-look
/// orbit), and `RunFixedMainLoop` (gunner aim, range dial, drive gather). Shared by both windowed
/// composition roots — SP [`ClientPlugin`] and net [`NetClientPlugin`] — so the license to consume
/// mouse/gameplay input (`grab_mode == Locked`) is configured identically in one place. The headless
/// server and the scripted harness mount neither root, so the gate never touches them.
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

/// The client — command generation (devices → `TankCommand`) and presentation (state → screen).
/// Requires [`SimPlugin`] in the same app (single-player and listen-server mount both; a pure
/// network client will too, for interpolation/prediction).
pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        gate_player_input(app);
        app.add_plugins((
            // Load the bundled UI font first: it inserts `UiFonts` at build time, so the HUD/crew
            // spawn systems below always find it (see `ui_font`).
            ui_font::plugin,
            branding::plugin,
            // Exit plumbing: `Cmd+Q` -> `AppExit` on macOS, and the render-app recall that keeps
            // bevy's own exit teardown from deadlocking against the render thread (bevy#12912).
            quit::plugin,
            // Pause/cursor handling (drives the states that `state::sim_plugin` owns).
            state::client_plugin,
            // Device gather: the only device→command translation.
            command::client_plugin,
            tank::client_plugin,
            // Resolves every `VisualScope`/`CameraProfile`/`LightProfile` into bevy's own render
            // components. Mounted before the modules that declare them so the ordering reads in
            // dependency order; the systems themselves are `PostUpdate` and order-independent.
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
            ballistics::view_plugin,
        ));
        app.add_plugins(drive_hud::plugin);
        // View-layer audio (view-only, ADR-0014 — the server never mounts this): the trigger click,
        // each weapon's authored report on the fire seam, and the per-tank engine loop.
        app.add_plugins(sfx::plugin);
        // Per-frame wall-clock recorder (idle unless `SPIKE_FRAME_COST` is set) — mounted on this
        // root so the offline frame-budget sweep needs no server; the net root mounts it too.
        app.add_plugins(frame_cost::client_plugin);
        // Per-render-pass cost recorder (idle unless `SPIKE_RENDER_COST` is set), the render-side
        // companion to the recorder above. Mounted here for the same reason: the offline captures
        // (`scripts/perf/run-fire-capture.sh`) set both variables and run with no server, and
        // without this mount the render-cost half asked for was silently never written.
        app.add_plugins(render_cost::client_plugin);
        // Player graphics settings + the Esc settings page + the render-scale render-app half, all
        // behind one mount. SP's pause surface is `AppState::Paused` (there is no overlay authority
        // here), so the page's visibility is declared from the state — which is the plugin's one
        // parameter, so mounting the page without a declarer is unrepresentable.
        app.add_plugins(settings::plugin(settings::PageEntry::PauseState));

        // Physics visualization (collider/ray wireframes) + debug toggles, behind the `dev_tools`
        // feature (default-on, droppable from an optimized build via `--no-default-features`).
        // Offline SP registers no diagnostic plugins at all, which is accepted: the settings page
        // covers the render knobs, `cargo tracy` is the frame-cost instrument, and the net client
        // still has `net::debug_hud`'s fps/frame-time card.
        #[cfg(feature = "dev_tools")]
        app.add_plugins((avian3d::prelude::PhysicsDebugPlugin, debug::plugin));
    }
}

/// The networked client's presentation + device gather (Milestone B step 8): [`ClientPlugin`]
/// minus the single-player-only pieces. No `state::client_plugin` — its Esc pause freezes the
/// local sim and physics clock, which desyncs a predicting client from a server that keeps
/// ticking; there is no online pause, so the netcode bin owns its own cursor-release menu overlay
/// instead. No `tank::client_plugin` — the Tab possession swap is an SP scenario tool; under
/// netcode the server assigns possession (`ControlledBy`).
pub struct NetClientPlugin;

impl Plugin for NetClientPlugin {
    fn build(&self, app: &mut App) {
        gate_player_input(app);
        app.add_plugins((
            // Load the bundled UI font first (inserts `UiFonts` at build time; see `ui_font`).
            ui_font::plugin,
            branding::plugin,
            command::client_plugin,
            camera::plugin,
            aim::client_plugin,
            sight::plugin,
            firecontrol::client_plugin,
            hud::plugin,
            crew_ui::plugin,
            // The single overlay authority (net-client only): one active-set resource + derived
            // input/cursor/scrim rules behind which connect status, the death screen, the Esc menu,
            // and the view-death black all compose with explicit priority and z-order. Owns the one
            // cursor system; the connect/death/sight owners declare their presence into it.
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
        // The live view + the render half of the certificate (see `ClientPlugin`), separate for the
        // same arity reason as the block below.
        app.add_plugins((view::plugin, geometry_lod::view_plugin));
        // (Separate call: the tuple above is at bevy's 15-plugin tuple arity limit.)
        app.add_plugins((
            // The render half of ballistics (see `ClientPlugin`): the shell glb, the tracer streak
            // and hold visibility. `SimPlugin` mounts only the sim half.
            ballistics::view_plugin,
            drive_hud::plugin,
            // The render-policy resolver (see `ClientPlugin`) — every windowed root needs it, or
            // nothing that declares a scope or a profile is ever resolved. Down here rather than
            // beside `camera::plugin`: the tuple above is at bevy's 15-plugin arity limit.
            render_policy::plugin,
            // Exit plumbing: `Cmd+Q` -> `AppExit` on macOS, and the render-app recall that keeps
            // bevy's own exit teardown from deadlocking against the render thread (bevy#12912).
            // (Down here rather than beside `branding::plugin`: the tuple above is full.)
            quit::plugin,
            // The `M` spawn map — net-client only: a top view of the terrain whose click asks the
            // authority to place this player's NEXT respawn there (nothing teleports now).
            net::spawn_map_plugin,
            // Player graphics settings + the Esc settings page + the render-scale render-app half,
            // all behind one mount. The page IS the Esc menu's content (`Overlay::Menu` already
            // blocks input, frees the cursor and owns the scrim), which is what the `OverlayMenu`
            // entry names — and naming it is not optional, so the page cannot be mounted without the
            // declarer that makes it visible.
            settings::plugin(settings::PageEntry::OverlayMenu),
        ));
        // View-layer audio (see `ClientPlugin`): the trigger click, each weapon's authored report,
        // and the engine loop on own AND remote tanks — `TankTransmission` is replicated, so one
        // code path.
        app.add_plugins(sfx::plugin);

        // Physics visualization + debug toggles, same pair `ClientPlugin` mounts for SP
        // (`G` = force arrows + collider wireframes, `X` = x-ray, `F` = camera detach). View-only:
        // it reads `TrackContacts`/`GlobalTransform` and draws gizmos — nothing sim-visible — so it is
        // safe on a predicting client and is never mounted by the headless server (which composes
        // `SimPlugin` only, never this plugin). Behind the `dev_tools` feature (default-on).
        #[cfg(feature = "dev_tools")]
        app.add_plugins((avian3d::prelude::PhysicsDebugPlugin, debug::plugin));
    }
}

/// Every gameplay feature, composed — the single-player configuration: the full sim plus the
/// local client, one app, no netcode. Add to an `App` that already has the runtime plugins.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // The single-player physics choice: bodies render at an interpolated pose between
            // fixed steps (ADR-0004). The networked bins mount lightyear's config instead.
            PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()),
            SimPlugin,
            // The single-player scenario: two-tank duel spawn, first tank controlled.
            tank::sp_spawn_plugin,
            // The LOD eyeball harness's runtime half — the per-tank shoe clamp and its one-shot
            // camera aim. Adds NO systems unless `OVERMATCH_LOD_SHOWCASE` is set, and the scenario
            // spawn above lays out the pairs instead of the duel under the same variable.
            lod_showcase::plugin,
            ClientPlugin,
            // Passive jitter-trace recorder (frame + tick rows; no net extras in this build). Idle
            // unless `SPIKE_TRACE` is set.
            trace::sp_plugin,
        ));
    }
}

/// The offline feel-test route (`overmatch --offline` / `OVERMATCH_OFFLINE=1`): the windowed
/// runtime plugin set [`run_client`](net::run_client) mounts — exe-relative asset root, continuous
/// winit updates — plus [`GamePlugin`], the true single-player composition. NO netcode: no
/// lightyear plugins, no connection entity, nothing that can attempt a connect.
///
pub fn run_offline() {
    // Hidden-capture mode, the same contract as the net client's capture path (`net::run_client`'s
    // composition): `SPIKE_SIM_WINDOWED` without `SPIKE_SIM_VISIBLE` keeps the full presentation
    // stack but creates the window invisible — `visible: false` is the ONLY macOS lever that stops
    // a capture run stealing focus (bevy's `set_visible(true)` is `makeKeyAndOrderFront`; see the
    // window-field comments in `net::client`). Used by `scripts/perf/run-frame-sweep.sh`'s SMOKE
    // mode; a hidden window's present returns `SurfaceError::Occluded` so the frame loop
    // FREE-RUNS — plumbing validation only, never a frame-time measurement.
    let hidden = env_flag("SPIKE_SIM_WINDOWED", false) && !env_flag("SPIKE_SIM_VISIBLE", false);
    let mut app = App::new();
    // Read the player's settings BEFORE the window is described — see `load_at_boot`, which also
    // inserts the values and the boot report into the app.
    let settings = settings::load_at_boot(&mut app);
    // Exe-relative asset root, exactly as `run_client` resolves it: a double-clicked binary
    // finds `assets/` beside it no matter the launch cwd.
    app.add_plugins(
        DefaultPlugins
            // On macOS this drops the GPU-query wgpu features whose timestamp query set Metal
            // refuses to allocate (tracy builds DeviceLost-quit without it); see
            // `client_render_plugin` for the full causal chain and the vendored citations.
            .set(client_render_plugin())
            .set(bevy::asset::AssetPlugin {
                // The same `String` conversion `net::client`'s wrapper applies.
                file_path: assets::asset_root().to_string_lossy().into_owned(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    // ASCII hyphen: `lib.rs` is scanned by the ui_ascii guard now that the
                    // offline feel label spawns `Text` here (default-font surface).
                    title: "Overmatch - offline".into(),
                    // `Unprobed` on purpose: the capability probe needs a surface, which needs this
                    // window — the mapping self-negotiates until the probe lands.
                    present_mode: settings.present_mode(settings::PresentCaps::Unprobed),
                    // Described at creation so a persisted fullscreen boots fullscreen instead of
                    // flashing a window first. The monitor is `at_window_creation`'s and NOT the
                    // stored rung: naming an indexed display here would be cached unresolved by
                    // bevy and would make the `Startup` correction unrepresentable — see
                    // `settings::DisplaySelection`. A hidden capture run must NOT boot into the
                    // player's persisted fullscreen — it stays a plain window.
                    mode: if hidden {
                        bevy::window::WindowMode::Windowed
                    } else {
                        settings
                            .window_mode
                            .to_window_mode(settings.display.at_window_creation())
                    },
                    visible: !hidden,
                    ..default()
                }),
                ..default()
            }),
    );
    // Same policy as the net client: never drop below the 64 Hz tick when unfocused.
    app.insert_resource(bevy::winit::WinitSettings::continuous());
    if hidden {
        // The runtime half of the hidden-window guard: without the pin, `settings` re-applying a
        // persisted fullscreen a frame later would order the window front regardless of
        // `visible: false` (see `settings::CaptureWindowPinned`).
        app.insert_resource(settings::CaptureWindowPinned);
        // Post-launch activation revocation — winit's didFinishLaunching has already made even an
        // invisible unbundled binary the active app; see the doc on the shared function.
        #[cfg(target_os = "macos")]
        app.add_systems(Startup, net::revoke_macos_activation);
    }
    app.add_plugins(GamePlugin);
    // The offline transmission feel test (phase 2.5): an EXPLICIT override of the spec's
    // declared architecture (which is authoritative and mandatory since REV 14), seeded to
    // the Tiger's own L600 so a bare offline boot matches the shipped spec. `T` cycles
    // governor → hybrid → L600 live; the shared F3 drive panel names the selected adapter
    // while the diagnostic view is open.
    app.insert_resource(track::sim::TransmissionFeelTest(
        track::transmission::TransmissionMode::FixedRadii,
    ));
    app.add_systems(
        Update,
        cycle_transmission_feel.before(drive_hud::DriveHudUpdate),
    );
    // The scripted-trigger capture hook (`SPIKE_AUTO_FIRE`), mounted HERE and nowhere else: this
    // offline root has no netcode at all — no lightyear plugins, no connection entity — so the
    // commands it writes cannot leave the process. Mounting it inside the shared device-gather
    // plugin would put it in the packaged network client too (`dev_tools` is a default feature),
    // where scripted trigger edges would ride the input bridge to a live server. See
    // `command::offline_auto_fire_plugin`.
    #[cfg(feature = "dev_tools")]
    app.add_plugins(command::offline_auto_fire_plugin);
    app.run();
}

/// `T` cycles the offline transmission mode (governor → hybrid → L600). Every tank's
/// [`track::sim::TankTransmission`] resets so the incoming adapter starts from a constructed
/// state (gear 1, no shift in flight) instead of another mode's leftovers. The mode is logged
/// here; the shared F3 drive panel renders the active mode.
fn cycle_transmission_feel(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    feel: Option<ResMut<track::sim::TransmissionFeelTest>>,
    gear: Option<Res<track::sim::TrackGear>>,
    mut states: Query<&mut track::sim::TankTransmission>,
) {
    use track::transmission::TransmissionMode;
    let Some(mut feel) = feel else {
        return;
    };
    let cycled = keys.is_some_and(|keys| keys.just_pressed(KeyCode::KeyT));
    if cycled {
        feel.0 = match feel.0 {
            TransmissionMode::Governor => TransmissionMode::Hybrid,
            TransmissionMode::Hybrid => TransmissionMode::FixedRadii,
            TransmissionMode::FixedRadii => TransmissionMode::Governor,
        };
        let fresh = gear
            .as_deref()
            .and_then(track::sim::TrackGear::trans)
            .map_or_else(
                track::sim::TankTransmission::for_governor,
                track::sim::TankTransmission::from_spec,
            );
        for mut state in &mut states {
            *state = fresh;
        }
        info!("offline transmission mode → {}", feel.0.label());
    }
}
