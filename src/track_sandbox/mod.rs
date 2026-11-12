//! Isolated continuous-track locomotion sandbox (ADR-0025; origins in ADR-0005).
//!
//! The deterministic course and scripted harness isolate track work from netcode. The vehicle
//! uses the game's `TankBlueprint` (glb + `.tank.ron`) and marker-derived [`rig_geom::RigGeom`]:
//! a different lab vehicle would invalidate a game verdict. The field-belt contact model is
//! shared with `track::forces`.
//!
//! Bake inserts the blueprint with `Startup` commands; [`build_rig`] waits until `Update`
//! to consume that publication, then builds once.
//! Rig-dependent systems gate on `resource_exists::<RigGeom>`.
//!
//! # Control surface
//!
//! The `dev_ui`-only egui [`panel`] owns non-driving controls (Tune / Layers / Telemetry / Scene);
//! run with `cargo run --bin track_sandbox --features dev_ui`; egui stays out of the shipping
//! client. Arrow keys drive; `WASD`+mouse+Shift/Ctrl free-fly;
//! `R` cycles reset spots; `Esc` pauses/releases the cursor; `L` logs; `;`/`'` and `n`/`m` tune
//! link/tooth counts. [`PanelWantsInput`] gates driving/camera input while a widget has focus.
//!
//! Panel and keyboard write on change through [`RigCounts`] / [`apply_rig_counts`] and
//! [`ResetRequested`] / [`reset_rig`], sharing commit paths and preserving change ticks.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, Collider, CollisionLayers, Forces, LayerMask, LinearVelocity,
    Mass, NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, Physics, PhysicsDebugPlugin,
    PhysicsGizmos, PhysicsTime, ReadRigidBodyForces, RigidBody, WriteRigidBodyForces,
};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use bevy::world_serialization::WorldAssetRoot;

use crate::Layer;
use crate::bake::TankBlueprint;
use crate::track::bump_stop::{BumpStop, SidePlate, TrackColliders, spawn_track_colliders};

/// The headless measurement beds over the harness (`-- --ignored`).
#[cfg(test)]
mod bed;
mod harness;
// Instances the asset's shoe mesh at resampled belt stations; other track overlays are lines.
mod link_view;
// Shared field-belt force/view adapters.
mod belt;
// Hull solid/x-ray/hidden and `*_Collider` / `*_Ballistic` off/on-top/solid/x-ray layers.
// The overlay camera composites on-top volumes; `mesh_layers` owns the panel's state enums.
mod mesh_layers;
// The rubble yard: loose brick pieces and a standing wall section cut from the shipped house, for
// the belts to climb and push. `pub(crate)`: the track scorecard builds its heap with the yard's
// own builder.
pub(crate) mod rubble;
// Binds glb wheel/sprocket/idler nodes by name and renders suspension travel + belt phase;
// otherwise `articulate_wheels_field` would update state without moving the rendered wheels.
mod wheel_view;
// Shared suspension laws and marker-derived geometry: the sandbox's sole running-gear geometry
// source (`crate::track::marker_model::DerivedModel`). Re-exports preserve submodules'
// `super::derive` / `super::rig_geom` paths.
pub(crate) use crate::track::derive;
pub(crate) use crate::track::rig_geom;
// Rest/droop/compression cast routes, grip columns and sprocket ring follow the live hull pose.
// Text readouts belong to `panel`.
pub(crate) mod suspension_viz;

mod data;
mod panel;
use data::scene;

use belt::{
    BeltPhase, PinBelt, RigTransmission, TerrainField, ViewPerf, WrapMemory,
    apply_belt_support_field, articulate_wheels_field, conform_belts_field, draw_sample_points,
};
use derive::SuspensionParams;
use mesh_layers::{MeshState, VolumeState};
use rig_geom::{Pose, RigGeom};
// Re-export the shared geometry core for model submodules using `super::*`.
pub(crate) use crate::track::oracle::{BlockField, TerrainBlock};
pub(crate) use crate::track::route::{arc, external_tangent, polyline_len, resample};
pub(crate) use crate::track::side::{PerSide, Side};

// Rig lengths, radii, centres and counts come from `RigGeom`, derived from glb markers and tank
// data; contact-model policies are not geometry measurements.

/// Live [`SuspensionParams`], seeded from the same authored `track.suspension` as the game's
/// envelope calibration and edited by the panel. [`RigGeom`] uses them for cast poses and the
/// link-count window. One resource keeps the panel verdict and [`tune_rig_counts`] clamp in sync.
#[derive(Resource, Default)]
pub(crate) struct RigSuspension(pub(crate) SuspensionParams);

/// Non-geometry blueprint data read once at rig build for the fixed-tick force law.
/// Belt/transmission inertia is DERIVED from links and wheel moments; envelope calibration uses
/// combat weight. Geometry belongs to [`RigGeom`].
#[derive(Resource)]
struct RigSpec {
    /// Combat weight (N) = `spec.mass · g` — the scale of the static-grip bristle stiffness, the
    /// envelope-law calibration, and the support-fraction readout.
    weight_n: f32,
    /// Soft-engagement depth (m): ramps contact force to model track compliance. Well below
    /// static sink, it affects the contact boundary rather than ride height.
    engage_depth: f32,
    /// DERIVED per-side belt inertia (kg): the links plus the wheel moments reflected at belt
    /// speed ([`crate::spec::TrackSpec::reflected_inertia`]). The engine crank stays on the
    /// engine side of the clutch.
    belt_inertia: f32,
    /// Solid-recipient track friction and nominal pressure from construction; terrain supplies
    /// effective grip through the ground field.
    grip: crate::track::forces::GripParams,
}

/// The pad's top-face centre and tilt; harness slope poses share this construction.
pub(super) fn slope_pad_pose() -> (Vec3, Quat) {
    let data = scene();
    let rot = Quat::from_rotation_x(data.slope_pad_deg.0.to_radians());
    (
        Vec3::from_array(data.slope_pad_center_m.0)
            + rot * Vec3::Y * (data.slope_pad_thickness_m.0 / 2.0),
        rot,
    )
}

// Station spacing is `geom.pitch`; material length is `geom.belt_len()` = pitch × link_count.
// Per-metre coefficients keep total force independent of sampling resolution. Slack is the
// authored loop minus taut perimeter (`RigGeom::window`), not an extra budget.
/// Downward ray length used to find ground just beneath each station (m); also the sink at which
/// support saturates.
fn contact_probe() -> f32 {
    scene().contact_probe_m.0
}

// Vehicle data supplies the drivetrain envelope; `track::forces` derives wheelspin, skid,
// engine braking, hill hold and top speed from belt/ground slip. Friction is the material pair.

// The belt alone supplies ground support/traction across gaps; wheels carry no force.
// Authored collision proxies handle walls and failed bridges (see `report_collision_proxies`).
// Visuals flow ground → wheels → belt (`articulate_wheels_field`): belt-first would be circular.
// Instant rise avoids terrain entering a lagging wheel; for feel the shared wheel law uses
// critically damped rise instead (`crate::track::wheels::wheel_lift_step`). Fall is ballistic.
// Cosmetic travel spans the physical envelope: `SuspensionParams::bump_stop` to
// `RigGeom::droop_travel(..).effective`, not an arbitrary clamp.

/// A wheel's role in the running gear. The sprocket (front) and idler (rear) anchor the belt loop
/// and carry no ground load; the road wheels are the suspension/contact stations.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WheelKind {
    Sprocket,
    Road,
    Idler,
}

/// A single wheel of the code-generated rig: its side and role (radius follows from the role).
/// Spawned as a child of the hull, so its `GlobalTransform` follows the hull (and, for road wheels,
/// its own cosmetic travel).
#[derive(Component)]
struct RigWheel {
    side: Side,
    kind: WheelKind,
}

/// The physical tread belongs to the declared station from its construction flush onward.
#[derive(Component)]
struct RoadTread(crate::spec::WheelTread);

/// The spec node whose baked station this carrier represents; names address, never classify.
#[derive(Component)]
struct StationAddress(String);

/// Visual-only road-wheel placement: hull-local rest pivot, vertical lift and ballistic fall
/// speed `dvel`. Rise/fall and travel bounds follow the wheel doctrine above [`RigWheel`].
#[derive(Component)]
struct Suspension {
    pivot_local: Vec3,
    dy: f32,
    dvel: f32,
}

/// One station of the conformed belt: its hull-local side-plane position on the rigid reference loop
/// (z, y — *pre*-conform, used to tell the belly from the top run and to align with wheels) and its
/// conformed world position (raised onto terrain).
struct BeltSample {
    local: Vec2,
    world: Vec3,
}

/// Loop-ordered belt around articulated wheels, conformed to terrain once per frame by
/// `conform_belts_field`. The drawn spline uses this exact path.
#[derive(Resource, Default)]
struct ConformedBelts(PerSide<Vec<BeltSample>>);

impl ConformedBelts {
    fn get(&self, side: Side) -> &[BeltSample] {
        self.0.get(side)
    }

