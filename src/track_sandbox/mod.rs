//! Isolated continuous-track locomotion sandbox (ADR-0005).
//!
//! It owns the course and the belt-contact model (the field-belt, promoted into the game as
//! `track::forces`), so track work can be driven, captured, and A/B'd without the rest of the game
//! in the way.
//!
//! What it does NOT own is the VEHICLE. The sandbox drives the REAL TIGER I, spawned from the same
//! `TankBlueprint` (glb + `.tank.ron`) the game spawns from, with every geometry number coming out
//! of the marker-derived [`rig_geom::RigGeom`] contract: if the lab rig and the shipped tank are
//! different vehicles, a lab verdict is not a game verdict. The isolation is in the ENVIRONMENT — a
//! deterministic course, a scripted harness, and no netcode.
//!
//! One consequence to hold onto: the blueprint is **not available at `Startup`** (the bake inserts it
//! there, so it first becomes visible in `Update`). Everything that needs the rig therefore hangs off
//! a one-shot deferred build ([`build_rig`]) and gates on `resource_exists::<RigGeom>`.
//!
//! # Control surface
//!
//! Every non-driving control lives in one clickable egui [`panel`] (a left `SidePanel` with
//! collapsing Tune / Model / Layers / Telemetry / Scene sections), compiled ONLY under the `dev_ui`
//! feature — `cargo run --bin track_sandbox --features dev_ui` — so egui never reaches the shipping
//! client. The KEYBOARD is exactly the direct-manipulation set:
//! arrow keys drive, `WASD`+mouse+Shift/Ctrl free-fly the camera, `R` cycles the reset spots, `Esc`
//! pauses/frees the cursor, and the live toggles are `T` (transmission), `L` (log) and
//! `;`/`'` `n`/`m` (link/tooth counts) — where a panel widget shares the resource (the
//! transmission selector, the counts stepper) keyboard and panel write it the same way
//! (write-on-change), so they never fight. The panel gates the driving/camera input systems off while
//! an egui widget has focus ([`PanelWantsInput`]).
//!
//! Two intent seams exist so panel and keyboard share one commit path with correct change-ticks:
//! [`RigCounts`] (link/tooth counts; committed by [`apply_rig_counts`]) and [`ResetRequested`]
//! (consumed by [`reset_rig`]). The transmission selector's state reset is centralised the same way
//! in [`reset_trans_on_change`], so a flip from the `T` key or the panel refreshes the adapter
//! identically.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, CoefficientCombine, Collider, CollisionLayers, Forces,
    Friction, LayerMask, LinearVelocity, Mass, NoAutoAngularInertia, NoAutoCenterOfMass,
    NoAutoMass, Physics, PhysicsDebugPlugin, PhysicsGizmos, PhysicsInterpolationPlugin,
    PhysicsPlugins, PhysicsTime, ReadRigidBodyForces, RigidBody, WriteRigidBodyForces,
};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use bevy::world_serialization::WorldAssetRoot;

use crate::Layer;
use crate::bake::TankBlueprint;

// Shared course/rig/belt machinery lives here in `mod.rs`; the model's force and view systems
// live in `belt.rs` (the field-belt — the sandbox's single model, promoted into the game as
// `track::forces`).
mod harness;
// The TRACK-LINK render layer: the Tiger's own shoe mesh, instanced onto the belt stations the
// active view resamples. Everything else in this file draws the track as a line; this is the one
// module that draws it as track.
mod link_view;
// Semantic mesh layering (ported from the ballistics sandbox `crate::sandbox`): the hull's
// solid/x-ray/hidden loop and the `*_Collider` / `*_Ballistic` volume layers with their
// off/on-top/solid/x-ray states, plus the overlay camera that draws "on-top" volumes over the scene.
// Owns the [`MeshState`] / [`VolumeState`] enums the panel and [`VizLayers`] use.
mod belt;
mod mesh_layers;
// The RUNNING-GEAR render layer: the glb's own wheel/sprocket/idler nodes, bound by name and driven
// from the suspension travel + the belt phase. Without it the model's wheels sit frozen while the
// shoes scroll past them — `articulate_wheels_field` would be updating a number nothing rendered.
mod wheel_view;
// The marker-driven track model — the universal suspension laws (`derive`), the glb marker read
// (`crate::track::marker_model`'s `DerivedModel`), and the assembled geometry contract (`rig_geom`),
// which is the sandbox's only source of "where the running gear is". It lives in the shared track
// core (`crate::track`), mirroring `track::forces`; re-exported here so the sandbox's
// `super::derive` / `super::rig_geom` paths resolve.
pub(crate) use crate::track::derive;
pub(crate) use crate::track::rig_geom;
// The suspension visualisation (migrated here from the suspension editor, which this tool replaces):
// the cast routes (rest / max droop / max compression), the grip columns, the sprocket ring. Drawn
// against the LIVE hull pose, so the casts ride the driving tank. It used to own the on-screen text
// readout too; that whole surface is now the egui [`panel`].
pub(crate) mod suspension_viz;
// The clickable egui control panel — the sandbox's entire control surface (Tune / Model / Layers /
// Telemetry / Scene). Behind `dev_ui` so `bevy_egui` is compiled ONLY into the sandbox build, never
// the shipping client (`bin/overmatch`). Mounted below under the same gate.
#[cfg(feature = "dev_ui")]
mod panel;

use belt::{
    BeltPhase, PinBelt, RigTransmission, TerrainField, ViewPerf, WrapMemory,
    apply_belt_support_field, articulate_wheels_field, conform_belts_field, draw_sample_points,
};
use derive::SuspensionParams;
use mesh_layers::{MeshState, VolumeState};
use rig_geom::{Pose, RigGeom};
// The pure track core (route geometry) — moved out for game promotion (architecture §2); the
// sandbox consumes it exactly as the game's view plugin will. Re-exported so the model
// submodules' `use super::*` keeps resolving.
pub(crate) use crate::track::oracle::{BlockField, TerrainBlock};
pub(crate) use crate::track::route::{arc, external_tangent, polyline_len, resample};
// One side encoding for the whole track core; the sandbox's formerly-private `Side` migrated here.
pub(crate) use crate::track::side::{PerSide, Side};

// --- Rig geometry: NONE of it lives here any more. Every length, radius, centre, and count comes
// from [`RigGeom`], derived at build time from the Tiger's glb markers + its `.tank.ron` (see the
// substitution the module doc describes). The only rig number still authored in this file is the
// collider inset below, which is a CONTACT-MODEL policy, not a measurement. ---

/// The LIVE suspension knobs ([`SuspensionParams`] — ride frequency, damping ratio, bump-stop) as a
/// sandbox resource: seeded from the authored `track.suspension` RON block at rig build (the same
/// source the game's envelope calibration reads) and tweaked live from the [`panel`]'s Tune section;
/// [`RigGeom`] measures its droop/compression cast poses and its link-count window against them.
/// ONE resource so there is one answer wherever it is read from — a second copy is exactly how the
/// panel's live verdict and [`tune_rig_counts`]'s clamp band would end up disagreeing about the same
/// rig.
#[derive(Resource, Default)]
pub(crate) struct RigSuspension(pub(crate) SuspensionParams);

/// The Tiger's NON-geometry vehicle data, lifted out of the blueprint once at rig build so the
/// fixed-tick force systems never re-read the spec. Geometry lives in [`RigGeom`]; this is
/// everything else the force law needs.
///
/// It is all AUTHORED for the 57 t Tiger, which is exactly why it can't stay as constants: the
/// belt inertia here and the one the declared transmission is built with are the same
/// `powertrain.inertia`, so the governor and the regenerative adapters spin the same belt, and the
/// weight the envelope law calibrates against is the spec's combat mass.
#[derive(Resource)]
struct RigSpec {
    /// Combat weight (N) = `spec.mass · g` — the scale of the static-grip bristle stiffness, the
    /// envelope-law calibration, and the support-fraction readout.
    weight_n: f32,
    /// Soft-engagement depth (m): a station ramps its contact force in over the first this-many
    /// metres of penetration instead of switching full force on at the belt surface — a real track
    /// is compliant, not a hard edge. Well below the static sink, so it sets the behaviour at the
    /// contact boundary, not the ride height.
    engage_depth: f32,
    /// Governor envelope (`track.powertrain`): governed belt speed (m/s), constant-power engine
    /// (W/track), low-speed force cap (N), governor gain (N per m/s of speed error).
    max_speed: f32,
    engine_power: f32,
    engine_force: f32,
    governor_gain: f32,
    /// Reflected belt + drivetrain inertia (kg).
    belt_inertia: f32,
}

// --- Test course (module-level so the reset + trench floors can reference the trenches) ---
/// Trenches down the −Z lane, each `(centre z, width)`, nearest→farthest. Narrow: some road wheels
/// still catch the lips. Wide (> the road-wheel span): all road wheels float, only the sprocket/idler
/// diagonals catch — the pure bridging case. Pit (> the whole track footprint): nothing can catch —
/// the rig drops in; the drop-in / grind-out case.
const TRENCHES: [(f32, f32); 3] = [(30.0, 2.2), (42.0, 5.0), (58.0, 10.0)];
/// Washboard sets `(start z, period, bumps, height)` of increasing coarseness, all before the first
/// trench. Bump thickness is `period / 3`, so the gaps grow with the period: the fine set's gaps are
/// narrower than a road wheel (the belt/wheels *bridge* them), the coarse sets' gaps are wider (the
/// wheels drop in and ride over each bump) — the resolve-vs-bridge spectrum in one drive.
const WASHBOARDS: [(f32, f32, usize, f32); 3] = [
    (3.0, 0.8, 6, 0.12),
    (10.0, 1.5, 5, 0.18),
    (19.0, 2.5, 4, 0.22),
];
/// Lane extent (Z) of the ground: from `LANE_NEAR` in front of spawn out to `LANE_FAR`.
const LANE_NEAR: f32 = 20.0;
const LANE_FAR: f32 = -110.0;
/// Lane width (X) of the ground slabs — wide enough to manoeuvre, turn, and drive around obstacles.
const LANE_W: f32 = 40.0;
/// Width (X) of the raised obstacles (washboards, step, ramp): a sub-lane, so there is open flat
/// ground on both sides to steer around them and to compare against.
const OBSTACLE_W: f32 = 16.0;
/// Top of the trench floors: a hard bottom below belt reach, so a *failed* bridge rests the rig in
/// the ditch instead of dropping into a bottomless gap.
const TRENCH_FLOOR_Y: f32 = -1.2;