    fn get_mut(&mut self, side: Side) -> &mut Vec<BeltSample> {
        self.0.get_mut(side)
    }
}

/// Marker for the rig's hull body, and the frame every piece of
/// hull-local geometry ([`RigGeom`]) is expressed in. `pub(crate)` because the viz layers draw on
/// the moving tank and need `With<Hull>` to find it.
#[derive(Component)]
pub(crate) struct Hull;

/// The free-fly inspection camera (own copy, like `armor_sandbox`'s).
#[derive(Component)]
struct FreeFlyCam;

pub fn plugin(app: &mut App) {
    scene(); // Refuse tool data before construction or spawning the inspection scene.
    crate::track::wheels::wheel_lift_rise_omega();
    crate::track::wrap::wrap_feel_zeta();
    // Fingerprint the sim artifact before bake reads it and resolve the shoe's certified chain.
    app.add_plugins((
        crate::geometry_lod::sim_plugin,
        crate::track::world_contact::plugin,
        // The live view the shoe's chain is selected through.
        crate::view::plugin,
        crate::geometry_lod::view_plugin,
    ));
    // Inspect the authored surface at every distance: belt wrap, suspension envelope and tooth
    // mesh are checked by eye against millimetre overlays (`suspension_viz::draw_sprocket`).
    // CLAIMED inspection ranges: vantage 11.9 m from the belt, beyond the shoe's 6.9 m first switch
    // and 1 m
    // inside the road wheels' 12.9 m switch. Reducing the measured mesh through the certificate
    // being checked would invalidate that inspection.
    app.insert_resource(crate::geometry_lod::SourceDetailOnly);
    app.add_plugins((crate::spec::plugin, crate::bake::plugin))
        // Windowed `DefaultPlugins` needs `crate::quit` to avoid Bevy's exit deadlock.
        .add_plugins(crate::quit::plugin)
        // View layers wait for their inputs: overlay needs `RigGeom` + `Hull`; shoes need
        // `RigGeom` + glb template; running gear needs `RigGeom` + bound glb nodes.
        .add_plugins(suspension_viz::plugin)
        .add_plugins(link_view::plugin)
        .add_plugins(wheel_view::plugin)
        // Tags meshes as the glb instantiates, independently of `RigGeom`.
        .add_plugins(mesh_layers::plugin)
        .add_plugins(crate::offline_physics())
        .add_plugins(crate::contact_transfer::sim_plugin)
        // Registers the `PhysicsGizmos` group for the collider-wireframe layer; starts disabled in
        // `configure_collider_gizmos`.
        .add_plugins(PhysicsDebugPlugin)
        .init_resource::<BeltContacts>()
        .init_resource::<SideDynamics>()
        .init_resource::<BeltGrip>()
        .init_resource::<BeltGripElements>()
        .init_resource::<TransTelemetry>()
        .init_resource::<Paused>()
        .init_resource::<ResetSpot>()
        .init_resource::<RawDriveInput>()
        .init_resource::<ShapedDrive>()
        .init_resource::<BeltSpeed>()
        .init_resource::<BeltPhase>()
        .init_resource::<ConformedBelts>()
        .init_resource::<VizLayers>()
        .init_resource::<TautReference>()
        .init_resource::<TerrainField>()
        .init_resource::<rubble::RubbleTouches>()
        // `conform_belts_field` mutates this memory; reseats clear it. Feel laws are
        // unconditional and parameter-free.
        .init_resource::<WrapMemory>()
        .init_resource::<ViewPerf>()
        .init_resource::<RigSuspension>()
        // Present with false defaults even without the panel, for input/reset signatures.
        .init_resource::<ResetRequested>()
        .init_resource::<PanelWantsInput>()
        // `RigCounts` owns the pre-build sentinel contract.
        .init_resource::<RigCounts>()
        .add_systems(
            Startup,
            (
                spawn_camera,
                // A harness run must not steal the user's cursor while it captures.
                grab_cursor.run_if(not(resource_exists::<harness::Harness>)),
                spawn_environment,
                rubble::spawn_rubble_yard,
                configure_collider_gizmos,
            ),
        )
        .add_systems(Update, build_rig.run_if(rig_unbuilt))
        // Forces precede Avian's FixedPostUpdate integration. Paused forces would accumulate
        // and fling the rig on resume. `RigGeom` gates all rig resources (one command flush).
        // Support uses the advected pin-line ring and analytic terrain at fixed stations,
        // without narrow-phase queries.
        .add_systems(
            FixedUpdate,
            apply_belt_support_field
                .run_if(sim_running)
                .run_if(resource_exists::<RigGeom>),
        )
        .add_systems(
            Update,
            (
                // Wheel doctrine above `RigWheel` owns the data order. Esc pauses Avian,
                // not Update: stateful visuals must stop too, or wheels/wrap deform while
                // paused. Immediate-mode gizmos still redraw the frozen state.
                (
                    articulate_wheels_field.run_if(sim_running),
                    conform_belts_field.run_if(sim_running),
                    draw_rig_gizmos,
                )
                    .chain(),
                refresh_envelope,
                refresh_track_colliders,
                reset_rig,
                tune_rig_counts,
                apply_rig_counts,
                log_state,
                draw_contacts,
                draw_sample_points,
                rubble::draw_rubble,
            )
                // Everything above reads the rig (its resources or its `Hull`/`RigWheel`
                // entities), so it all waits for the deferred build.
                .run_if(resource_exists::<RigGeom>),
        )
        .add_systems(
            Update,
            (
                // Panel drags/clicks must not leak into camera/drive input. Cursor lock also
                // gates flying, since freeing the cursor to reach the panel stops mouse-look.
                fly_camera
                    .run_if(cursor_locked)
                    .run_if(not(panel_capturing)),
                read_drive_input.run_if(not(panel_capturing)),
                toggle_pause,
                // `apply_mesh_visibility` owns the every-frame visibility invariant.
                apply_mesh_visibility,
                sync_collider_gizmos.run_if(resource_changed::<VizLayers>),
                draw_taut_reference,
            ),
        );

    app.add_plugins(panel::plugin);

    // `SANDBOX_HARNESS`: scenario in, JSONL out, exit. Bit-repeatability requires one fixed
    // tick per rendered frame, no wall-clock sim input, and scripted throttle inside FixedUpdate
    // before forces so phase boundaries land on exact ticks. Otherwise pacing causes
    // MEASURED ~mm-level hull drift between identical runs and A/B gates become statistical.
    let harness_scenario = harness::parse_env().unwrap_or_else(crate::refuse_start);
    if let Some(scenario) = harness_scenario {
        mount_harness(app, scenario);
    }
}

/// Mount the scripted harness for `scenario` (module doc of `harness`).
fn mount_harness(app: &mut App, scenario: harness::Harness) {
    let output = harness::open_log(&scenario).unwrap_or_else(crate::refuse_start);
    {
        app.insert_resource(output)
            .insert_resource(scenario)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_micros(15_625), // exactly 1/64 s
            ))
            // Setup poses the hull once (`HarnessLog` latch). `after(build_rig)` forces a
            // flush so setup sees the hull in the build frame, before its first fixed tick
            // (scenario tick 0).
            .add_systems(
                Update,
                harness::harness_setup
                    .after(build_rig)
                    .run_if(resource_exists::<RigGeom>)
                    .run_if(not(resource_exists::<harness::HarnessLog>)),
            )
            .add_systems(
                FixedUpdate,
                harness::harness_drive
                    .before(apply_belt_support_field)
                    .run_if(resource_exists::<harness::HarnessLog>),
            )
            .add_systems(
                FixedUpdate,
                harness::harness_record
                    .after(apply_belt_support_field)
                    .run_if(resource_exists::<harness::HarnessLog>),
            );
    }
}

/// A live belt contact station for visualization: the station in **hull-local** space (so the dot
/// rides the interpolated rig instead of jittering against the last fixed-tick pose), its load, the
/// ground normal it pushes along, its longitudinal slip speed (m/s — colours the dot green→red),
/// and the total grip plus rolling force it applied (world space — the force-vector layer).
struct Contact {
    local: Vec3,
    /// Actual damped support load; traction capacity reads elastic load instead.
    load: f32,
    load_elastic: f32,
    normal: Vec3,
    slip: f32,
    slip_lat: f32,
    f_long: f32,
    f_lat: f32,
    /// Emitted contact capacity before mobilization, N.
    capacity_n: f32,
    tangential_force: Vec3,
}

/// The belt contact stations found this tick, PER SIDE `[left, right]` (side identity matters
/// for steer diagnostics) — filled in the fixed step, drawn by `draw_contacts` per frame.
/// Visualization/telemetry only.
#[derive(Resource, Default)]
struct BeltContacts(PerSide<Vec<Contact>>);

impl BeltContacts {
    fn all(&self) -> impl Iterator<Item = &Contact> {
        self.0.values().flatten()
    }
}

/// Per-side belt-dynamics telemetry from the core report: engine force applied and ground
/// reaction. Harness rows only.
#[derive(Resource, Default)]
struct SideDynamics {
    engine: PerSide<f32>,
    reaction: PerSide<f32>,
}

/// The per-side static-friction resultant (`SideState::grip`) — telemetry only: the sum of the
/// per-element shear the force law integrates, and the sandbox analogue of the game's `TrackGrip`
/// component.
#[derive(Resource, Default)]
struct BeltGrip(PerSide<Vec2>);

/// The per-element isotropic shear state (`track::forces::GripElements`): one world-space shear
/// vector per advected force cell × column. Always PRE-SIZED at force resolution ([`Self::sized`]; startup,
/// `R` reset, count retune): `contact_side` never resizes at runtime, and empty slabs would skip
/// traction for the tick.
#[derive(Resource, Default)]
struct BeltGripElements(PerSide<crate::track::forces::GripElements>);

impl BeltGripElements {
    /// Both sides at rest, pre-sized for the force field's station and lateral resolution.
    fn sized() -> Self {
        use crate::track::forces::GripElements;
        Self(PerSide::new(
            GripElements::for_stations(
                crate::track::forces::CONTACT_RESOLUTION.stations,
                crate::track::forces::CONTACT_RESOLUTION.lateral_points,
            ),
            GripElements::for_stations(
                crate::track::forces::CONTACT_RESOLUTION.stations,
                crate::track::forces::CONTACT_RESOLUTION.lateral_points,
            ),
        ))
    }
}

/// The joint transmission's state (gear, shift countdown, steering detent, direction) and the
/// Driver's ledger — the sandbox analogue of the game's `TankTransmission` and `DriverSense`
/// components. Reset with the rig.
#[derive(Resource)]
struct TransState(
    crate::track::transmission::TransmissionState,
    crate::track::sense::DriverSense,
);

impl TransState {
    fn fresh(params: &crate::track::transmission::TransmissionParams) -> Self {
        Self(
            crate::track::transmission::TransmissionState::from_spec(params),
            crate::track::sense::DriverSense::for_cut(params.shift_ticks),
        )
    }
}

/// Last tick's transmission report (gear/rpm/detent/power scale) — harness `tr` rows + the
/// legend. `None` before the first tick.
#[derive(Resource, Default)]
struct TransTelemetry(Option<crate::track::transmission::TransmissionReport>);

/// Recalibrate the envelope law whenever its inputs move: a rig rebuild (`;`/`'`, `n`/`m`,
/// `R`) or a suspension knob (the panel's Tune section). Change-detection driven — the calibration
/// runs one contact pass per side, far too heavy for every tick and trivial on a knob turn.
fn refresh_envelope(
    geom: Res<RigGeom>,
    suspension: Res<RigSuspension>,
    rig: Res<RigSpec>,
    mut law: ResMut<belt::EnvelopeLaw>,
    mut segment_law: ResMut<crate::track::belt_segment::SegmentLaw>,
) {
    if !(geom.is_changed() || suspension.is_changed() || rig.is_changed()) {
        return;
    }
    *law = belt::calibrate_envelope(&geom, &suspension.0, rig.weight_n, rig.engage_depth);
    *segment_law = law.segment_law();
    info!(
        "envelope recalibrated: travel {:.1} mm ({}), k {:.0} kN/m per m, c {:.2} kN·s/m per m",
        law.free_travel * 1e3,
        if geom.droop_travel(&suspension.0).chain_limited() {
            "CHAIN-limited"
        } else {
            "spring-limited"
        },
        law.stiffness_per_m / 1e3,
        law.damping_per_m / 1e3,
    );
}

/// Whether the sim is frozen (`Esc`). The belt model gates on this so it doesn't accumulate force
/// against a paused physics world.
#[derive(Resource, Default)]
struct Paused(bool);

fn sim_running(paused: Res<Paused>) -> bool {
    !paused.0
}

/// Independent panel Layers switches. Boot shows an x-ray hull, running gear and shoes;
/// other diagnostics start hidden to avoid clutter.
/// `Copy + PartialEq` lets the panel write only real changes, keeping the change-gated
/// `sync_collider_gizmos` from firing every frame.
#[derive(Resource, Clone, Copy, PartialEq)]
struct VizLayers {
    /// Hull `*_Visual` solid/x-ray/hidden loop, driven by [`mesh_layers`]. X-ray exposes the
    /// suspension through a translucent shell. Late-writer handling follows
    /// [`apply_mesh_visibility`]'s every-frame invariant.
    hull: MeshState,
    /// The asset's authored `*_Collider` proxy meshes — the two convex-hull backstops (`Hull_Collider`,
    /// `Turret_Collider`) rendered as translucent AMBER volumes (off → on-top → solid → x-ray). These
    /// are the same glb meshes the raw `WorldAssetRoot` spawn instantiates. DISTINCT from
    /// [`Self::colliders`], which draws the AVIAN physics collider WIREFRAMES via `PhysicsGizmos`.
    collider_volumes: VolumeState,
    /// The asset's authored `*_Ballistic` armour/component meshes, rendered as translucent STEEL-BLUE
    /// volumes (off → on-top → solid → x-ray). Render-only in this tool — the sandbox builds no
    /// ballistic colliders.
    ballistic_volumes: VolumeState,
    /// Scene-root course meshes from [`spawn_environment`], classified by [`mesh_layers`]
    /// ancestry rather than name so hull x-ray cannot ghost the world. Solid by default: this
    /// is ground, not a diagnostic. X-ray is a dim neutral ghost to keep the tank prominent;
    /// hidden clears close-up clutter. Visibility/material only; colliders and analytic terrain
    /// stay unchanged, preserving physics.
    world: MeshState,
    /// Scene-root glb running gear bound by [`wheel_view`]. Like the shoes, it overrides
    /// inherited visibility for "model off, running gear on" tooth-mesh inspection.
    wheels: bool,
    /// The conformed belt line (the drawn pin line).
    belt_line: bool,
    /// The outer-face companion line.
    outer: bool,
    /// The hub marker spheres.
    hubs: bool,
    /// The contact dots (load-sized, slip-coloured).
    dots: bool,
    /// The contact-normal lines.
    normals: bool,
    /// Force vectors per contact: support (magenta) + traction (orange), N-scaled.
    forces: bool,
    /// The collocation stations at the *physics* ring (where the physics thinks the shoes are, vs
    /// the drawn view).
    casts: bool,
    /// Avian collider wireframes (hull box, bump stops, side plates, terrain).
    colliders: bool,
    /// The taut reference loop (the belt's rest path, vs the conformed/solved view).
    reference: bool,
    /// [`link_view`] instances the asset's shoe at each material link station.
    /// On at boot: shoes are the track, not a diagnostic.
    links: bool,
    /// [`wheel_view`] motion: suspension travel, belt-derived spin, tooth-locked sprocket phase.
    /// Off parks nodes at authored poses for comparison with the derived-rest suspension overlay.
    /// Unlike other layers, changes motion, not visibility. On at boot: wheels should turn.
    running_gear: bool,
}

impl Default for VizLayers {
    fn default() -> Self {
        Self {
            // X-ray exposes rest/droop routes instead of burying them in the hull.
            hull: MeshState::Xray,
            collider_volumes: VolumeState::Hidden,
            ballistic_volumes: VolumeState::Hidden,
            world: MeshState::Solid,
            wheels: true,
            // The line is diagnostic; the shoes are the track.
            belt_line: false,
            outer: false,
            hubs: false,
            dots: false,
            normals: false,
            forces: false,
            casts: false,
            colliders: false,
            reference: false,
            links: true,
            running_gear: true,
        }
    }
}