/// Slope test pad (harness `pose=slope_*`): a 20° incline parked OFF-lane at +X, large enough
/// to hold any hull orientation through a 30 s parked-hold capture. Authored unconditionally —
/// the lab course costs nothing and interactive drives can visit it.
pub(super) const SLOPE_PAD_DEG: f32 = 20.0;
const SLOPE_PAD_CENTER: Vec3 = Vec3::new(34.0, 0.0, -20.0);
const SLOPE_PAD_SIZE: f32 = 24.0;
const SLOPE_PAD_THICK: f32 = 2.0;

/// Flat runway/turn pad (harness `pose=runway`), far off-lane at +X: 400 m of straight run for
/// gearing top-speed measurements plus room for full turning circles — the lane proper is too
/// obstacle-dense for either. Its z-extent stays INSIDE the lane's z-range on purpose: the
/// terrain broadphase buckets by z, so the grid shape (and with it every existing capture's
/// candidate iteration) is unchanged — the slope-pad captures see zero new candidates
/// (x-AABB rejection is pure comparison). Verified byte-identical against the pre-runway build.
const RUNWAY_CENTER: Vec3 = Vec3::new(260.0, 0.0, -45.0);
const RUNWAY_SIZE: (f32, f32) = (400.0, 120.0);
/// The `pose=runway` spawn: near the pad's −X end, facing +X down the long axis.
pub(super) const RUNWAY_SPAWN: Vec3 = Vec3::new(70.0, 0.0, -45.0);

/// The pad's top-face centre + its tilt rotation — the harness spawns slope poses from this.
pub(super) fn slope_pad_pose() -> (Vec3, Quat) {
    let rot = Quat::from_rotation_x(SLOPE_PAD_DEG.to_radians());
    (
        SLOPE_PAD_CENTER + rot * Vec3::Y * (SLOPE_PAD_THICK / 2.0),
        rot,
    )
}

// --- Belt contact model. The station SPACING is the track's own link pitch (`geom.pitch`) and the
// belt LENGTH is the material loop (`geom.belt_len()` = pitch × link_count) — both derived. Because
// every coefficient below is **per metre of belt**, resolution and total force are independent: a
// different pitch changes only how finely the loop is sampled. Slack is not a budget added to a taut
// wrap either — it is what the authored link count leaves over (`belt_len − taut_perimeter`,
// reported by `RigGeom::window`). ---
/// Downward ray length used to find ground just beneath each station (m); also the sink at which
/// support saturates.
const CONTACT_PROBE: f32 = 0.5;

// --- Drive: belt-speed / slip model. Each track has a belt *speed*; friction comes from the slip
// between belt and ground, so wheelspin, skid, engine-braking, hill-hold, and top speed all emerge.
// The drivetrain envelope is *vehicle* spec, not track-model spec — so it comes off the blueprint
// ([`RigSpec`]), not from constants here. What stays below is the FRICTION law, which is a property
// of steel-on-ground, not of the vehicle. ---
/// Slip speed (m/s) at which ground friction saturates to μ·load. Below it grip is ~proportional to
/// slip (rolling); above it the track is sliding (the wheelspin/skid regime).
const SLIP_SATURATION: f32 = 0.4;
/// Coulomb coefficient: an element's grip force is capped at μ × its elastic load (the
/// element law is isotropic — turning resistance emerges from footprint geometry).
const MU: f32 = 0.9;

// --- Wheels carry NO force: the belt is the *sole* ground-contact system (carries the tank,
// tractions, does walls/gaps). The VISUAL data direction is wheels-first
// (`articulate_wheels_field` reads the terrain field directly, then the view fits the belt
// around the wheels — ground → wheels → belt, acyclic; a belt-first order is circular).
//
// Wheel smoothing is asymmetric and physical: a RISE is instant, a FALL is ballistic
// (gravity-limited). Zero tuning constants — smoothing the rise would be wrong anyway, because
// terrain forcing a wheel up is kinematic and lag there reads as the board entering the wheel. The cosmetic
// travel spans the physical CONTACT ENVELOPE — up to the bump stop, down to the chain-clamped
// droop ([`SuspensionParams::bump_stop`] and `RigGeom::droop_travel(..).effective`, wired in
// `belt::articulate_wheels_field`) — not an arbitrary clamp. ---

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

/// A road wheel's cosmetic placement state: the rest pivot in hull-local space and the current
/// vertical lift. Rise is instant; `dvel` is the ballistic fall speed while the wheel drops
/// toward a lower target (see the wheel-doctrine comment above [`RigWheel`], and the
/// contact-envelope band `articulate_wheels_field` clamps to). Visual only — no force.
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

/// Each side's conformed belt this frame — the belt path fitted around the articulated wheels and
/// conformed to terrain, in loop order. Built once per frame by the view system
/// (`conform_belts_field`); the drawn spline is exactly this.
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

/// Marker for the hull body — the sandbox's single dynamic rigid body, and the frame every piece of
/// hull-local geometry ([`RigGeom`]) is expressed in. `pub(crate)` because the viz layers draw on
/// the moving tank and need `With<Hull>` to find it.
#[derive(Component)]
pub(crate) struct Hull;

/// The RED hard-stop collider for one side — the suspension's mechanical backstop. One convex-hull
/// prism per side, extruded from [`RigGeom::hard_stop_polyline`] across the true shoe faces, hung on
/// the hull. It is a PURE penetration stop (frictionless; the belt owns every tangential force) that
/// nothing may cross once the springs bottom on the bump-stop; at rest it clears flat ground by the
/// bump-stop reserve and touches nothing. Marked so [`refresh_hard_stops`] can despawn and rebuild it
/// whenever a retune moves the geometry it was cut from.
#[derive(Component)]
struct HardStop;

/// The free-fly inspection camera (own copy, like `armor_sandbox`'s).
#[derive(Component)]
struct FreeFlyCam;