/// Wheel visibility uses `Visible` rather than `Inherited` so a hidden hull cannot hide the
/// running gear during tooth-mesh inspection ([`VizLayers::wheels`]). [`mesh_layers`] owns hull
/// visuals. Covers mesh-less [`RigWheel`] carriers and rendered [`wheel_view`] glb nodes;
/// the latter also seed the override when bound, before this system can run.
///
/// Re-assert every frame: an edge-triggered mirror loses to async glb instantiation, hot reload,
/// or [`build_rig`] spawning after a toggle. `set_if_neq` avoids writes/visibility propagation
/// unless needed, as in [`crate::sandbox::apply_layer_visibility`].
fn apply_mesh_visibility(
    viz: Res<VizLayers>,
    mut wheels: Query<&mut Visibility, Or<(With<RigWheel>, With<wheel_view::GearNode>)>>,
) {
    let wheel_vis = if viz.wheels {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    for mut v in &mut wheels {
        v.set_if_neq(wheel_vis);
    }
}

/// Collider wireframes start disabled; the panel's collider layer enables them.
fn configure_collider_gizmos(mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<PhysicsGizmos>().0.enabled = false;
}

fn sync_collider_gizmos(viz: Res<VizLayers>, mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<PhysicsGizmos>().0.enabled = viz.colliders;
}

/// World-space taut rest path around articulated wheels, requested through
/// [`wrap::WrapInput::reference`](crate::track::wrap::WrapInput), written by
/// [`conform_belts_field`] and drawn by the panel's reference layer. Deviations show terrain/slack
/// effects. Diagnostic only: the game never requests it.
#[derive(Resource, Default)]
pub(super) struct TautReference {
    pub(super) left: Vec<Vec3>,
    pub(super) right: Vec<Vec3>,
}

fn draw_taut_reference(mut gizmos: Gizmos, reference: Res<TautReference>, viz: Res<VizLayers>) {
    if !viz.reference {
        return;
    }
    for pts in [&reference.left, &reference.right] {
        if pts.len() < 2 {
            continue;
        }
        gizmos.linestrip(
            pts.iter().copied().chain(pts.first().copied()),
            Color::srgb_from_array(scene().reference_srgb.0),
        );
    }
}

/// Which reset spot `R` will drop the rig at next (index into [`reset_spots`]).
#[derive(Resource, Default)]
struct ResetSpot(usize);

/// Reset witnesses follow the declared trenches; adding one needs no Rust index or label.
fn reset_spots() -> Vec<(f32, f32, &'static str)> {
    let mut spots = vec![(0.0, 0.0, "flat ground")];
    spots.extend(
        scene()
            .trenches
            .iter()
            .map(|trench| (0.0, -trench.forward_m.0, trench.label.as_str())),
    );
    let spot = rubble::rubble_spot();
    spots.push((
        spot.x,
        spot.y,
        "rubble yard (brick pile ahead, wall section on the left)",
    ));
    spots
}

/// Unshaped throttle/steer in [-1, 1] from arrows (WASD stays with the camera) or harness.
/// The fixed-tick adapter uses the game's [`crate::track::drive::shape_drive`], including slew
/// in the harness-tested path.
#[derive(Resource, Default)]
struct RawDriveInput(crate::track::drive::DriveAxes);

/// The slewed drive state — the sandbox's analogue of the game's `DriveSignal`,
/// advanced on the FIXED tick by the force adapter (never in `Update`: frame-rate-independent
/// shaping is half of what makes harness runs bit-repeatable).
#[derive(Resource, Default)]
struct ShapedDrive(crate::track::drive::DriveAxes);

/// Per-track belt surface speed (m/s, + = drives the tank forward): the integrated state of the
/// slip model. Positive when the track is laying ground backward under the hull.
#[derive(Resource, Default)]
struct BeltSpeed(PerSide<f32>);

impl BeltSpeed {
    fn get(&self, side: Side) -> f32 {
        *self.0.get(side)
    }
    fn set(&mut self, side: Side, value: f32) {
        *self.0.get_mut(side) = value;
    }
}

/// `Esc` toggles cursor capture and pauses Avian dynamics for inspection/screenshots. [`read_drive_input`] clears raw
/// input while the cursor is free; [`cursor_locked`] gates flying.
fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut physics: ResMut<Time<Physics>>,
    mut paused: ResMut<Paused>,
    mut raw: ResMut<RawDriveInput>,
    mut shaped: ResMut<ShapedDrive>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    for (mut window, mut cursor) in &mut windows {
        if cursor.grab_mode == CursorGrabMode::Locked {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
            physics.pause();
            paused.0 = true;
            // The paused force adapter cannot slew this down; clear it to avoid stale thrust
            // on resume.
            raw.0 = crate::track::drive::DriveAxes::default();
            shaped.0 = crate::track::drive::DriveAxes::default();
        } else {
            let center = window.size() / 2.0;
            window.set_cursor_position(Some(center));
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
            physics.unpause();
            paused.0 = false;
        }
    }
}

fn spawn_camera(mut commands: Commands) {
    // A side-on-ish vantage so the belt profile (the Z–Y plane) and its envelope read at a glance.
    commands
        .spawn((
            Camera3d::default(),
            Transform::from_translation(Vec3::from_array(scene().camera_eye_m.0))
                .looking_at(Vec3::from_array(scene().camera_target_m.0), Vec3::Y),
            FreeFlyCam,
            // `view` and belt selection use this camera. The overlay shares its pose and
            // carries no PlayerView: it is not another world view.
            crate::view::PlayerView,
        ))
        .with_children(|parent| {
            // Child overlay shares the fly pose and draws after the main pass, without clear,
            // on `OVERLAY_LAYER`. Its own depth buffer composites on-top volumes over the scene
            // even inside the hull, as in `crate::sandbox`.
            parent.spawn((
                Camera3d::default(),
                Camera {
                    order: 1,
                    clear_color: bevy::camera::ClearColorConfig::None,
                    ..default()
                },
                bevy::camera::visibility::RenderLayers::layer(mesh_layers::OVERLAY_LAYER),
                mesh_layers::OverlayCamera,
            ));
        });
}

/// Lock + hide the cursor for mouse-look (a query, so a not-yet-present cursor is a no-op).
fn grab_cursor(mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>) {
    for (mut window, mut cursor) in &mut windows {
        let center = window.size() / 2.0;
        window.set_cursor_position(Some(center));
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

/// Lighting and a deterministic −Z course: trenches, step, ramp and washboards.
/// All blocks use `Terrain`, so belt contact sees known geometry uniformly.
fn spawn_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        DirectionalLight {
            illuminance: scene().light_lux.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_translation(Vec3::from_array(scene().light_eye_m.0))
            .looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let ground_mat = materials.add(Color::srgb_from_array(scene().ground_srgb.0));
    let obstacle_mat = materials.add(Color::srgb_from_array(scene().obstacle_srgb.0));

    // Every block also lands in the analytic terrain field (the belt's oracle) — colliders and
    // field are built from the same transforms, so the two representations cannot drift.
    let mut field: Vec<TerrainBlock> = Vec::new();
    // Every block is terrain, so each carries the game terrain's friction.
    let ground_friction = crate::ground_types::GroundTypes::load_shipped().terrain_friction();

    let block = |commands: &mut Commands,
                 field: &mut Vec<TerrainBlock>,
                 transform: Transform,
                 mat: &Handle<StandardMaterial>| {
        field.push(TerrainBlock::new(
            transform.translation,
            transform.rotation,
            transform.scale,
        ));
        commands.spawn((
            Mesh3d(cube.clone()),
            MeshMaterial3d(mat.clone()),
            transform,
            RigidBody::Static,
            Collider::cuboid(1.0, 1.0, 1.0),
            crate::track::terrain::FieldSurface,
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
            ground_friction,
        ));
    };
    // A ground slab spanning z_hi..z_lo (z_hi > z_lo), top face at y=0.
    let ground = |commands: &mut Commands, field: &mut Vec<TerrainBlock>, z_hi: f32, z_lo: f32| {
        block(
            commands,
            field,
            Transform::from_xyz(0.0, -scene().slab_thickness_m.0 / 2.0, (z_hi + z_lo) / 2.0)
                .with_scale(Vec3::new(
                    scene().lane_width_m.0,
                    scene().slab_thickness_m.0,
                    z_hi - z_lo,
                )),
            &ground_mat,
        );
    };

    // Walk declared trench gaps nearest→farthest, including their hard floors.
    let mut cursor = scene().lane_near_m.0;
    for trench in &scene().trenches {
        let (tz, tw) = (trench.forward_m.0, trench.width_m.0);
        let near_lip = -(tz - tw / 2.0);
        let far_lip = -(tz + tw / 2.0);
        ground(&mut commands, &mut field, cursor, near_lip);
        block(
            &mut commands,
            &mut field,
            Transform::from_xyz(
                0.0,
                scene().trench_floor_y_m.0 - scene().slab_thickness_m.0 / 2.0,
                -tz,
            )
            .with_scale(Vec3::new(
                scene().lane_width_m.0,
                scene().slab_thickness_m.0,
                tw,
            )),
            &ground_mat,
        );
        cursor = far_lip;
    }
    ground(&mut commands, &mut field, cursor, scene().lane_far_m.0);

    // The declared curb provides a hard vertical climbing edge beyond the trenches.
    block(
        &mut commands,
        &mut field,
        Transform::from_xyz(0.0, scene().step_height_m.0 / 2.0, scene().step_z_m.0).with_scale(
            Vec3::new(
                scene().obstacle_width_m.0,
                scene().step_height_m.0,
                scene().step_depth_m.0,
            ),
        ),
        &obstacle_mat,
    );

    // The declared ramp checks climb/envelope, with a sunk entry and a drop after its crest.
    let (run, thick, deg) = (
        scene().ramp_run_m.0,
        scene().ramp_thickness_m.0,
        scene().ramp_deg.0,
    );
    let (sin, cos) = deg.to_radians().sin_cos();
    let center_y = scene().ramp_entry_y_m.0 - (thick / 2.0) * cos + (run / 2.0) * sin;
    block(
        &mut commands,
        &mut field,
        Transform::from_xyz(0.0, center_y, scene().ramp_z_m.0)
            .with_rotation(Quat::from_rotation_x(deg.to_radians()))
            .with_scale(Vec3::new(scene().obstacle_width_m.0, thick, run)),
        &obstacle_mat,
    );

    // The slope pad (see `slope_pad_pose`).
    block(
        &mut commands,
        &mut field,
        Transform::from_translation(Vec3::from_array(scene().slope_pad_center_m.0))
            .with_rotation(Quat::from_rotation_x(scene().slope_pad_deg.0.to_radians()))
            .with_scale(Vec3::new(
                scene().slope_pad_size_m.0,
                scene().slope_pad_thickness_m.0,
                scene().slope_pad_size_m.0,
            )),
        &obstacle_mat,
    );

    // The runway/turn pad shares the lane's ground datum.
    block(
        &mut commands,
        &mut field,
        Transform::from_xyz(
            scene().runway_center_m.0[0],
            -scene().slab_thickness_m.0 / 2.0,
            scene().runway_center_m.0[2],
        )
        .with_scale(Vec3::new(
            scene().runway_size_m.0[0],
            scene().slab_thickness_m.0,
            scene().runway_size_m.0[1],
        )),
        &ground_mat,
    );

    for &(start, period, bumps, height) in &scene().washboards.0 {
        let thickness = period / scene().washboard_thickness_divisor.0;
        for i in 0..bumps {
            let z = -(start + i as f32 * period);
            block(
                &mut commands,
                &mut field,
                Transform::from_xyz(0.0, height / 2.0, z).with_scale(Vec3::new(
                    scene().obstacle_width_m.0,
                    height,
                    thickness,
                )),
                &obstacle_mat,
            );
        }
    }

    // The sandbox course paints no ground: the base pack's, as the game's flat world.
    let ground = crate::ground_field::GroundField::unpainted(
        &crate::ground_types::GroundTypes::load_shipped(),
    );
    commands.insert_resource(TerrainField {
        revision: None,
        field: Some(BlockField::new(field).with_ground(Some(ground))),
    });
}

/// Run condition for the one-shot [`build_rig`]: the blueprint has landed and the rig is not up yet.
/// `RigGeom` is the latch — `build_rig` inserts it, so this stops matching from the next frame on.
fn rig_unbuilt(geom: Option<Res<RigGeom>>, blueprint: Option<Res<TankBlueprint>>) -> bool {
    geom.is_none() && blueprint.is_some()
}

/// Resolve the sim artifact through the shared asset root, as bake and
/// `track::sim::init_track_gear` do (ADR-0035): the lab must read what the sim reads.
/// `RigGeom::build` re-reads pin pitch, surface offsets and sprocket/idler mesh centroids because
/// the baked node list cannot express scaled-ancestor markers or wheel geometry baked into
/// vertices under identity transforms. The strip retains the node graph and all rung-0 positions,
/// so it answers the view glb's geometry questions.
fn tiger_sim_glb() -> std::path::PathBuf {
    crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH)
}

/// Build once when the blueprint lands: derive geometry, spawn hull/visual/wheel carriers and
/// size belt/grip/drivetrain state. One system publishes everything in one command flush;
/// `resource_exists::<RigGeom>` therefore gates the entire downstream rig without a half-built
/// frame.
fn build_rig(
    mut commands: Commands,
    blueprint: Res<TankBlueprint>,
    asset_server: Res<AssetServer>,
    mut suspension: ResMut<RigSuspension>,
    mut grip_elements: ResMut<BeltGripElements>,
) {
    let spec = &blueprint.spec;
    let track = &spec.track;
    suspension.0 = track.suspension.params();
    // The two AUTHORED counts the rig owns; every other number below is measured or derived from
    // them. `teeth` sets the chord-exact sprocket pitch circle, `link_count` IS the material loop.
    let geom = RigGeom::build(
        &blueprint,
        &tiger_sim_glb(),
        track.sprocket.teeth,
        track.link_count,
        &suspension.0,
    );
    let window = geom.window;
    // No provenance branch: a failed marker read aborts in `marker_model::refuse` rather than falling
    // back to the RON, so a rig that exists was measured off the glb by construction.
    info!(
        "rig: Tiger I (glb markers) — {} road wheels/side, pitch {:.4} m × {} links = {:.3} m loop \
         vs a {:.3} m rest wrap (slack {:+.3} m, droop limiter {:?}; n_min {} = the hard floor, \
         n_droop {} = the chain→spring crossover, which is SOFT — above it the belt simply carries \
         slack, as a real tensioned track does); ride height {:.3} m, track plane ±{:.3} m",
        geom.wheel_count(Side::Right),
        geom.pitch,
        geom.link_count,
        geom.belt_len(),
        geom.taut_perimeter(Side::Right, Pose::Rest, &suspension.0),
        window.slack_rest,
        window.limiter,
        window.n_min,
        window.n_droop,
        geom.hull_rest_y,
        geom.plane_x,
    );
    // The bake’s RigGeom construction refuses an unreachable authored loop before this spawn.
    geom.validate(&suspension.0)
        .expect("the bake validates the authored running gear before publishing its blueprint");

    commands.insert_resource(RigSpec {
        weight_n: spec.mass * derive::G,
        engage_depth: track.suspension.engage,
        belt_inertia: track
            .reflected_inertia(&spec.roadwheels, geom.wrap_radii())
            .expect("running-gear admission certified its construction inertia"),
        grip: crate::track::forces::GripParams::of(
            spec,
            crate::track::rig_geom::tank_ground_pressure_kpa(&geom),
            crate::track::rig_geom::com_rest_height_m(&blueprint.geometry, &geom),
        ),
    });

    let envelope = belt::calibrate_envelope(
        &geom,
        &suspension.0,
        spec.mass * derive::G,
        track.suspension.engage,
    );
    commands.insert_resource(envelope.segment_law());
    commands.insert_resource(envelope);

    // Use the game's validated transmission seam and DERIVED chord-exact sprocket pitch
    // radius (from pin pitch and tooth count) (`geom.rest`'s first circle) + half-tread (`geom.plane_x`). The rejected τ-arc
    // radius `pitch·teeth/τ` is feel-neutral (reductions derive against it, so it cancels),
    // but does not represent the pin polygon.
    let transmission = RigTransmission(
        track
            .transmission_params(&spec.roadwheels, geom.wrap_radii(), geom.plane_x)
            .expect("the Tiger's transmission block is validated at bake"),
    );
    commands.insert_resource(TransState::fresh(&transmission.0));
    commands.insert_resource(transmission);
    // Size grip slabs at force resolution; see `BeltGripElements` for the invariant.
    let pin_belt = PinBelt::for_rig(&geom);
    *grip_elements = BeltGripElements::sized();
    commands.insert_resource(pin_belt);

    // Authored full x/y/z inertia extents govern mass distribution/handling, not collision
    // silhouette. Sharing the box would make yaw retuning move the collision hull (counterexample:
    // CLAIMED 3.0 m inertia width vs ±1.89 m track span). Collision uses authored proxies below.
    let inertia_box = Cuboid::new(
        spec.inertia_extents.0,
        spec.inertia_extents.1,
        spec.inertia_extents.2,
    );

    // Check proxies before spawn; `report_collision_proxies` owns the constraints.
    report_collision_proxies(&blueprint, &geom);

    // Rotate the data carriers' Y-up axle onto X (+90° about Z).
    let axle = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

    let hull = commands
        .spawn((
            Hull,
            crate::track::sim::TrackDrive::default(),
            // At `hull_rest_y` the rest belt exactly kisses flat ground — the datum `RigGeom`
            // derives from the rest envelope's own lowest point, not an authored ride height.
            Transform::from_xyz(0.0, geom.hull_rest_y, 0.0),
            // Supply visibility inheritance to the render-only glb child.
            Visibility::default(),
            crate::eased_dynamic_body(),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            // Mass properties are authored, not derived from the colliders (`NoAuto*`), exactly as
            // the game does: the spec's combat mass in the spec's inertia box.
            Mass(spec.mass),
            AngularInertia::from_shape(&inertia_box, spec.mass),
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
        ))
        .with_children(|parent| {
            // The game's scene has authored hull-local vertices: an identity child aligns
            // the visual exactly with the derived geometry overlay.
            parent.spawn((
                WorldAssetRoot(
                    asset_server
                        .load(GltfAssetLabel::Scene(0).from_asset(crate::tank::TANK_GLB_PATH)),
                ),
                Transform::IDENTITY,
                Visibility::default(),
            ));

            // Authored `*_Collider` proxies declared in tank data, built through the game's
            // `MeshGeometry::convex_hull_colliders`: node-local POSITION buffers, one hull per
            // loose part. Matching collision construction makes lab feel verdicts valid in game.
            // Downward belt rays cannot resist vertical walls; proxies also catch failed trench
            // bridges. On normal terrain they clear the ground and the belt carries all weight.
            for &index in &blueprint.geometry.collision_proxies {
                let node = &blueprint.geometry.nodes[index];
                for primitive in &node.primitives {
                    let colliders = primitive.convex_hull_colliders().unwrap_or_else(|| {
                        panic!(
                            "collision proxy `{}` has a degenerate hull source",
                            node.name
                        )
                    });
                    for collider in colliders {
                        parent.spawn((
                            collider,
                            // Flatten the baked root-relative pose: this render-only glb has no
                            // physics node chain. The game uses Avian `ColliderTransform` through
                            // real nodes. Translation/rotation flattening is exact only without
                            // scale; `report_collision_proxies` checks every ancestor (the shipped
                            // Hull→Hull_Collider and Hull→Turret_Yaw→Turret_Collider chains are
                            // identity TRS). Turret yaw stays at rest: this tool has no traverse.
                            Transform::from_translation(node.root_position)
                                .with_rotation(node.root_rotation),
                            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
                            // Its substance's, as the game's proxies carry.
                            crate::tank::proxy_friction(spec, &node.name),
                        ));
                    }
                }
            }

            for side in Side::ALL {
                // The stored rest list is `[sprocket, road wheels…, idler]` on the PIN line, which
                // is exactly what the drive wheels need: the sprocket's chord-exact pitch circle and
                // the idler's measured rim pushed out to the pins.
                let rest = geom.rest.get(side);
                let sprocket = rest[0];
                let idler = *rest.last().expect("a side has a sprocket and an idler");

                // Mesh-less, collider-less road-wheel carriers: `Suspension` for cosmetics,
                // `RigWheel` for lookup. The glb renders wheels and the belt carries ground load.
                // Preserve each node's lateral x: interleaved rows do not all sit on the track plane.
                let authored_side = match side {
                    Side::Left => crate::tank::TrackSide::Left,
                    Side::Right => crate::tank::TrackSide::Right,
                };
                for &(index, _) in blueprint
                    .geometry
                    .roadwheels
                    .iter()
                    .filter(|(_, side)| *side == authored_side)
                {
                    let node = &blueprint.geometry.nodes[index];
                    let centre = node.root_position;
                    parent.spawn((
                        StationAddress(node.name.clone()),
                        RoadTread(
                            spec.roadwheels
                                .iter()
                                .find(|wheel| wheel.node == node.name)
                                .expect(
                                    "the bake's roadwheel table comes from the spec declaration",
                                )
                                .tread
                                .clone(),
                        ),
                        RigWheel {
                            side,
                            kind: WheelKind::Road,
                        },
                        Transform::from_translation(centre),
                        Suspension {
                            pivot_local: centre,
                            dy: 0.0,
                            dvel: 0.0,
                        },
                    ));
                }

                // Sprocket/idler data carriers for the hub-gizmo layer — NO colliders. The RED
                // bump-stop prism (`track::bump_stop`) is the whole rig's penetration backstop,
                // its end arcs riding the unsprung wheels' outer shoe surface.
                for (kind, (centre, _radius)) in
                    [(WheelKind::Sprocket, sprocket), (WheelKind::Idler, idler)]
                {
                    parent.spawn((
                        RigWheel { side, kind },
                        // Side-plane `(z, y)` back to 3-D on this side's track plane.
                        Transform::from_xyz(geom.side_plane_x(side), centre.y, centre.x)
                            .with_rotation(axle),
                    ));
                }
            }
        })
        .id();

    // The bump stops and side plates, through the constructor every game tank's body uses
    // ([`spawn_track_colliders`]), cut at the live knobs' travel. Spawned here so the rig is never up
    // for a frame without its bottoming stop; [`refresh_track_colliders`] recuts them on every retune.
    spawn_track_colliders(
        &mut commands,
        hull,
        &TrackColliders::new(&geom, &suspension.0, track.friction()),
    );

    // Publish count intent with geometry; see `RigCounts` for initial-insert handling.
    commands.insert_resource(RigCounts {
        link_count: geom.link_count,
        teeth: geom.teeth,
    });
    commands.insert_resource(geom);
}

/// Check flattened proxies and log their ground clearance.
/// `NodeGeometry::root_position` / `root_rotation` omit scale, so scaled ancestors would silently
/// mis-size/mis-place the sandbox's flattened proxies. The game composes real nodes through Avian;
/// this constraint is specific to the sandbox.
/// A proxy below the rest belt would carry the tank and replace belt contact with a box on a floor.
/// Log its rest bottom above y = 0 every run so that cause of a dead-feeling belt is visible.
fn report_collision_proxies(blueprint: &TankBlueprint, geom: &RigGeom) {
    let nodes = &blueprint.geometry.nodes;
    for &index in &blueprint.geometry.collision_proxies {
        let node = &nodes[index];
        // Walk the chain to the scene root; any non-identity scale invalidates the flattening.
        let mut ancestor = Some(index);
        while let Some(i) = ancestor {
            let scale = nodes[i].transform.scale;
            assert!(
                (scale - Vec3::ONE).abs().max_element() < 1e-4,
                "collision proxy `{}` sits under a scaled node (`{}` scale {scale:?}): the sandbox \
                 flattens proxies to their baked root-relative pose, which carries no scale — hang \
                 them under real node entities (as `tank::spawn` does) before authoring scale here",
                node.name,
                nodes[i].name,
            );
            ancestor = nodes[i].parent;
        }
        // Hull-local AABB over the raw hull source, then the rest-pose world height of its floor.
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for primitive in &node.primitives {
            for p in &primitive.positions {
                let p = node.root_rotation * Vec3::from(*p) + node.root_position;
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        info!(
            "rig: collision proxy `{}` — hull-local x∈[{:.3},{:.3}] y∈[{:.3},{:.3}] z∈[{:.3},{:.3}]; \
             floor sits {:.3} m above flat ground at rest",
            node.name,
            lo.x,
            hi.x,
            lo.y,
            hi.y,
            lo.z,
            hi.z,
            lo.y + geom.hull_rest_y,
        );
    }
}

/// Recut on geometry or bump-stop changes. Convex hulls are not mutable fields and retunes
/// are rare, so despawn/rebuild is cheap. Like [`refresh_envelope`], unordered against
/// [`apply_rig_counts`]: changes appear this frame or next.
fn refresh_track_colliders(
    mut commands: Commands,
    geom: Res<RigGeom>,
    suspension: Res<RigSuspension>,
    rig: Res<RigSpec>,
    hull: Query<Entity, With<Hull>>,
    existing: Query<Entity, Or<(With<BumpStop>, With<SidePlate>)>>,
) {
    if !(geom.is_changed() || suspension.is_changed()) {
        return;
    }
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    spawn_track_colliders(
        &mut commands,
        hull.single()
            .expect("RigGeom is published with exactly one sandbox hull in the same command flush"),
        &TrackColliders::new(&geom, &suspension.0, rig.grip.track),
    );
}

/// State indexed by loop count or travel phase. Teleports ([`reset_rig`]) and count changes
/// ([`apply_rig_counts`]) invalidate the same set; bundling owns that list once and keeps
/// `reset_rig` under Bevy's 16-param ceiling.
#[derive(bevy::ecs::system::SystemParam)]
struct BeltState<'w> {
    phase: ResMut<'w, BeltPhase>,
    contacts: ResMut<'w, BeltContacts>,
    dynamics: ResMut<'w, SideDynamics>,
    grip: ResMut<'w, BeltGrip>,
    elements: ResMut<'w, BeltGripElements>,
    wrap: ResMut<'w, WrapMemory>,
}

impl BeltState<'_> {
    /// Cold state sized for force resolution (`BeltGripElements` owns the slab invariant).
    /// Keep speed, drive command, transmission gear and hull pose: they are not loop-indexed,
    /// and a count tweak must not stop a rolling tank. [`reset_rig`] clears them for a teleport.
    fn reseat(&mut self) {
        *self.phase = BeltPhase::default();
        // Stale contacts/dynamics display the pre-teleport tick, and
        // stale bristle strain is shear the pads never took.
        *self.contacts = BeltContacts::default();
        *self.dynamics = SideDynamics::default();
        *self.grip = BeltGrip::default();
        *self.elements = BeltGripElements::sized();
        // Pose and loop both invalidate filter cells; re-init rather than settle from stale ones.
        self.wrap.reset();
    }
}

/// Panel Scene "Reset tank" intent consumed by [`reset_rig`], sharing `R`'s executor without
/// borrowing all reset state in egui. Always present for a `dev_ui`-independent signature.
#[derive(Resource, Default)]
struct ResetRequested(bool);

/// `R` or [`ResetRequested`] cycles [`reset_spots`], dropping the rig at rest.
#[expect(
    clippy::too_many_arguments,
    reason = "a reset spanning input, the hull, belt, drive, transmission, rig and wheels"
)]
fn reset_rig(
    keys: Res<ButtonInput<KeyCode>>,
    mut requested: ResMut<ResetRequested>,
    mut hull: Query<
        (
            &mut Transform,
            &mut LinearVelocity,
            &mut AngularVelocity,
            &mut crate::track::sim::TrackDrive,
        ),
        With<Hull>,
    >,
    mut spot: ResMut<ResetSpot>,
    mut belt: ResMut<BeltSpeed>,
    mut belt_state: BeltState,
    mut raw: ResMut<RawDriveInput>,
    mut shaped: ResMut<ShapedDrive>,
    transmission: Res<RigTransmission>,
    mut trans_state: ResMut<TransState>,
    geom: Res<RigGeom>,
    mut wheels: Query<&mut Suspension>,
) {
    // Touch `ResetRequested` (a `ResMut` deref) ONLY on a frame that actually resets — an
    // unconditional clear would mark the resource changed every frame for nothing.
    if !keys.just_pressed(KeyCode::KeyR) && !requested.0 {
        return;
    }
    requested.0 = false;
    let spots = reset_spots();
    let (x, z, label) = spots[spot.0];
    spot.0 = (spot.0 + 1) % spots.len();
    let (mut transform, mut lin, mut ang, mut motion) = hull
        .single_mut()
        .expect("RigGeom is published with exactly one sandbox hull in the same command flush");
    *transform = Transform::from_xyz(x, geom.hull_rest_y, z);
    lin.0 = Vec3::ZERO;
    ang.0 = Vec3::ZERO;
    *belt = BeltSpeed::default();
    *motion = crate::track::sim::TrackDrive::default();
    // A reset is a fresh at-rest rig: stale command state would re-thrust it immediately.
    raw.0 = crate::track::drive::DriveAxes::default();
    shaped.0 = crate::track::drive::DriveAxes::default();
    belt_state.reseat();
    // A fresh rig is in 1st gear with no shift in flight.
    *trans_state = TransState::fresh(&transmission.0);
    // Stale cosmetic wheel lift survives the teleport otherwise: for the first CLAIMED ~100 ms the
    // conform solves against phantom raised wheel circles while the hull settles.
    for mut susp in &mut wheels {
        susp.dy = 0.0;
        susp.dvel = 0.0;
    }
    info!("reset → {label} (x = {x:.1}, z = {z:.1})");
}