pub fn plugin(app: &mut App) {
    // `spec` registers the `.tank.ron` loader; `bake` parses the Tiger glb into `TankBlueprint` at
    // Startup — the vehicle's single source of truth, shared verbatim with the game (which mounts
    // the same pair). Its resource only becomes VISIBLE in `Update`, which is what
    // forces the deferred [`build_rig`] below.
    app.add_plugins((crate::spec::plugin, crate::bake::plugin))
        // This is a WINDOWED root on `DefaultPlugins`, so it is on bevy's deadlocking exit path
        // exactly like the game is: without this, quitting the sandbox wedges the process instead
        // of ending it (see `crate::quit`).
        .add_plugins(crate::quit::plugin)
        // The suspension cast/route/grip-column overlay + its readout panel. Self-gated on
        // `RigGeom` and on a `Hull` existing, so it simply no-ops on the pre-build frames.
        .add_plugins(suspension_viz::plugin)
        // The instanced shoes. Self-gated on `RigGeom` + the glb template, so it no-ops on
        // the pre-build frames exactly like the overlay does.
        .add_plugins(link_view::plugin)
        // The driven running gear. Self-gated on `RigGeom` + a bound glb node, so it no-ops
        // on the pre-build frames like the other two view layers.
        .add_plugins(wheel_view::plugin)
        // Semantic mesh layering: the hull's solid/x-ray/hidden loop and the collider/ballistic
        // volume layers (rendered from the glb's own `*_Collider`/`*_Ballistic` node meshes) with an
        // overlay camera for the on-top state. Independent of `RigGeom` — it tags scene meshes as
        // they land, so it self-noops until the glb instantiates.
        .add_plugins(mesh_layers::plugin)
        .add_plugins(PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()))
        // Registers the `PhysicsGizmos` group for the collider-wireframe layer; starts disabled in
        // `configure_collider_gizmos`.
        .add_plugins(PhysicsDebugPlugin)
        .init_resource::<BeltContacts>()
        .init_resource::<SideDynamics>()
        .init_resource::<BeltGrip>()
        .init_resource::<BeltGripElements>()
        .init_resource::<TransSwitch>()
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
        // The kinematic wrap's per-side filter memory — `conform_belts_field` owns the mutation,
        // the reseat paths drop it. There is no config beside it: the feel tiers are unconditional
        // parameter-free laws.
        .init_resource::<WrapMemory>()
        .init_resource::<ViewPerf>()
        .init_resource::<RigSuspension>()
        // Panel↔keyboard shared seams: the reset trigger and the egui input-capture flags. Always
        // present (default false) so the driving/camera run-conditions compile and behave without
        // `dev_ui`; the panel writes the capture flags only when it is compiled in.
        .init_resource::<ResetRequested>()
        .init_resource::<PanelWantsInput>()
        // Exists from frame 0 (0/0 sentinel) so the panel's `ResMut<RigCounts>` never faults on a
        // pre-rig frame; `build_rig` overwrites it with the authored counts.
        .init_resource::<RigCounts>()
        .add_systems(
            Startup,
            (
                spawn_camera,
                // A harness run must not steal the user's cursor while it captures.
                grab_cursor.run_if(not(resource_exists::<harness::Harness>)),
                spawn_environment,
                configure_collider_gizmos,
            ),
        )
        // The rig is built ONCE, in `Update`, as soon as the blueprint lands — it cannot be a
        // Startup system, because `bake` inserts `TankBlueprint` with `Commands` at Startup and
        // deferred inserts are only visible from the next schedule on.
        .add_systems(Update, build_rig.run_if(rig_unbuilt))
        // Physics runs in the fixed step (before Avian integrates in FixedPostUpdate), NOT while
        // paused (else penalty force accumulates against a frozen sim and flings the rig on resume),
        // and not before the rig exists (`RigGeom` is the single "the rig is up" gate — every
        // rig resource lands in the same command flush, so one check covers them all).
        //
        // `apply_belt_support_field`: the advected pin-line ring, penetration from the analytic
        // terrain field at fixed collocation stations (no narrow-phase queries).
        .add_systems(
            FixedUpdate,
            apply_belt_support_field
                .run_if(sim_running)
                .run_if(resource_exists::<RigGeom>),
        )
        .add_systems(
            Update,
            (
                // The visual pipeline, in data order — wheels FIRST (ground → wheels → belt,
                // acyclic): the wheels read the field, then the wrap fits around them. A
                // belt-first order is circular.
                // The stateful pieces gate on `sim_running` like the physics — Esc pauses Avian's
                // clock but NOT the Update schedule, so ungated they kept easing wheels / advancing
                // the wrap's filter memory against a frozen sim ("deforms while paused" — the
                // second clock). The draw systems stay ungated: gizmos are immediate-mode and must
                // redraw the frozen state.
                (
                    articulate_wheels_field.run_if(sim_running),
                    conform_belts_field.run_if(sim_running),
                    draw_rig_gizmos,
                )
                    .chain(),
                toggle_trans_mode,
                // The transmission state reset (a fresh adapter never inherits another's gear),
                // centralised as a change-reaction so a flip from EITHER the `T` key or the panel
                // refreshes identically — neither surface has to own the reset.
                reset_trans_on_change,
                refresh_envelope,
                refresh_hard_stops,
                reset_rig,
                // `;`/`'` `n`/`m` are now INTENT writers onto `RigCounts`; `apply_rig_counts` is the
                // single commit path (shared with the panel's stepper) that rebuilds the rig.
                tune_rig_counts,
                apply_rig_counts,
                log_state,
                draw_contacts,
                draw_sample_points,
            )
                // Everything above reads the rig (its resources or its `Hull`/`RigWheel`
                // entities), so it all waits for the deferred build.
                .run_if(resource_exists::<RigGeom>),
        )
        .add_systems(
            Update,
            (
                // Both gate off while an egui widget has focus, so a slider drag or a click on the
                // panel never leaks into the camera / drive command (`panel_capturing`). `fly_camera`
                // is additionally cursor-locked as before — belt-and-suspenders, since freeing the
                // cursor to reach the panel already stops the fly.
                fly_camera
                    .run_if(cursor_locked)
                    .run_if(not(panel_capturing)),
                read_drive_input.run_if(not(panel_capturing)),
                toggle_pause,
                // The mesh/collider visibility mirrors + the reference-ring draw. The layer TOGGLES
                // moved to the egui panel (Layers section); these systems still read the same
                // `VizLayers` the panel now writes, unchanged.
                // Runs EVERY frame (write-on-change), not just when the layer resource flips: this
                // is the same continuously-asserting pattern the ballistics sandbox uses
                // (`crate::sandbox::apply_layer_visibility`). An edge-triggered mirror loses every
                // race against LATE writers of `Visibility` on the model tree — the async glb scene
                // instantiation / hot-reload path, and a rig whose `Hull` spawns a frame AFTER the
                // toggle was pressed — leaving a hidden layer resurrected until the next keypress.
                // Asserting the state every frame is what makes the toggle authoritative.
                apply_mesh_visibility,
                sync_collider_gizmos.run_if(resource_changed::<VizLayers>),
                draw_taut_reference,
            ),
        );

    // The egui control panel — the sandbox's whole control surface. Behind `dev_ui` so `bevy_egui`
    // is compiled ONLY here (the `track_sandbox` bin declares `required-features = ["dev_ui"]`), and
    // never into `bin/overmatch`. It brings its own `EguiPlugin` and runs in `EguiPrimaryContextPass`.
    #[cfg(feature = "dev_ui")]
    app.add_plugins(panel::plugin);

    // The scripted capture harness (`SANDBOX_HARNESS` env var): scenario in, JSONL out, exit.
    // Bit-REPEATABLE (step 25b): virtual time advances exactly one fixed tick per rendered frame
    // (wall clock never enters the sim), and the scripted throttle is written INSIDE FixedUpdate
    // before the force systems — its phase boundaries land on exact ticks. Without both, frame
    // pacing leaked into recorded trajectories (~mm-level hull drift between identical runs) and
    // A/B gates could only ever be statistical.
    let harness_scenario =
        harness::parse_env().unwrap_or_else(|err| panic!("invalid SANDBOX_HARNESS: {err}"));
    if let Some(scenario) = harness_scenario {
        app.insert_resource(scenario)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_micros(15_625), // exactly 1/64 s
            ))
            // Setup follows the rig into `Update`: it poses the hull the scenario asks for, so it
            // must run AFTER the deferred build, and only once (its own `HarnessLog` is the
            // latch). The explicit `after` also forces the sync point that makes `build_rig`'s
            // spawned hull visible to it in the SAME frame — so the first fixed tick the rig
            // exists for is already the scenario's tick 0, exactly as the Startup version was.
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
/// and the friction force it applied (world space — the force-vector layer).
struct Contact {
    local: Vec3,
    /// Actual damped load (what scaled the ellipse) — `load_elastic` is the spring-only part.
    load: f32,
    load_elastic: f32,
    normal: Vec3,
    slip: f32,
    slip_lat: f32,
    f_long: f32,
    f_lat: f32,
    traction: Vec3,
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
/// vector per material link × column. Always PRE-SIZED from the pin belt ([`Self::sized`]; startup,
/// `R` reset, count retune): `contact_side` never resizes at runtime, and empty slabs would skip
/// traction for the tick.
#[derive(Resource, Default)]
struct BeltGripElements(PerSide<crate::track::forces::GripElements>);

impl BeltGripElements {
    /// Both sides at rest, slabs pre-sized for `link_count` material links (the fixed-size
    /// invariant — see `track::forces::GripElements::for_links`).
    fn sized(link_count: usize) -> Self {
        use crate::track::forces::GripElements;
        Self(PerSide::new(
            GripElements::for_links(link_count),
            GripElements::for_links(link_count),
        ))
    }
}

/// The active transmission adapter (harness `trans=` key; `T` or the panel's Model selector cycle
/// live). `None` ONLY on the pre-rig frames: [`build_rig`] seeds it from the SPEC's declared
/// `transmission.architecture`, so a bare sandbox/harness run sims the same drivetrain the
/// shipped tank does — the old implicit Governor default silently A/B'd against the wrong
/// transmission. Every sim consumer runs behind the `RigGeom` gate, which lands in the same
/// command flush as the seed, so no tick ever executes on `None`.
#[derive(Resource, Default)]
struct TransSwitch(Option<crate::track::transmission::TransmissionMode>);

/// The joint transmission's state (gear, shift countdown, steering detent, direction) — the
/// sandbox analogue of the game's `TankTransmission` component. Reset with the rig and on
/// every mode flip (a fresh adapter never inherits another's gear).
#[derive(Resource)]
struct TransState(crate::track::transmission::TransmissionState);

/// Last tick's transmission report (gear/rpm/detent/power scale) — harness `tr` rows + the
/// legend. `None` while the governor runs (it has no operating point to report).
#[derive(Resource, Default)]
struct TransTelemetry(Option<crate::track::transmission::TransmissionReport>);

/// `T` (or the panel's Model selector) cycles the transmission adapter live
/// (governor → hybrid → L600). A pure INTENT write: the state reset that gives the incoming
/// adapter a fresh gear lives in [`reset_trans_on_change`], so a flip from either surface refreshes
/// identically. Write-on-change so an idle frame never marks [`TransSwitch`] changed.
fn toggle_trans_mode(keys: Res<ButtonInput<KeyCode>>, mut switch: ResMut<TransSwitch>) {
    use crate::track::transmission::TransmissionMode;
    if !keys.just_pressed(KeyCode::KeyT) {
        return;
    }
    // Behind the `RigGeom` gate the switch is always seeded; the `else` is unreachable belt
    // and suspenders, not a hidden default.
    let Some(current) = switch.0 else {
        return;
    };
    switch.0 = Some(match current {
        TransmissionMode::Governor => TransmissionMode::Hybrid,
        TransmissionMode::Hybrid => TransmissionMode::FixedRadii,
        TransmissionMode::FixedRadii => TransmissionMode::Governor,
    });
}

/// Reset the transmission state whenever the adapter changes (from the `T` key or the panel): the
/// incoming adapter starts constructed (gear 1, no shift in flight) instead of inheriting the
/// previous one's gear. Change-driven so it is the ONE owner of the reset regardless of who moved
/// the switch. Skips the initial resource insert — [`TransState`] is not up yet on the frame
/// [`TransSwitch`] is first added (`build_rig` builds a fresh state a flush later).
fn reset_trans_on_change(
    switch: Res<TransSwitch>,
    transmission: Res<RigTransmission>,
    mut state: ResMut<TransState>,
) {
    if switch.is_added() || !switch.is_changed() {
        return;
    }
    let Some(mode) = switch.0 else {
        return;
    };
    *state = TransState(crate::track::transmission::TransmissionState::from_spec(
        &transmission.0,
    ));
    info!("transmission → {}", mode.label());
}

/// Recalibrate the envelope law whenever its inputs move: a rig rebuild (`;`/`'`, `n`/`m`,
/// `R`) or a suspension knob (the panel's Tune section). Change-detection driven — the calibration
/// runs one contact pass per side, far too heavy for every tick and trivial on a knob turn.
fn refresh_envelope(
    geom: Res<RigGeom>,
    suspension: Res<RigSuspension>,
    rig: Res<RigSpec>,
    mut law: ResMut<belt::EnvelopeLaw>,
) {
    if !(geom.is_changed() || suspension.is_changed() || rig.is_changed()) {
        return;
    }
    *law = belt::calibrate_envelope(&geom, &suspension.0, rig.weight_n, rig.engage_depth);
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

/// Per-layer visibility switches for every visual element in the sandbox, each independently
/// toggleable from the [`panel`]'s Layers section.
///
/// The boot defaults are QUIET — the tank and its shoes, and nothing else. Everything below that is
/// a diagnostic you ask for; drawing them all at once is unreadable.
///
/// `Copy + PartialEq` so the [`panel`] can edit a LOCAL copy behind its checkboxes and write the
/// resource back only on a real change — the write-on-change discipline that keeps
/// `sync_collider_gizmos` (gated on `resource_changed::<VizLayers>`) from firing every frame.
#[derive(Resource, Clone, Copy, PartialEq)]
struct VizLayers {
    /// The hull's render meshes (the Tiger glb's `*_Visual` nodes) as a solid → x-ray → hidden loop
    /// ([`MeshState`]). Driven per-mesh by [`mesh_layers`], which tags each hull visual mesh and
    /// re-asserts its `Visibility` + material EVERY frame (write-on-change) — so a late writer on the
    /// model tree (the async glb scene finishing instantiation, a hot-reload re-spawning it, or the
    /// deferred [`build_rig`] bringing the model up a frame after a panel edit) cannot resurrect a
    /// hidden model: the next frame puts it back. THE INVARIANT: re-assert visibility every frame —
    /// an edge-triggered mirror loses races to late `Visibility` writers. X-ray swaps a translucent
    /// material in, so the suspension story reads through the shell.
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
    /// The sandbox COURSE — the terrain slabs, obstacles, ramps and pads [`spawn_environment`] spawns
    /// at the scene root (everything NOT under the tank) — as a solid → x-ray → hidden loop
    /// ([`MeshState`]), driven per-mesh by [`mesh_layers`] like the hull. X-ray drops it to a dim
    /// neutral ghost so the tank stays the subject; hidden clears the visual clutter for a
    /// running-gear/belt close-up. Visibility/material ONLY — the static terrain colliders (and the
    /// belt's analytic field) are untouched, so the physics is identical whatever this shows.
    /// Default SOLID: the course is the ground the tank drives on, not a diagnostic. It is a layer of
    /// its own because the course meshes are parent-less scene-root meshes — classified by
    /// [`mesh_layers`]' ancestry walk, not by name, so x-raying the hull cannot ghost the world.
    world: MeshState,
    /// The wheel render meshes: the glb's wheel/sprocket/idler nodes live at the scene root, so
    /// [`wheel_view`] binds them and this switch hides exactly the running gear. It writes
    /// `Visible`/`Hidden` rather than `Inherited`, the same override the shoes take, so "model off,
    /// running gear on" — the view for a tooth-mesh check — is a state the layer can express.
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
    /// Avian collider wireframes (hull box, hard-stop prisms, terrain).
    colliders: bool,
    /// The taut reference loop (the belt's rest path, vs the conformed/solved view).
    reference: bool,
    /// The INSTANCED TRACK LINKS ([`link_view`]): the model's own shoe mesh laid on the belt
    /// stations, one entity per material link. ON at boot: it is the track, not a diagnostic.
    links: bool,
    /// DRIVE the running gear ([`wheel_view`]): suspension travel on the road-wheel nodes,
    /// belt-derived spin on all of them, the sprocket tooth-locked to the belt phase. Off parks
    /// every node at its authored pose — the A/B against the derived rest gear the suspension
    /// overlay draws. The one switch here that changes MOTION rather than visibility. ON at boot: a
    /// tank whose wheels don't turn is the bug, not the baseline.
    running_gear: bool,
}

impl Default for VizLayers {
    fn default() -> Self {
        Self {
            // The curated "suspension story through a translucent tank" boot view: an x-ray hull with
            // the running gear and shoes on and the belt line off, so the rest/droop routes read
            // through the shell rather than being buried under an opaque model.
            hull: MeshState::Xray,
            collider_volumes: VolumeState::Hidden,
            ballistic_volumes: VolumeState::Hidden,
            // The course is the ground, not a diagnostic — solid at boot.
            world: MeshState::Solid,
            wheels: true,
            // Off at boot — the belt LINE is a diagnostic; the shoes (below) are the track.
            belt_line: false,
            outer: false,
            hubs: false,
            dots: false,
            normals: false,
            forces: false,
            casts: false,
            colliders: false,
            reference: false,
            // The shoes are the feature, not a diagnostic — the one layer whose default is loud.
            links: true,
            // Same argument: driven wheels are the vehicle, not an overlay.
            running_gear: true,
        }
    }
}

/// Mirror the `wheels` switch onto the running-gear render entities. The running gear hangs under the
/// hull, so a hidden hull would inherit-hide it; `Visibility::Visible` is the unconditional override
/// that keeps the wheels drawable with the hull model off — the tooth-mesh view (see
/// [`VizLayers::wheels`]). (The HULL visual meshes are no longer toggled here: [`mesh_layers`] tags
/// and drives them per-mesh for the solid/x-ray/hidden loop.)
///
/// Both wheel sets are covered: the sandbox's own data-carrier [`RigWheel`] entities (mesh-less
/// today, so the write is inert but free) and the glb nodes [`wheel_view`] binds, which are the ones
/// that actually draw. `wheel_view` still seeds the same override at bind time so a node is correct
/// on the very frame it appears (before this system next runs); from then on this system re-asserts
/// it every frame.
///
/// Runs UNCONDITIONALLY, every frame — NOT gated on `resource_changed::<VizLayers>`. Re-assert
/// visibility every frame; an edge-triggered mirror loses races to late `Visibility` writers (the
/// async glb scene instantiation completing, a hot-reload re-instantiating the scene, or the
/// deferred [`build_rig`] spawning a node a frame after an early toggle). The `set_if_neq` keeps the
/// every-frame run cheap: it only actually writes (and so only trips visibility propagation) when
/// the value truly changes, exactly like the ballistics sandbox's
/// [`crate::sandbox::apply_layer_visibility`].
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

/// Avian's `PhysicsGizmos` group (collider wireframes) starts silent; the `0` layer enables it.
fn configure_collider_gizmos(mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<PhysicsGizmos>().0.enabled = false;
}

fn sync_collider_gizmos(viz: Res<VizLayers>, mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<PhysicsGizmos>().0.enabled = viz.colliders;
}

/// The taut reference loop in world space — the belt's rest path around the articulated wheels,
/// built by the wrap on request ([`wrap::WrapInput::reference`](crate::track::wrap::WrapInput)).
/// Written by [`conform_belts_field`], drawn by the `-` layer: belt-vs-reference deviation shows
/// where terrain and slack hold the belt off its rest path. A DIAGNOSTIC overlay, not a view option
/// — the game never asks the wrap to build it.
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
        gizmos.linestrip(pts.iter().copied().chain(pts.first().copied()), REF_COLOR);
    }
}