/// ASSUMED inspection band above the spring-limited count, used by the panel and count keys.
/// The shared rig gate still refuses any tuning outside the contact law's physical domain.
const LINK_TUNE_SLOP: i32 = 12;

/// Panel/keyboard count intent; [`apply_rig_counts`] owns rebuilding so egui needs no heavy
/// `RigGeom` / `PinBelt` / [`BeltState`] mutable borrows. `Copy + PartialEq` enables write-on-change.
/// Default 0/0 exists before the rig so panel borrows remain valid. [`build_rig`] replaces it with
/// authored counts in the geometry's flush; the commit skips the insert or already-matching counts.
#[derive(Resource, Clone, Copy, PartialEq, Default)]
struct RigCounts {
    link_count: usize,
    teeth: u32,
}

/// The panel and keyboard’s inspection band, derived from the current wheel geometry.
/// Counts below the geometric minimum are unavailable; the shared rig validator also checks travel.
fn clamp_link_count(geom: &RigGeom, params: &SuspensionParams, desired: i32) -> usize {
    let window = geom.link_window(params);
    desired.clamp(window.n_min as i32, window.n_droop as i32 + LINK_TUNE_SLOP) as usize
}

/// A closed pin polygon needs at least three vertices. The rig validator checks the resulting
/// geometry; an asset-specific upper tooth count is not a physical limit.
fn clamp_teeth(desired: i32) -> u32 {
    desired.max(3) as u32
}

/// `;`/`'` tune links; `n`/`m` tune teeth. Commit chosen `track.link_count` /
/// `track.sprocket.teeth` by hand in `assets/tiger_1/tiger_1.tank.ron`: no save keybind, because
/// the file also configures the game and committing it is the user's decision.
///
/// Writes [`RigCounts`]; [`apply_rig_counts`] rebuilds. Clamp only the count whose key was
/// pressed: a tooth edit must not snap a link count left outside the band by suspension tuning.
/// Write-on-change prevents idle rebuilds.
fn tune_rig_counts(
    keys: Res<ButtonInput<KeyCode>>,
    suspension: Res<RigSuspension>,
    geom: Res<RigGeom>,
    mut counts: ResMut<RigCounts>,
) {
    let step = |down, up| i32::from(keys.just_pressed(up)) - i32::from(keys.just_pressed(down));
    let d_links = step(KeyCode::Semicolon, KeyCode::Quote);
    let d_teeth = step(KeyCode::KeyN, KeyCode::KeyM);
    if d_links == 0 && d_teeth == 0 {
        return;
    }
    let params = &suspension.0;
    let mut next = *counts;
    if d_links != 0 {
        next.link_count = clamp_link_count(&geom, params, counts.link_count as i32 + d_links);
    }
    if d_teeth != 0 {
        next.teeth = clamp_teeth(counts.teeth as i32 + d_teeth);
    }
    if next != *counts {
        *counts = next;
    }
}

/// Rebuild [`RigGeom`] without re-reading the glb ([`RigGeom::rebuild`]); validate construction,
/// then update inertia/transmission, geometry, [`PinBelt`] (pitch × link_count length,
/// link_count stations), and loop-indexed state via [`BeltState::reseat`].
/// Geometry keeps length, sprocket circle, ride height and window
/// verdict coherent; grip slabs must match the loop and never resize in the force law.
/// Skip the initial insert and matching/unchanged counts to avoid per-frame rebuilds.
/// [`refresh_envelope`] / [`refresh_track_colliders`] consume the geometry change.
#[expect(
    clippy::too_many_arguments,
    reason = "one atomic retune of geometry, drive construction and belt state"
)]
fn apply_rig_counts(
    mut counts: ResMut<RigCounts>,
    suspension: Res<RigSuspension>,
    mut geom: ResMut<RigGeom>,
    mut pin_belt: ResMut<PinBelt>,
    mut belt_state: BeltState,
    blueprint: Res<TankBlueprint>,
    mut rig: ResMut<RigSpec>,
    mut transmission: ResMut<RigTransmission>,
) {
    if counts.is_added() || !counts.is_changed() {
        return;
    }
    if counts.link_count == geom.link_count && counts.teeth == geom.teeth {
        return;
    }
    let params = &suspension.0;
    let next = geom.rebuild(counts.teeth, counts.link_count, params);
    if let Err(error) = next.validate(params) {
        warn!("track sandbox: refused tuning: {error}");
        *counts = RigCounts {
            teeth: geom.teeth,
            link_count: geom.link_count,
        };
        return;
    }
    let mut track = blueprint.spec.track.clone();
    track.link_count = counts.link_count;
    track.sprocket.teeth = counts.teeth;
    let wrap = next.wrap_radii();
    let derived = track.transmission_params(&blueprint.spec.roadwheels, wrap, next.plane_x);
    let derived = match derived {
        Ok(derived) => derived,
        Err(error) => {
            warn!("track sandbox: refused construction tuning: {error}");
            *counts = RigCounts {
                teeth: geom.teeth,
                link_count: geom.link_count,
            };
            return;
        }
    };
    rig.belt_inertia = track
        .reflected_inertia(&blueprint.spec.roadwheels, wrap)
        .expect("the retuned transmission admitted the same construction inertia");
    transmission.0 = derived;
    *geom = next;
    *pin_belt = PinBelt::for_rig(&geom);
    belt_state.reseat();
    // Keep tuning verdicts in the log for choosing values to commit to RON.
    let window = geom.window;
    info!(
        "tuned: {} teeth, {} links = {:.3} m loop vs a {:.3} m rest wrap (slack {:+.3} m, \
         {:?}-limited; n_min {} = hard floor, n_droop {} = the soft chain→spring crossover)",
        geom.teeth,
        geom.link_count,
        geom.belt_len(),
        geom.taut_perimeter(Side::Right, Pose::Rest, params),
        window.slack_rest,
        window.limiter,
        window.n_min,
        window.n_droop,
    );
}

/// `L` logs exact model diagnostics beside the panel: height, grounded stations, support/weight,
/// belt vs hull speed (slip/wheelspin), taut-implied vs authored links and derived mass.
/// [`derive::link_count`] and model mass feed both displays.
fn log_state(
    keys: Res<ButtonInput<KeyCode>>,
    hull: Query<(&Transform, &LinearVelocity), With<Hull>>,
    contacts: Res<BeltContacts>,
    belt: Res<BeltSpeed>,
    rig: Res<RigSpec>,
    geom: Res<RigGeom>,
    suspension: Res<RigSuspension>,
) {
    if !keys.just_pressed(KeyCode::KeyL) {
        return;
    }
    let (transform, lin) = hull
        .single()
        .expect("RigGeom is published with exactly one sandbox hull in the same command flush");
    let count = contacts.all().count();
    let total: f32 = contacts.all().map(|c| c.load).sum();
    let weight = rig.weight_n;
    let speed = lin.0.dot(transform.forward().into());
    let taut = geom.taut_perimeter(Side::Right, Pose::Rest, &suspension.0);
    let implied_links = derive::link_count(taut, geom.pitch);
    info!(
        "hull y = {:.3} m | stations = {count} | support = {:.0}% of weight | belt L/R = {:.1}/{:.1} m/s | tank = {:.1} m/s",
        transform.translation.y,
        100.0 * total / weight,
        belt.get(Side::Left),
        belt.get(Side::Right),
        speed,
    );
    info!(
        "model: mass {:.0} kg | taut wrap {taut:.3} m -> {implied_links} links (authored {})",
        geom.model.mass, geom.link_count,
    );
}