/// Which reset spot `R` will drop the rig at next (index into [`RESET_SPOTS`]).
#[derive(Resource, Default)]
struct ResetSpot(usize);

/// The `R` drop spots: a quick tour of the test cases. `z` is the lane position; all drop at the
/// resting ride height.
const RESET_SPOTS: [(f32, &str); 4] = [
    (0.0, "flat ground"),
    (-TRENCHES[0].0, "narrow trench"),
    (-TRENCHES[1].0, "wide trench (pure diagonal bridge)"),
    (
        -TRENCHES[2].0,
        "pit (swallows the whole rig - drop in, grind out)",
    ),
];

/// RAW driver intent in [-1, 1]: throttle (↑/↓) and steer (→/←), unshaped — arrow keys (WASD
/// stays the free-fly camera), or the harness script. The FIXED-tick force adapter slews it
/// through the shared [`crate::track::drive::shape_drive`] (same seam as the game), so the
/// harness tests the slew as part of the path.
#[derive(Resource, Default)]
struct RawDriveInput(crate::track::drive::DriveAxes);

/// The slewed drive state — the sandbox's analogue of the game's `TrackDrive.throttle/steer`,
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

/// `Esc` releases the cursor and freezes the sim so you can take a screenshot; press again to
/// re-capture and resume. Fly + (future) drive gate on `cursor_locked`, so releasing the cursor is
/// what pauses the interaction; pausing Avian time freezes the dynamics too.
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
            // The force adapter (the only system that slews ShapedDrive) is gated off while
            // paused — without this clear, resume would re-apply the pre-pause command and
            // slew it down: stale thrust (codex parts-1/2 review #1).
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
            Transform::from_xyz(11.0, 3.5, 3.0).looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y),
            FreeFlyCam,
        ))
        .with_children(|parent| {
            // The overlay camera for "on-top" volumes: a child so it shares the fly pose, drawn after
            // the main pass (order 1) with no clear, rendering only `OVERLAY_LAYER` — its own depth
            // buffer makes those volumes composite over the scene even when geometrically inside the
            // hull. Mirrors `crate::sandbox`'s overlay camera; coupled to the fly cam so on-top
            // rendering tracks the view.
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

/// Lighting + the deterministic test course: a flat lane down −Z with a **trench** (a gap in the
/// ground the rig must bridge), a **step**, and a **ramp**. All on the `Terrain` layer so the belt
/// contact (once it exists) reads it uniformly. Isolated, known geometry — you can tell the sim from
/// the terrain.
fn spawn_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 9.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let ground_mat = materials.add(Color::srgb(0.32, 0.42, 0.28));
    let obstacle_mat = materials.add(Color::srgb(0.44, 0.37, 0.27));

    // Every block also lands in the analytic terrain field (the belt's oracle) — colliders and
    // field are built from the same transforms, so the two representations cannot drift.
    let mut field: Vec<TerrainBlock> = Vec::new();

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
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        ));
    };
    // A ground slab spanning z_hi..z_lo (z_hi > z_lo), top face at y=0.
    let ground = |commands: &mut Commands, field: &mut Vec<TerrainBlock>, z_hi: f32, z_lo: f32| {
        block(
            commands,
            field,
            Transform::from_xyz(0.0, -0.5, (z_hi + z_lo) / 2.0).with_scale(Vec3::new(
                LANE_W,
                1.0,
                z_hi - z_lo,
            )),
            &ground_mat,
        );
    };

    // Lay the ground as slabs between the trench gaps, walking nearest→farthest. Each trench also
    // gets a hard floor below belt reach so a failed bridge rests in the ditch, not the void.
    let mut cursor = LANE_NEAR;
    for (tz, tw) in TRENCHES {
        let near_lip = -(tz - tw / 2.0);
        let far_lip = -(tz + tw / 2.0);
        ground(&mut commands, &mut field, cursor, near_lip);
        block(
            &mut commands,
            &mut field,
            Transform::from_xyz(0.0, TRENCH_FLOOR_Y - 0.5, -tz)
                .with_scale(Vec3::new(LANE_W, 1.0, tw)),
            &ground_mat,
        );
        cursor = far_lip;
    }
    ground(&mut commands, &mut field, cursor, LANE_FAR);

    // A step / curb (top at y=0.45), past the trenches: a hard vertical edge to climb.
    block(
        &mut commands,
        &mut field,
        Transform::from_xyz(0.0, 0.225, -72.0).with_scale(Vec3::new(OBSTACLE_W, 0.45, 4.0)),
        &obstacle_mat,
    );

    // A 20° ramp beyond the step (flush entry, crest with a drop) to check climb + envelope over a
    // slope. Low-edge top sunk ~1 m under the ground plane so the approach is step-free.
    let (run, thick, deg) = (10.0_f32, 2.0_f32, 20.0_f32);
    let (sin, cos) = deg.to_radians().sin_cos();
    let center_y = -1.0 - (thick / 2.0) * cos + (run / 2.0) * sin;
    block(
        &mut commands,
        &mut field,
        Transform::from_xyz(0.0, center_y, -88.0)
            .with_rotation(Quat::from_rotation_x(deg.to_radians()))
            .with_scale(Vec3::new(OBSTACLE_W, thick, run)),
        &obstacle_mat,
    );

    // The slope pad (see `slope_pad_pose`).
    block(
        &mut commands,
        &mut field,
        Transform::from_translation(SLOPE_PAD_CENTER)
            .with_rotation(Quat::from_rotation_x(SLOPE_PAD_DEG.to_radians()))
            .with_scale(Vec3::new(SLOPE_PAD_SIZE, SLOPE_PAD_THICK, SLOPE_PAD_SIZE)),
        &obstacle_mat,
    );

    // The runway/turn pad (see `RUNWAY_CENTER`), top face at y = 0 like the lane slabs.
    block(
        &mut commands,
        &mut field,
        Transform::from_xyz(RUNWAY_CENTER.x, -0.5, RUNWAY_CENTER.z).with_scale(Vec3::new(
            RUNWAY_SIZE.0,
            1.0,
            RUNWAY_SIZE.1,
        )),
        &ground_mat,
    );

    // The washboards, in front of spawn and before the first trench: one set per density (see
    // `WASHBOARDS`) — fine gaps the wheels bridge, coarse gaps they drop into and ride over. The
    // clearest "the model resolves what it should and bridges what it should" demo.
    for (start, period, bumps, height) in WASHBOARDS {
        let thickness = period / 3.0;
        for i in 0..bumps {
            let z = -(start + i as f32 * period);
            block(
                &mut commands,
                &mut field,
                Transform::from_xyz(0.0, height / 2.0, z)
                    .with_scale(Vec3::new(OBSTACLE_W, height, thickness)),
                &obstacle_mat,
            );
        }
    }

    commands.insert_resource(TerrainField(BlockField::new(field)));
}