/// Present hubs and [`ConformedBelts`]: terrain-raised lower run, drive-wheel arcs and sagging
/// return. The procedural links follow this same path.
fn draw_rig_gizmos(
    mut gizmos: Gizmos,
    wheels: Query<(&RigWheel, &GlobalTransform)>,
    belts: Res<ConformedBelts>,
    hull: Query<&GlobalTransform, With<Hull>>,
    viz: Res<VizLayers>,
    geom: Res<RigGeom>,
) {
    // Distinguish sprocket/idler hubs from road wheels by colour.
    if viz.hubs {
        for (wheel, gt) in &wheels {
            let color = match wheel.kind {
                WheelKind::Road => Color::srgb_from_array(scene().hub_srgb.0),
                WheelKind::Sprocket | WheelKind::Idler => {
                    Color::srgb_from_array(scene().drive_hub_srgb.0)
                }
            };
            gizmos.sphere(
                Isometry3d::from_translation(gt.translation()),
                scene().hub_radius_m.0,
                color,
            );
        }
    }

    for side in Side::ALL {
        if viz.belt_line {
            let mut world = belts.get(side).iter().map(|s| s.world);
            gizmos.linestrip(world.clone(), Color::srgb_from_array(scene().belt_srgb.0));
            if let (Some(a), Some(b)) = (world.next_back(), world.next()) {
                gizmos.line(a, b, Color::srgb_from_array(scene().belt_srgb.0));
            }
        }

        // The outward normal follows neighbour tangents; construction owns the face offset.
        // The dimmer outer-face companion exposes shoe thickness: dark on ground, wheels on light.
        if !viz.outer {
            continue;
        }
        let samples = belts.get(side);
        let n = samples.len();
        if n < 3 {
            continue;
        }
        let affine = hull
            .single()
            .expect("RigGeom is published with exactly one sandbox hull in the same command flush")
            .affine();
        let track_x = geom.side_plane_x(side);
        let outer: Vec<Vec3> = (0..n)
            .map(|i| {
                let tan2 = (samples[(i + 1) % n].local - samples[(i + n - 1) % n].local)
                    .try_normalize().expect("ConformedBelts samples a positive-pitch closed route with distinct neighbouring stations");
                let out2 = Vec2::new(tan2.y, -tan2.x);
                let p = samples[i].local + out2 * geom.model.pin_to_outer;
                affine.transform_point3(Vec3::new(track_x, p.y, p.x))
            })
            .collect();
        gizmos.linestrip(
            outer.iter().copied().chain(outer.first().copied()),
            Color::srgb_from_array(scene().belt_outer_srgb.0),
        );
    }
}

/// Contact dots: load-sized, slip-coloured (green grip → red slide/wheelspin), placed from the
/// current hull pose to follow interpolation. Show support normals and N-scaled force arrows:
/// support along the normal, traction in the contact plane.
fn draw_contacts(
    mut gizmos: Gizmos,
    hull: Query<&GlobalTransform, With<Hull>>,
    contacts: Res<BeltContacts>,
    viz: Res<VizLayers>,
    geom: Res<RigGeom>,
    law: Res<belt::EnvelopeLaw>,
) {
    if !(viz.dots || viz.normals || viz.forces) {
        return;
    }
    let hull = hull
        .single()
        .expect("RigGeom is published with exactly one sandbox hull in the same command flush");
    // Per-station stiffness (N/m), from the per-metre-of-belt law.
    let k = law.stiffness_per_m * geom.pitch;
    for c in contacts.all() {
        let p = hull.transform_point(c.local);
        // elastic load / k ≈ the station's penetration (m) — a stable size cue (the damped
        // actual load would add velocity-driven size flicker).
        let r = scene().dot_min_radius_m.0
            + (c.load_elastic / k).clamp(0.0, scene().dot_max_growth_m.0);
        if viz.dots {
            // Slip fraction 0→1 grades green to red at the display scale `scene().slip_saturation_mps.0`.
            let t = (c.slip.abs() / scene().slip_saturation_mps.0).clamp(0.0, 1.0);
            let color = Color::srgb(t, 1.0 - scene().slip_green_fall.0 * t, scene().slip_blue.0);
            gizmos.sphere(Isometry3d::from_translation(p), r, color);
        }
        if viz.normals {
            gizmos.line(
                p,
                p + c.normal * (scene().normal_stub_m.0 + r),
                Color::srgb_from_array(scene().normal_srgb.0),
            );
        }
        if viz.forces {
            gizmos.arrow(
                p,
                p + c.normal * (c.load * force_viz_scale()),
                Color::srgb_from_array(scene().support_force_srgb.0),
            );
            if c.tangential_force.length_squared() > scene().traction_min_n_squared.0 {
                gizmos.arrow(
                    p,
                    p + c.tangential_force * force_viz_scale(),
                    Color::srgb_from_array(scene().traction_force_srgb.0),
                );
            }
        }
    }
}

/// The taut lower run: chain the lower external tangents between consecutive circles (front→rear),
/// yielding an ordered polyline of belt-surface points in the side plane.
fn lower_run_polyline(circles: &[(Vec2, f32)]) -> Vec<Vec2> {
    let mut pts = Vec::new();
    for pair in circles.windows(2) {
        let (t0, t1) = external_tangent(pair[0].0, pair[0].1, pair[1].0, pair[1].1, -1.0);
        pts.push(t0);
        pts.push(t1);
    }
    pts
}

/// The full closed belt envelope of one side in the side plane (z, y), ordered CCW: lower run (front
/// → rear) → rear arc wrapping the idler → taut top run (rear → front) → front arc wrapping the
/// sprocket. `circles` must be front→rear. Used by the physics ring, which only samples the lower +
/// front where the belt meets ground — the view systems drape their own sagging top run.
fn belt_loop(circles: &[(Vec2, f32)]) -> Vec<Vec2> {
    let (sprocket_c, sprocket_r) = circles[0];
    let (idler_c, idler_r) = *circles.last().unwrap();
    let (idler_up, sprocket_up) = external_tangent(idler_c, idler_r, sprocket_c, sprocket_r, 1.0);

    let lower = lower_run_polyline(circles);
    let idler_low = *lower.last().unwrap();
    let sprocket_low = lower[0];
    let idler_arc = arc(idler_c, idler_r, idler_low, idler_up, Vec2::new(1.0, 0.0));
    let sprocket_arc = arc(
        sprocket_c,
        sprocket_r,
        sprocket_up,
        sprocket_low,
        Vec2::new(-1.0, 0.0),
    );

    let mut pts = lower;
    pts.extend_from_slice(&idler_arc);
    pts.push(sprocket_up);
    pts.extend_from_slice(&sprocket_arc);
    pts
}

/// Diagnostic metres per newton; the declared taste scales physical force.
fn force_viz_scale() -> f32 {
    1.0 / scene().force_n_per_m.0
}

/// Panel keyboard/pointer focus, written each frame under `dev_ui`. [`panel_capturing`] gates
/// camera/drive input so slider drags and text edits cannot leak into the sim.
#[derive(Resource, Default)]
struct PanelWantsInput {
    keyboard: bool,
    pointer: bool,
}

fn panel_capturing(want: Res<PanelWantsInput>) -> bool {
    want.keyboard || want.pointer
}

/// Read the driver's arrow-key intent as the RAW axes. Zeroed while the cursor is free
/// (paused / unfocused) so a released window doesn't keep driving.
fn read_drive_input(
    keys: Res<ButtonInput<KeyCode>>,
    cursors: Query<&CursorOptions, With<PrimaryWindow>>,
    mut input: ResMut<RawDriveInput>,
) {
    let locked = crate::run_window::primary_or_headless(cursors.single())
        .is_some_and(|c| c.grab_mode == CursorGrabMode::Locked);
    let axis = |pos, neg| keys.pressed(pos) as i8 as f32 - keys.pressed(neg) as i8 as f32;
    (input.0.throttle, input.0.steer) = if locked {
        (
            axis(KeyCode::ArrowUp, KeyCode::ArrowDown),
            axis(KeyCode::ArrowRight, KeyCode::ArrowLeft),
        )
    } else {
        (0.0, 0.0)
    };
}

/// Free-fly the inspection camera. Mouse look (yaw/pitch read from the current rotation), WASD on the
/// heading plane, Shift/Ctrl for altitude — on real time so you can reposition freely.
fn fly_camera(
    mut camera: Query<&mut Transform, With<FreeFlyCam>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time<Real>>,
) {
    let mut transform = camera
        .single_mut()
        .expect("the sandbox spawns exactly one free-fly camera at Startup");
    crate::camera::free_fly_transform(&mut transform, &keys, motion.delta, time.delta_secs());
}

fn cursor_locked(cursors: Query<&CursorOptions, With<PrimaryWindow>>) -> bool {
    crate::run_window::primary_or_headless(cursors.single())
        .is_some_and(|cursor| cursor.grab_mode == CursorGrabMode::Locked)
}