/// Run condition for the one-shot [`build_rig`]: the blueprint has landed and the rig is not up yet.
/// `RigGeom` is the latch — `build_rig` inserts it, so this stops matching from the next frame on.
fn rig_unbuilt(geom: Option<Res<RigGeom>>, blueprint: Option<Res<TankBlueprint>>) -> bool {
    geom.is_none() && blueprint.is_some()
}

/// Path to the Tiger glb, resolved through the shared asset root — the same file the bake opens.
/// `RigGeom::build` re-reads it directly for the MARKER measurements (pin pitch, surface offsets,
/// sprocket/idler mesh centroids), which the baked node list cannot express: the markers hang under
/// a scaled ancestor and the drive wheels carry identity transforms with their geometry baked into
/// the vertices.
fn tiger_glb() -> std::path::PathBuf {
    crate::assets::asset_root().join(crate::tank::TIGER_GLB_PATH)
}

/// Build the whole rig, ONCE, the frame the blueprint becomes visible: derive [`RigGeom`], spawn the
/// Tiger (hull body + glb visual + wheel data carriers), and size everything that hangs off the belt
/// (the pin belt, the grip-element slabs, the drivetrain state).
///
/// It is one system rather than the old chain of Startup systems on purpose: `RigGeom` is the input
/// to all of it, and doing it in one body means there is no window in which half the rig exists.
/// Everything it inserts lands in the same command flush, which is what makes
/// `resource_exists::<RigGeom>` a sound gate for the entire downstream sandbox.
fn build_rig(
    mut commands: Commands,
    blueprint: Res<TankBlueprint>,
    asset_server: Res<AssetServer>,
    mut suspension: ResMut<RigSuspension>,
    mut grip_elements: ResMut<BeltGripElements>,
    mut trans_switch: ResMut<TransSwitch>,
) {
    let spec = &blueprint.spec;
    let track = &spec.track;
    // Seed the live knobs from the AUTHORED ride model — the same `track.suspension` block
    // the game's envelope calibration reads, so the sandbox boots as the shipped tank and
    // the panel's Tune knobs move it from there.
    suspension.0 = track.suspension.params();
    // The two AUTHORED counts the rig owns; every other number below is measured or derived from
    // them. `teeth` sets the chord-exact sprocket pitch circle, `link_count` IS the material loop.
    let geom = RigGeom::build(
        &blueprint,
        &tiger_glb(),
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
    // An authoring alarm, not a crash: the belt model happily resamples a too-short loop onto the
    // taut wrap, so a bad link count shows up as slightly-wrong link density and a track the
    // suspension could never actually reach — silent, and exactly the class of error the derived
    // contract exists to make loud.
    //
    // ONLY the too-short case warns, and that asymmetry is deliberate: `Impossible` means the loop
    // cannot wrap even the fully COMPRESSED hull, so no pose is reachable and the rig is a fiction.
    // A count ABOVE `n_droop` is not a fault at all — the springs limit droop first and the belt
    // carries slack, which is how a real tank runs (the tensioner takes it up). Warning on that
    // would be crying wolf at the desirable regime; the readout reports it as a regime instead.
    if window.limiter == rig_geom::DroopLimiter::Impossible {
        warn!(
            "rig: the authored link_count ({}) cannot wrap the derived running gear — the loop is \
             {:.3} m short of the fully-compressed hull. `track.link_count` in tiger_1.tank.ron was \
             sized against the RON's own (pre-marker) circles; the marker-derived geometry wants at \
             least {} links.",
            window.n, -window.slack_rest, window.n_min,
        );
    }

    // The non-geometry vehicle data the force law reads every tick, pulled out of the spec once.
    commands.insert_resource(RigSpec {
        weight_n: spec.mass * derive::G,
        engage_depth: track.suspension.engage,
        max_speed: track.powertrain.max_speed,
        engine_power: track.powertrain.power,
        engine_force: track.powertrain.force,
        governor_gain: track.powertrain.governor_gain,
        belt_inertia: track.powertrain.inertia,
    });

    // The calibrated contact-envelope law, lands in the same command flush as `RigGeom` (the
    // force step reads both behind the one `resource_exists::<RigGeom>` gate).
    commands.insert_resource(belt::calibrate_envelope(
        &geom,
        &suspension.0,
        spec.mass * derive::G,
        track.suspension.engage,
    ));

    // The Tiger's OWN drivetrain, built through the same validated seam the game builds it with
    // (`TrackSpec::transmission_params`) — the sandbox must be geared like the tank it is testing.
    // The seam now takes caller-supplied geometry, so the sandbox passes the MEASURED chord-exact
    // sprocket pitch radius (`geom.rest`'s first circle) and half-tread (`geom.plane_x`) — the same
    // values the game's `init_track_gear` feeds. (This closes the old τ-arc-vs-chord gap: the seam
    // used to size the sprocket by `pitch·teeth/τ`; both game and sandbox now use the chord circle.
    // The gap was feel-neutral anyway — reductions derive against the same radius, so it cancels.)
    let transmission = RigTransmission(
        track
            .transmission_params(geom.rest.get(Side::Right)[0].1, geom.plane_x)
            .expect("the Tiger's transmission block is validated at bake")
            .expect("the Tiger authors a transmission block"),
    );
    commands.insert_resource(TransState(
        crate::track::transmission::TransmissionState::from_spec(&transmission.0),
    ));
    commands.insert_resource(transmission);
    // Seed the adapter switch from the SPEC's declared architecture — the sandbox boots
    // geared exactly like the shipped tank (the shared `TransmissionArchitecture::mode`
    // mapping, so game and sandbox can never disagree on what a spec runs). A harness
    // `trans=` key or a live `T`/panel flip overrides it EXPLICITLY afterwards; before this
    // seed the switch is `None` and nothing sims (everything gates on `RigGeom`, which lands
    // in this same command flush).
    trans_switch.0 = Some(
        track
            .powertrain
            .transmission
            .as_ref()
            .expect("the Tiger's transmission block is validated at bake")
            .architecture
            .mode(),
    );

    // The pin belt IS the material loop now (see `PinBelt::for_rig`); the element slabs size from it
    // here rather than in a chained system — `contact_side` never resizes at runtime (the fixed-size
    // invariant), so empty slabs would silently skip traction.
    let pin_belt = PinBelt::for_rig(&geom);
    *grip_elements = BeltGripElements::sized(pin_belt.count);
    commands.insert_resource(pin_belt);

    // The hull's INERTIA box: the spec's authored extents (x, y, z full dimensions), used for the
    // angular inertia and nothing else. It is a mass-DISTRIBUTION knob — the number handling is
    // tuned through — not a silhouette, and the two answer different questions ("how is the mass
    // spread?" vs "where is the steel?"). This box used to double as the backstop collider's shape,
    // which coupled them: retuning yaw response would have silently moved the collision hull, and
    // the box was 3.0 m wide against a vehicle whose tracks span ±1.89 m. Collision now comes from
    // the asset's own authored `*_Collider` proxies (below), exactly as the game builds it.
    let inertia_box = Cuboid::new(
        spec.inertia_extents.0,
        spec.inertia_extents.1,
        spec.inertia_extents.2,
    );

    // Validate + read out the authored collision proxies before anything spawns on them: their
    // ground clearance is the number that decides whether the BELT still carries the tank, and a
    // proxy that hangs below the belt would take the load and quietly void the contact model.
    report_collision_proxies(&blueprint, &geom);

    // Wheels: a cylinder collider lying along X (the axle). Bevy's `Cylinder` is Y-up, so a −90°
    // turn about Z lays it along X.
    let axle = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

    let hull = commands
        .spawn((
            Hull,
            // At `hull_rest_y` the rest belt exactly kisses flat ground — the datum `RigGeom`
            // derives from the rest envelope's own lowest point, not an authored ride height.
            Transform::from_xyz(0.0, geom.hull_rest_y, 0.0),
            // The glb child renders the tank; the hull's own `Visibility` is what the `1` layer
            // toggles, and it needs to exist here for that inheritance to reach the model.
            Visibility::default(),
            RigidBody::Dynamic,
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            // The backstop colliders are *penetration stops only* — ALL tangential surface physics
            // (traction, grinding-climb, skid) belongs to the belt. Avian colliders default to
            // μ = 0.5, which silently made them frictional surfaces: pressed against a trench wall,
            // the collider contact dragged *down* with 0.5·N exactly as the belt tried to grind up
            // it, locking the climb (the harder the tracks pressed, the harder it dragged). Zero
            // friction with `Min` combine (outranks the terrain's default `Average`) so the combined
            // contact is frictionless regardless of terrain material.
            Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
            // Mass properties are authored, not derived from the colliders (`NoAuto*`), exactly as
            // the game does: the spec's combat mass in the spec's inertia box.
            Mass(spec.mass),
            AngularInertia::from_shape(&inertia_box, spec.mass),
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
        ))
        .with_children(|parent| {
            // The REAL Tiger, as the visual — the same scene the game loads.
            // Its mesh vertices carry their authored hull-local positions, so at the identity child
            // transform the model registers exactly with the derived geometry drawn over it.
            parent.spawn((
                WorldAssetRoot(
                    asset_server
                        .load(GltfAssetLabel::Scene(0).from_asset(crate::tank::TIGER_GLB_PATH)),
                ),
                Transform::IDENTITY,
                Visibility::default(),
            ));

            // Solid-body collision (walls, hard bottoming): the ASSET'S OWN authored proxies — the
            // `*_Collider` nodes the artist modelled, captured by `bake::captures_mesh` and turned
            // into convex hulls exactly the way `tank::spawn::assemble_tank_body` turns them (same
            // `Collider::convex_hull` over the same node-local POSITION buffers, which is precisely
            // what avian's `ConvexHullFromMesh` does). Identical construction is the point, not
            // tidiness: if the lab rig's collision shape differed from the shipped tank's, a feel
            // verdict taken here would not be a verdict about the game.
            //
            // The belt rays only probe DOWNWARD, so they cannot resist a vertical face — these
            // hulls are what stops the tank at a wall and what rests it when a trench bridge fails.
            // On normal terrain they sit clear of the ground and the belt carries the whole weight.
            for &index in &blueprint.geometry.collision_proxies {
                let node = &blueprint.geometry.nodes[index];
                for primitive in &node.primitives {
                    let points: Vec<Vec3> = primitive
                        .positions
                        .iter()
                        .copied()
                        .map(Vec3::from)
                        .collect();
                    let collider = Collider::convex_hull(points).unwrap_or_else(|| {
                        panic!(
                            "collision proxy `{}` has a degenerate hull source",
                            node.name
                        )
                    });
                    parent.spawn((
                        collider,
                        // FLATTENED to a hull-local pose instead of hung under a node entity. The
                        // game parents each proxy to its own glb node and lets avian's
                        // `ColliderTransform` compose the chain (which is how a scaled ancestor gets
                        // its scale onto the shape); the sandbox has no node hierarchy — the glb
                        // child above is a render-only `WorldAssetRoot` — so the baked root-relative
                        // pose is used directly. That is exact while the chain carries no scale
                        // (it doesn't: `Hull`→`Hull_Collider` and `Hull`→`Turret_Yaw`→
                        // `Turret_Collider` are all identity TRS), and `refuse_scaled_proxy` below
                        // makes a future scaled proxy loud instead of silently mis-sized. The turret
                        // proxy is frozen at its rest yaw, which is what the sandbox wants: there is
                        // no turret to traverse here.
                        Transform::from_translation(node.root_position)
                            .with_rotation(node.root_rotation),
                        CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
                        // Frictionless, like every backstop here — see the hull comment above.
                        Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
                    ));
                }
            }

            for side in Side::ALL {
                // The stored rest list is `[sprocket, road wheels…, idler]` on the PIN line, which
                // is exactly what the drive wheels need: the sprocket's chord-exact pitch circle and
                // the idler's measured rim pushed out to the pins.
                let rest = geom.rest.get(side);
                let sprocket = rest[0];
                let idler = *rest.last().expect("a side has a sprocket and an idler");

                // Road wheels carry NO mesh and NO collider — the glb draws them and the belt is the
                // sole ground-contact system. They exist as data carriers: `Suspension` for the
                // cosmetic articulation, `RigWheel` so the view systems can find them. Each sits at
                // the model's own node position, lateral `x` included (the Tiger's wheels are
                // interleaved across two lateral rows — they are NOT all on the track plane).
                for &centre in geom.road_wheels.get(side) {
                    parent.spawn((
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
                // hard-stop prism (`spawn_hard_stops`) is the whole rig's penetration backstop,
                // its end arcs riding the unsprung wheels' INNER (wheel-rim) surface.
                for (kind, (centre, _radius)) in
                    [(WheelKind::Sprocket, sprocket), (WheelKind::Idler, idler)]
                {
                    parent.spawn((
                        RigWheel { side, kind },
                        // Side-plane `(z, y)` back to 3-D on this side's track plane.
                        Transform::from_xyz(side.plane_x(geom.plane_x), centre.y, centre.x)
                            .with_rotation(axle),
                    ));
                }
            }
        })
        .id();

    // The RED hard-stop backstops — one convex-hull prism per side, cut from the compression-pose
    // INNER-surface (wheel-rim) wrap (see `RigGeom::hard_stop_polyline` — the full plate below it
    // is the support penalty's dig-in band). Spawned here so the rig is never up for a
    // frame without its bottoming stop; [`refresh_hard_stops`] recuts them on every retune. It
    // covers the whole belly the suspension bottoms onto — it is the rig's only penetration stop.
    spawn_hard_stops(&mut commands, hull, &geom, &suspension.0);

    // The authored counts as the panel/keyboard INTENT seam (see [`RigCounts`] / [`apply_rig_counts`]).
    // Lands in the same flush as `RigGeom`, so `is_added()` covers this first frame in the commit
    // system and no spurious rebuild fires.
    commands.insert_resource(RigCounts {
        link_count: geom.link_count,
        teeth: geom.teeth,
    });
    commands.insert_resource(geom);
}

/// Check and report the asset's authored collision proxies — the rig readout for the one thing the
/// belt model cannot tolerate getting wrong.
///
/// Two jobs, both about the flattening the spawn site performs (proxies land at their baked
/// root-relative pose instead of under their own glb node):
///
/// 1. **Refuse a scaled proxy.** `NodeGeometry::root_position`/`root_rotation` compose translation
///    and rotation only, and the flattened `Transform` carries no scale, so a proxy under a scaled
///    ancestor would be both mis-sized and mis-placed — invisibly, as a collision hull that is
///    merely a bit off. The game never has this problem (avian composes the real node chain), so
///    the constraint is the sandbox's alone and has to be checked here.
/// 2. **Print the ground clearance.** The proxies are penetration backstops; the BELT is the
///    contact model. A proxy hanging below the belt's rest envelope would touch flat ground and
///    carry the tank, silently replacing the whole model with a box on a floor. The readout puts
///    that margin (proxy bottom at rest, above y = 0) on screen every run, so the failure announces
///    itself instead of showing up as "the belt feels dead".
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

/// The hull-local convex-hull collider for one side's RED hard stop: the side's
/// [`RigGeom::hard_stop_polyline`] extruded across the track's TRUE lateral extent. The two x
/// positions are the EDGE grip columns ([`RigGeom::grip_columns`] index 0 and 2 — the measured shoe
/// faces), NOT `plane_x ± width/2`: the Tiger's shoe is authored ~17 mm outboard of the pin plane, so
/// the symmetric span would place the prism off the steel it is standing in for.
///
/// `None` on a degenerate point cloud — it never is for a real rig, but a retune into a broken count
/// should warn, not panic, so the caller can skip that side.
fn hard_stop_collider(geom: &RigGeom, side: Side, params: &SuspensionParams) -> Option<Collider> {
    let cols = geom.grip_columns(side);
    let (x_in, x_out) = (cols[0].0, cols[2].0);
    // Side-plane (z, y) → 3-D with the SAME mapping the wheel/cylinder spawns use
    // (`Transform::from_xyz(x, centre.y, centre.x)`): the polyline's `Vec2` is `(z, y)`. Extruding
    // each vertex to both faces turns the convex 2-D wrap into a convex prism.
    let points: Vec<Vec3> = geom
        .hard_stop_polyline(side, params)
        .into_iter()
        .flat_map(|p| [Vec3::new(x_in, p.y, p.x), Vec3::new(x_out, p.y, p.x)])
        .collect();
    Collider::convex_hull(points)
}

/// Spawn both sides' red hard-stop colliders as children of `hull`. Shared by [`build_rig`] (initial
/// spawn) and [`refresh_hard_stops`] (rebuild on retune), so the two can never cut the prism from the
/// geometry two different ways. Frictionless + `Layer::Vehicle` exactly like the authored proxies and
/// the drive-wheel backstops — a pure penetration stop; the belt owns all tangential physics.
fn spawn_hard_stops(
    commands: &mut Commands,
    hull: Entity,
    geom: &RigGeom,
    params: &SuspensionParams,
) {
    for side in Side::ALL {
        let Some(collider) = hard_stop_collider(geom, side, params) else {
            warn!("rig: hard-stop hull for {side:?} is degenerate — no backstop spawned");
            continue;
        };
        commands.spawn((
            HardStop,
            collider,
            // The extruded points are ALREADY full hull-local, so identity — there is no glb node
            // pose to compose (unlike the authored `*_Collider` proxies, which carry their node's
            // baked root-relative transform).
            Transform::IDENTITY,
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
            ChildOf(hull),
        ));
    }
}

/// Rebuild the red hard-stop colliders whenever the geometry they are cut from moves: a rig rebuild
/// (`;`/`'`, `n`/`m`, `R` — [`tune_rig_counts`] commits with `*geom = next`, so `is_changed` fires)
/// or the bump-stop knob ([`RigSuspension`], which sets the compression pose). The whole
/// prism is despawned and recut rather than reshaped in place — a collider's convex hull is not a
/// mutable field, and a retune is rare (a keypress), so the allocation is free. Mirrors
/// [`refresh_envelope`]'s change-gate; unordered against [`tune_rig_counts`] like it, so a retune is
/// reflected either this frame or the next.
fn refresh_hard_stops(
    mut commands: Commands,
    geom: Res<RigGeom>,
    suspension: Res<RigSuspension>,
    hull: Single<Entity, With<Hull>>,
    existing: Query<Entity, With<HardStop>>,
) {
    if !(geom.is_changed() || suspension.is_changed()) {
        return;
    }
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    spawn_hard_stops(&mut commands, *hull, &geom, &suspension.0);
}

/// Every piece of belt state that is INDEXED BY the loop — its link count (the grip-element slabs)
/// or its travel phase (the advected ring, the contacts/grip/dynamics that last tick's ring
/// produced, and the view wrap's filter memory), as ONE system param.
///
/// Bundled because there are two ways to invalidate exactly this set and they must agree:
/// teleporting the rig ([`reset_rig`]) and re-lengthening the loop under it ([`tune_rig_counts`]).
/// A named bundle makes "the belt-indexed set" a thing that exists once instead of a list two
/// systems have to keep in sync — and it keeps `reset_rig` under Bevy's 16-param ceiling.
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
    /// Back to a cold, correctly-sized belt for a loop of `link_count` links.
    ///
    /// The link-count knob in particular MUST re-size the slabs — `contact_side` never resizes
    /// at runtime (the fixed-size invariant), so a stale slab silently drops traction for the rest of
    /// the session.
    ///
    /// Not touched, deliberately: belt SPEED, the drive command, the transmission gear and the hull
    /// pose. None of them is indexed by the loop, and a count tweak should not stop a rolling tank
    /// (`reset_rig` clears those itself, because a teleport should).
    fn reseat(&mut self, link_count: usize) {
        *self.phase = BeltPhase::default();
        // Stale contacts/dynamics display the pre-teleport tick (codex parts-1/2 review #2), and
        // stale bristle strain is shear the pads never took.
        *self.contacts = BeltContacts::default();
        *self.dynamics = SideDynamics::default();
        *self.grip = BeltGrip::default();
        // Pre-sized, never `default()` — the fixed-size invariant (see `build_rig`).
        *self.elements = BeltGripElements::sized(link_count);
        // The kinematic-wrap filter memory is a configuration of a specific pose and loop; both
        // events invalidate it, so drop it and let the wrap re-init from the fresh state instead of
        // settling in from stale cells.
        self.wrap.reset();
    }
}

/// Set by the panel's Scene "Reset tank" button; consumed by [`reset_rig`] so the `R` key and the
/// button share ONE reset executor (the button has no way to hold `reset_rig`'s many `&mut` borrows
/// itself). Always present so `reset_rig`'s signature is `dev_ui`-independent.
#[derive(Resource, Default)]
struct ResetRequested(bool);

/// `R` (or the panel's Reset button) cycles the rig through the reset spots (flat → narrow trench →
/// wide trench → pit), dropping it at rest — the test tour in one key. The two triggers share this
/// one executor; the button raises [`ResetRequested`] and this consumes it.
fn reset_rig(
    keys: Res<ButtonInput<KeyCode>>,
    mut requested: ResMut<ResetRequested>,
    hull: Single<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity), With<Hull>>,
    pin_belt: Res<PinBelt>,
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
    let (z, label) = RESET_SPOTS[spot.0];
    spot.0 = (spot.0 + 1) % RESET_SPOTS.len();
    let (mut transform, mut lin, mut ang) = hull.into_inner();
    *transform = Transform::from_xyz(0.0, geom.hull_rest_y, z);
    lin.0 = Vec3::ZERO;
    ang.0 = Vec3::ZERO;
    *belt = BeltSpeed::default();
    // A reset is a fresh at-rest rig: stale command state would re-thrust it immediately.
    raw.0 = crate::track::drive::DriveAxes::default();
    shaped.0 = crate::track::drive::DriveAxes::default();
    belt_state.reseat(pin_belt.count);
    // A fresh rig is in 1st gear with no shift in flight.
    *trans_state = TransState(crate::track::transmission::TransmissionState::from_spec(
        &transmission.0,
    ));
    // Stale cosmetic wheel lift survives the teleport otherwise: for the first ~100 ms the
    // conform solves against phantom raised wheel circles while the hull settles.
    for mut susp in &mut wheels {
        susp.dy = 0.0;
        susp.dvel = 0.0;
    }
    info!("reset → {label} (z = {z:.1})");
}

/// How far past `[n_min, n_droop]` the link-count knob may roam, in LINKS. Sized in loop metres,
/// not taste: at the Tiger's 0.13 m pitch this is ±1.6 m of chain either side, which is enough to
/// walk into the `Impossible` regime and see the verdict flip, and enough to hang a frankly-sloppy
/// belt on the far side — while making "0 links" or "500 links" unreachable. The band is derived
/// from the LIVE window, so softening the springs widens it with the rig.
///
/// Note the two ends mean different things. Below `n_min` the knob is walking into a genuinely
/// invalid rig, on purpose, to show the fault. Above `n_droop` every count is VALID (springs limit
/// droop, belt carries slack) and the fence is pure taste — it just stops the knob wandering off
/// into belts no tensioner could take up. Widen it freely if a sloppier belt ever wants testing.
const LINK_TUNE_SLOP: i32 = 12;

/// Sprocket tooth-count clamp — the retired suspension editor's band, kept verbatim. Below ~6 the
/// chord-exact pitch radius `pitch/(2·sin(π/teeth))` stops describing a wheel; above ~40 the sprocket
/// is bigger than the hull.
const TEETH_TUNE_RANGE: (i32, i32) = (6, 40);

/// The authored geometry counts as a panel/keyboard INTENT seam. Both `;`/`'` `n`/`m`
/// ([`tune_rig_counts`]) and the panel's Tune-section steppers write here; [`apply_rig_counts`] is
/// the single commit path that turns a change into the rig rebuild. Splitting intent from commit is
/// what lets two surfaces drive the same knob without a second rebuild path — and keeps the heavy
/// `ResMut<RigGeom>` / `ResMut<PinBelt>` / [`BeltState`] borrows out of the egui panel.
///
/// `Copy + PartialEq` so a writer edits a local copy and stores back only on a real change — no
/// spurious [`RigGeom`] rebuild from an idle frame. `Default` (0/0) is a startup sentinel:
/// `init_resource` makes it exist from frame 0 (so the panel's `ResMut<RigCounts>` never faults on a
/// pre-rig frame), and `build_rig` overwrites it with the real authored counts a flush later — a
/// change `apply_rig_counts` absorbs without a rebuild because the counts then equal the geometry's.
#[derive(Resource, Clone, Copy, PartialEq, Default)]
struct RigCounts {
    link_count: usize,
    teeth: u32,
}

/// Clamp a desired link count into the live feasible band `[n_min − slop, n_droop + slop]`. The
/// band comes from the window at the CURRENT suspension knobs (not the build-time snapshot):
/// `n_min`/`n_droop` move with the springs, and a stale window would fence the user out of counts
/// that are feasible for the springs actually loaded. Shared by the keyboard and the panel stepper
/// so both offer the exact same reachable range (including the below-`n_min` slop that walks into
/// the `Impossible` regime on purpose — see [`LINK_TUNE_SLOP`]).
fn clamp_link_count(geom: &RigGeom, params: &SuspensionParams, desired: i32) -> usize {
    let window = geom.link_window(params);
    let lo = (window.n_min as i32 - LINK_TUNE_SLOP).max(1);
    let hi = window.n_droop as i32 + LINK_TUNE_SLOP;
    desired.clamp(lo, hi) as usize
}

/// Clamp a desired sprocket tooth count into [`TEETH_TUNE_RANGE`].
fn clamp_teeth(desired: i32) -> u32 {
    desired.clamp(TEETH_TUNE_RANGE.0, TEETH_TUNE_RANGE.1) as u32
}

/// `;` / `'` retune the LINK COUNT and `n` / `m` the sprocket TOOTH COUNT, live — the tuning loop the
/// whole derived-geometry contract exists to serve. The number you settle on is then hand-written
/// into `assets/tiger_1/tiger_1.tank.ron` (`track.link_count` / `track.sprocket.teeth`); there is
/// deliberately no save keybind, because that file is the GAME's tank too and the commit is the
/// user's call, not a keystroke's.
///
/// A pure INTENT writer now: it clamps and writes [`RigCounts`], and [`apply_rig_counts`] does the
/// rebuild. Each count is clamped only on the press that MOVES it, so a tooth press never silently
/// snaps a link count a suspension tweak has since left outside the band. Write-on-change so an idle
/// frame never marks `RigCounts` changed.
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

/// The single commit path for a count change (keyboard or panel): rebuild the rig whenever
/// [`RigCounts`] moves. What a commit has to move, in order:
///   1. [`RigGeom`] — rebuilt (never re-read from the glb; see [`RigGeom::rebuild`]) so `belt_len`,
///      the sprocket circle, the ride height and the stored `window` verdict all track the counts.
///   2. [`PinBelt`] — the physics loop IS `pitch × link_count` / `link_count`.
///   3. every belt-indexed piece of state, via [`BeltState::reseat`] — above all the grip-element
///      slabs, which are sized once and never re-sized by the force law.
///
/// Change-gated: skips the initial `RigCounts` insert (`is_added` — `build_rig` already built the
/// matching geometry) and any frame the counts already equal the geometry's, so no per-frame
/// rebuild can occur. `refresh_envelope` / `refresh_hard_stops` then react to the `RigGeom` change
/// as they do for a knob tweak.
fn apply_rig_counts(
    counts: Res<RigCounts>,
    suspension: Res<RigSuspension>,
    mut geom: ResMut<RigGeom>,
    mut pin_belt: ResMut<PinBelt>,
    mut belt_state: BeltState,
) {
    if counts.is_added() || !counts.is_changed() {
        return;
    }
    if counts.link_count == geom.link_count && counts.teeth == geom.teeth {
        return;
    }
    let params = &suspension.0;
    // `rebuild` borrows the old geometry, so the new one is finished before it replaces it.
    let next = geom.rebuild(counts.teeth, counts.link_count, params);
    *geom = next;
    *pin_belt = PinBelt::for_rig(&geom);
    belt_state.reseat(pin_belt.count);
    // Terminal trail of the tuning session — the panel shows the same verdict live, but the log is
    // what you scroll back through when picking the number to write into the RON.
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

/// `L` logs the current state — hull height, grounded stations, support vs weight, the belt speeds
/// vs the tank's actual forward speed (the gap is the slip / wheelspin), plus the model self-check
/// the old readout used to print: the taut-wrap-implied link count vs the authored one, and the
/// derived vehicle mass. It is the always-on "read the model as exact numbers, not eyeballed"
/// diagnostic — and the one non-`dev_ui` reader of [`derive::link_count`] and the derived model mass
/// now that the text HUD that printed them is the egui panel.
fn log_state(
    keys: Res<ButtonInput<KeyCode>>,
    hull: Single<(&Transform, &LinearVelocity), With<Hull>>,
    contacts: Res<BeltContacts>,
    belt: Res<BeltSpeed>,
    rig: Res<RigSpec>,
    geom: Res<RigGeom>,
    suspension: Res<RigSuspension>,
) {
    if !keys.just_pressed(KeyCode::KeyL) {
        return;
    }
    let (transform, lin) = *hull;
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

/// Draw the rig skeleton (hub markers) and the **conformed belt** of each side (`ConformedBelts`,
/// built by the active view system this frame): taut lower run raised onto any terrain it meets,
/// the drive-wheel arcs, and the sagging top run. Pure presentation; also the exact path the
/// procedural track will lay links along later.
fn draw_rig_gizmos(
    mut gizmos: Gizmos,
    wheels: Query<(&RigWheel, &GlobalTransform)>,
    belts: Res<ConformedBelts>,
    hull: Single<&GlobalTransform, With<Hull>>,
    viz: Res<VizLayers>,
    geom: Res<RigGeom>,
) {
    // Hub markers, coloured by role so the drive wheels (sprocket/idler) read apart from the road
    // wheels. `kind` is also the seam for later drive/animation (e.g. torque on the sprocket).
    if viz.hubs {
        for (wheel, gt) in &wheels {
            let color = match wheel.kind {
                WheelKind::Road => HUB_COLOR,
                WheelKind::Sprocket | WheelKind::Idler => DRIVE_HUB_COLOR,
            };
            gizmos.sphere(Isometry3d::from_translation(gt.translation()), 0.05, color);
        }
    }

    for side in Side::ALL {
        if viz.belt_line {
            let mut world = belts.get(side).iter().map(|s| s.world);
            gizmos.linestrip(world.clone(), BELT_COLOR);
            if let (Some(a), Some(b)) = (world.next_back(), world.next()) {
                gizmos.line(a, b, BELT_COLOR);
            }
        }

        // The conformed line is the *pin line* — draw the **outer face** (each sample offset by
        // its local outward normal × t/2, from neighbour tangents of the drawn belt) as a
        // dimmer companion, so the shoe thickness reads: the dark line rides the ground, the
        // wheels ride the light one.
        if !viz.outer {
            continue;
        }
        let samples = belts.get(side);
        let n = samples.len();
        if n < 3 {
            continue;
        }
        let affine = hull.affine();
        let track_x = side.plane_x(geom.plane_x);
        let outer: Vec<Vec3> = (0..n)
            .map(|i| {
                let tan2 = (samples[(i + 1) % n].local - samples[(i + n - 1) % n].local)
                    .normalize_or_zero();
                let out2 = Vec2::new(tan2.y, -tan2.x);
                let p = samples[i].local + out2 * (geom.thickness / 2.0);
                affine.transform_point3(Vec3::new(track_x, p.y, p.x))
            })
            .collect();
        gizmos.linestrip(
            outer.iter().copied().chain(outer.first().copied()),
            BELT_OUTER_COLOR,
        );
    }
}

/// Draw the live belt contact stations: a dot sized by load and coloured by **slip** (green =
/// gripping, red = sliding/wheelspin), transformed by the *current* hull pose so it rides the
/// interpolated rig; a short line along the support normal; and (the forces layer) the actual
/// applied forces as N-scaled arrows — support along the normal, traction in the contact plane.
fn draw_contacts(
    mut gizmos: Gizmos,
    hull: Single<&GlobalTransform, With<Hull>>,
    contacts: Res<BeltContacts>,
    viz: Res<VizLayers>,
    geom: Res<RigGeom>,
    law: Res<belt::EnvelopeLaw>,
) {
    if !(viz.dots || viz.normals || viz.forces) {
        return;
    }
    let hull = *hull;
    // The per-station spring constant of the envelope law: its calibrated per-metre stiffness ×
    // one station's arc length (the pitch).
    let k = law.stiffness_per_m * geom.pitch;
    for c in contacts.all() {
        let p = hull.transform_point(c.local);
        // elastic load / k ≈ the station's penetration (m) — a stable size cue (the damped
        // actual load would add velocity-driven size flicker).
        let r = 0.03 + (c.load_elastic / k).clamp(0.0, 0.1);
        if viz.dots {
            // Slip fraction 0→1 grades green (grip) to red (sliding at μ·load).
            let t = (c.slip.abs() / SLIP_SATURATION).clamp(0.0, 1.0);
            let color = Color::srgb(t, 1.0 - 0.7 * t, 0.2);
            gizmos.sphere(Isometry3d::from_translation(p), r, color);
        }
        if viz.normals {
            gizmos.line(p, p + c.normal * (0.15 + r), NORMAL_COLOR);
        }
        if viz.forces {
            gizmos.arrow(
                p,
                p + c.normal * (c.load * FORCE_VIZ_SCALE),
                SUPPORT_FORCE_COLOR,
            );
            if c.traction.length_squared() > 1.0 {
                gizmos.arrow(p, p + c.traction * FORCE_VIZ_SCALE, TRACTION_FORCE_COLOR);
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

const HUB_COLOR: Color = Color::srgb(1.0, 0.85, 0.2);
const DRIVE_HUB_COLOR: Color = Color::srgb(1.0, 0.45, 0.15);
const BELT_COLOR: Color = Color::srgb(0.2, 0.9, 1.0);
/// The outer-face companion line: dimmer/darker than the pin line, so the two parallel curves
/// read as inner vs ground face at a glance.
const BELT_OUTER_COLOR: Color = Color::srgb(0.1, 0.45, 0.55);
const NORMAL_COLOR: Color = Color::srgb(1.0, 0.9, 0.2);
/// Support-force arrows (the `8` layer): magenta, apart from every geometry colour.
const SUPPORT_FORCE_COLOR: Color = Color::srgb(0.95, 0.3, 0.9);
/// Traction (friction) force arrows: orange, the game's drive-force convention.
const TRACTION_FORCE_COLOR: Color = Color::srgb(1.0, 0.6, 0.1);
/// The collocation-station dots (the `9` layer): neutral grey-white when clear of terrain.
const CAST_COLOR: Color = Color::srgb(0.85, 0.85, 0.9);
/// The taut reference loop (the `-` layer): dim violet.
const REF_COLOR: Color = Color::srgb(0.7, 0.5, 1.0);
/// Metres of arrow per newton of contact force (~20 kN reads as 1 m). Typical per-station support
/// at rest is ~6 kN over ~45 grounded stations.
const FORCE_VIZ_SCALE: f32 = 1.0 / 20_000.0;

/// Whether the egui [`panel`] currently holds keyboard or pointer focus — written each frame by the
/// panel (under `dev_ui`), read by [`panel_capturing`] to gate the driving/camera input off so a
/// slider drag or a text field never leaks into the sim. Always present (default false) so those
/// gates behave identically when the panel is not compiled in.
#[derive(Resource, Default)]
struct PanelWantsInput {
    keyboard: bool,
    pointer: bool,
}

/// Run condition: an egui widget has focus, so player input must be suppressed this frame.
fn panel_capturing(want: Res<PanelWantsInput>) -> bool {
    want.keyboard || want.pointer
}

/// Read the driver's arrow-key intent as the RAW axes. Zeroed while the cursor is free
/// (paused / unfocused) so a released window doesn't keep driving.
fn read_drive_input(
    keys: Res<ButtonInput<KeyCode>>,
    cursors: Query<&CursorOptions>,
    mut input: ResMut<RawDriveInput>,
) {
    let locked = cursors
        .single()
        .map(|c| c.grab_mode == CursorGrabMode::Locked)
        .unwrap_or(false);
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
    camera: Single<&mut Transform, With<FreeFlyCam>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time<Real>>,
) {
    let mut transform = camera.into_inner();

    const SENS: f32 = 0.003;
    const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;
    let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
    yaw -= motion.delta.x * SENS;
    pitch = (pitch - motion.delta.y * SENS).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);

    const SPEED: f32 = 12.0;
    let forward = Vec3::from(transform.forward())
        .with_y(0.0)
        .normalize_or_zero();
    let right = Vec3::from(transform.right())
        .with_y(0.0)
        .normalize_or_zero();
    let mut dir = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        dir += forward;
    }
    if keys.pressed(KeyCode::KeyS) {
        dir -= forward;
    }
    if keys.pressed(KeyCode::KeyD) {
        dir += right;
    }
    if keys.pressed(KeyCode::KeyA) {
        dir -= right;
    }
    if keys.pressed(KeyCode::ShiftLeft) {
        dir += Vec3::Y;
    }
    if keys.pressed(KeyCode::ControlLeft) {
        dir -= Vec3::Y;
    }
    if dir != Vec3::ZERO {
        transform.translation += dir.normalize() * SPEED * time.delta_secs();
    }
}

/// Run condition: the cursor is captured (mouse-look active).
fn cursor_locked(cursors: Query<&CursorOptions>) -> bool {
    cursors
        .single()
        .map(|cursor| cursor.grab_mode == CursorGrabMode::Locked)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_geom::tiger_rig;

    /// The collider the sandbox extrudes must actually construct — a convex hull over the extruded
    /// wrap points, for both sides of the real Tiger. This is the one property a unit test can pin
    /// without an ECS: if `hard_stop_polyline` ever returned a degenerate cloud, this fails loudly
    /// instead of the spawn warning at runtime.
    #[test]
    fn the_hard_stop_collider_constructs_for_both_sides() {
        let p = SuspensionParams::default();
        let rig = tiger_rig();
        for side in Side::ALL {
            assert!(
                hard_stop_collider(&rig, side, &p).is_some(),
                "{side:?}: the extruded hard-stop hull must be a valid convex collider",
            );
        }
    }

    /// DECISION RECORD: the sprocket/idler backstop cylinders were retired (owner verdict,
    /// 2026-07-24) — the red hard-stop prism supersedes them. The prism's end arcs ride the
    /// unsprung wheels' INNER (wheel-rim) surface — the full plate keeps the support penalty's
    /// wall bite (the masking hazard the cylinders' 0.6 inset existed for) — and still sit
    /// well OUTSIDE the old minified cylinders, so the prism meets every obstacle long before
    /// a cylinder would have. The cylinders' one unshared strip — a ~17 mm slab inboard of the
    /// shoe's inboard face, an artefact of the symmetric `plane_x ± width/2` assumption —
    /// holds no track material; phantom coverage at the wrong radius is not a backstop. This
    /// test pins the radial supersession so a future geometry change that inverts it (a prism
    /// arc sinking inside the old cylinder surface) resurfaces the question loudly.
    #[test]
    fn the_red_prism_end_arcs_supersede_the_retired_backstop_cylinders() {
        let rig = tiger_rig();
        let rest = rig.rest.get(Side::Right);
        let sprocket = rest[0];
        let idler = *rest.last().unwrap();
        for (name, (_c, pin_r)) in [("sprocket", sprocket), ("idler", idler)] {
            let retired_cyl_r = pin_r * 0.6;
            let prism_arc_r = pin_r - rig.model.pin_to_inner;
            println!(
                "{name:<8} pin_r {pin_r:.4}  retired cyl_r {retired_cyl_r:.4}  \
                 prism inner-surface arc r {prism_arc_r:.4}",
            );
            assert!(
                prism_arc_r > retired_cyl_r + 0.05,
                "{name}: the red prism's end arc must sit well outside the retired cylinder \
                 surface ({prism_arc_r:.4} vs {retired_cyl_r:.4})",
            );
        }
    }
}
