//! Isolated continuous-track locomotion sandbox (ADR-0005).
//!
//! It owns a code-generated rig, course, and the belt-contact model (the field-belt, promoted
//! into the game as `track::forces`) so track work stays independent from the game's data-driven
//! simulation.

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

use crate::Layer;

// Shared course/rig/belt machinery lives here in `mod.rs`; the model's force and view systems
// live in `model4.rs` (the field-belt — the sandbox's single model, promoted into the game as
// `track::forces`).
mod harness;
mod model4;

use model4::{
    BeltPhase, RouteChain, TRACK_THICKNESS, TerrainField, apply_belt_support_field,
    articulate_wheels_field, conform_belts_field, conform_belts_field_chain, draw_sample_points,
    init_pin_belt,
};
// The pure track core (route geometry) — moved out for game promotion (architecture §2); the
// sandbox consumes it exactly as the game's view plugin will. Re-exported so the model
// submodules' `use super::*` keeps resolving.
pub(crate) use crate::track::oracle::{BlockField, TerrainBlock};
pub(crate) use crate::track::route::{
    arc, build_route, external_tangent, polyline_len, resample, sag_span,
};

// --- Rig geometry (metres), benchmarked on the **Soviet T-34** — well-documented numbers and a
// running-gear layout (5 big road wheels, rear-ish drive, all-steel track) essentially identical to
// this model. Nothing here is tuned-to-feel; it's the vehicle's spec sheet. ---

/// Number of road wheels per side (T-34: 5).
const ROAD_WHEELS: usize = 5;
/// Road-wheel radius. Also the effective belt half-thickness at the hub for now. (T-34's Christie
/// wheels are famously large: 830 mm diameter.)
const ROAD_RADIUS: f32 = 0.415;
/// Hub-to-hub spacing of road wheels along the track: T-34 ground-contact length ≈ 3.85 m over 4
/// gaps.
const WHEEL_SPACING: f32 = 0.96;
/// Sprocket/idler radius (T-34 sprocket ≈ 0.64 m diameter — *smaller* than its road wheels).
const DRIVE_RADIUS: f32 = 0.32;
/// The sprocket/idler *collider* radius as a fraction of the wheel radius. Deliberately inset inside
/// the belt surface: the **belt penalty is the primary contact** (it must be able to penetrate a wall
/// to generate support + grinding friction), and the collider is only a **hard backstop** against
/// fast-impact tunnelling the soft belt would miss. At 1.0 the collider pins the wheel exactly at the
/// belt surface and masks all belt-wall contact — which is the bug this inset fixes.
const DRIVE_COLLIDER_SCALE: f32 = 0.6;
/// How far ahead of the front road wheel (behind the rear) the sprocket/idler hub sits.
const DRIVE_OVERHANG: f32 = 1.0;
/// How far the sprocket/idler hub is raised above the road-wheel hub line (lifts the diagonal runs —
/// the surfaces that bridge a trench and climb a ledge).
const DRIVE_LIFT: f32 = 0.55;
/// Lateral offset of each track from the centreline (so the two belts straddle the hull).
const TRACK_HALF_WIDTH: f32 = 1.25;
/// Hull box half-extents (x = half width, y = half height, z = half length). T-34 hull ≈ 6.1 m long,
/// ≈ 2 m between the tracks.
const HULL_HALF: Vec3 = Vec3::new(1.0, 0.55, 3.05);
/// Hull centre height when resting on flat ground (road-wheel hubs sit at y = ROAD_RADIUS).
const HULL_REST_Y: f32 = 1.15;
/// Hull mass (kg): T-34/76, combat-loaded ≈ 26.5 t.
const HULL_MASS: f32 = 26_500.0;

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

// --- Belt contact model ---
/// Target arc-length spacing of belt contact stations (m) — the **track link pitch**. T-34: 172 mm,
/// 72 links per track (our slightly longer loop rounds to a few more). Because the coefficients
/// below are **per metre of belt**, changing this changes only resolution — never the total
/// support/traction (the fix for "finer spacing launched the rig").
const CONTACT_SPACING: f32 = 0.172;
/// Downward ray length used to find ground just beneath each station (m); also the sink at which
/// support saturates.
const CONTACT_PROBE: f32 = 0.5;
/// Slack (m) in the belt beyond the taut rest perimeter: the fixed track length is `rest perimeter +
/// this`. The leftover slack rests on the return (top) run as sag (depth ~√slack). The T-34 runs
/// famously loose — **no return rollers**, the return run lies on top of the road wheels — so the
/// budget is sized for the reference sag to dip past the wheel tops (~0.45 m below the taut top
/// line); the view's wheel-circle constraints then catch the drape and the track *rides the
/// wheels*, hanging in short spans between them.
const TRACK_SLACK: f32 = 0.13;
/// Contact-spring stiffness per **metre of belt** (N/m per m): the sole carrier holds the T-34's
/// 26.5 t at ~5 cm of sink over the ~7.7 m of grounded belt. Multiplied by the station's arc length
/// for the per-station value. Ride frequency ≈ √(g / sink) is mass-independent, so this generalizes:
/// pick a target sink, not a per-vehicle spring constant.
const SUPPORT_STIFFNESS_PER_M: f32 = 680_000.0;
/// Contact-spring damping per **metre of belt** (N·s/m per m): ~0.85 critical for the vertical mode
/// at the stiffness above (over-damping here just makes it sluggish).
const SUPPORT_DAMPING_PER_M: f32 = 80_000.0;
/// Soft-engagement depth (m): a station ramps its contact force in over the first this-many metres of
/// penetration (quadratic near zero) instead of switching full force on the instant it crosses the
/// belt surface. Kills the on/off flicker at the belt ends that see-saws the rigid rig at rest — the
/// principled fix, since a real track is compliant, not a hard edge. Well below the ~5 cm static sink,
/// so it doesn't change the resting height, only the behaviour right at the contact boundary.
const CONTACT_ENGAGE: f32 = 0.02;

/// Arc-length spacing (m) for the *drawn* belt spline: fine enough that a bump between two wheels is
/// sampled so the terrain-conform can raise the line onto it (finer = smoother drape, more rays).
const BELT_DRAW_SPACING: f32 = 0.1;

// --- Drive: belt-speed / slip model. Each track has a belt *speed*; friction comes from the slip
// between belt and ground, so wheelspin, skid, engine-braking, hill-hold, and top speed all emerge.
// Drivetrain benchmarked on the T-34's 500 hp V-2 diesel. The drivetrain is *vehicle* spec, not
// track-model spec. ---
/// Top belt surface speed (m/s) at full command — the governed top speed (T-34: ~53 km/h road).
const MAX_BELT_SPEED: f32 = 15.0;
/// Engine power available to one track (W): V-2 diesel, 373 kW total. The engine delivers a
/// **constant-power curve** — available force = power / belt speed — so
/// it's brutal at stall and tapers as the belt spins up; "full force at any speed" was what spun the
/// track up like a string.
const ENGINE_POWER: f32 = 186_500.0;
/// Low-speed torque cap per track (N): the constant-power curve would be infinite at stall; real
/// gearing caps it around the grip limit (μ·mg/2 ≈ 117 kN — 1st gear on a T-34 can just about spin
/// the tracks on hard ground).
const ENGINE_FORCE: f32 = 120_000.0;
/// Governor gain (N per m/s of belt-speed error): how hard the engine chases the commanded belt
/// speed, clamped to the available force. Also gives engine-braking when the command drops.
const BELT_GOVERNOR_GAIN: f32 = 60_000.0;
/// Effective linear inertia of one track's belt (kg): the belt itself (~1.2 t of steel on a T-34)
/// plus the reflected drivetrain inertia. Sets how quickly the belt spins up / down; smaller = more
/// responsive and more prone to wheelspin.
const BELT_INERTIA: f32 = 8_000.0;
/// Slip speed (m/s) at which ground friction saturates to μ·load. Below it grip is ~proportional to
/// slip (rolling); above it the track is sliding (the wheelspin/skid regime).
const SLIP_SATURATION: f32 = 0.4;
/// Coulomb coefficient: a station's total ground force is capped at μ·load (friction ellipse).
const MU: f32 = 0.9;
/// Lateral fraction of the friction ellipse — a track's turning-resistance coefficient vs its
/// longitudinal grip; the lower sideways budget is what lets the rig pivot (Wong/Merritt skid-steer).
const LATERAL_GRIP_RATIO: f32 = 0.55;

// --- Wheels carry NO force: the belt is the *sole* ground-contact system (carries the tank,
// tractions, does walls/gaps). The VISUAL data direction is wheels-first
// (`articulate_wheels_field` reads the terrain field directly, then the view fits the belt
// around the wheels — ground → wheels → belt, acyclic; the step-21 belt-first order was circular
// and the root of the wrong-side captures).
//
// Wheel smoothing is asymmetric and physical, replacing the step-21b critically-damped spring
// (explicit damping — divergent at 60 fps with 2ωΔt = 3, and smoothing the rise was wrong
// anyway: terrain forcing a wheel up is kinematic, lag reads as the board entering the wheel):
// a RISE is instant, a FALL is ballistic (gravity-limited). Zero tuning constants. ---
/// Clamp on the cosmetic lift (m): a tall obstacle can't fling the visual wheel arbitrarily far.
const SUSP_MAX_LIFT: f32 = 0.5;

/// The track-view A/B (`V`): the step-22 stateless kinematic wrap (default) vs the step-24 route
/// chain — same sim, same terrain, flip live and feel the difference. The chain (and this toggle)
/// gets deleted once the wrap wins the feel check.
#[derive(Resource)]
struct TrackViewMode {
    kinematic: bool,
}

impl Default for TrackViewMode {
    fn default() -> Self {
        Self { kinematic: true }
    }
}

/// Run condition: the kinematic-wrap view (wheels-first data direction).
fn view_kinematic(view: Res<TrackViewMode>) -> bool {
    view.kinematic
}

/// Run condition: the route-chain view (the A/B partner).
fn view_chain(view: Res<TrackViewMode>) -> bool {
    !view.kinematic
}

/// `V` flips the track view live (kinematic wrap ↔ route chain). The chain state is cleared so
/// the incoming chain solves fresh instead of waking a stale configuration.
fn toggle_view_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mut view: ResMut<TrackViewMode>,
    mut route_chain: ResMut<RouteChain>,
) {
    if !keys.just_pressed(KeyCode::KeyV) {
        return;
    }
    view.kinematic = !view.kinematic;
    *route_chain = RouteChain::default();
    info!(
        "track view → {}",
        if view.kinematic {
            "kinematic wrap (step 22)"
        } else {
            "route-chain (step 24: route tube, T-34 pin friction, pinch fuses)"
        }
    );
}

/// Which track a wheel belongs to. Left at −X, right at +X (matching the game's `TrackSide`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

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
/// toward a lower target (see the wheel-doctrine comment at [`SUSP_MAX_LIFT`]). Visual only — no
/// force.
#[derive(Component)]
struct Suspension {
    pivot_local: Vec3,
    dy: f32,
    dvel: f32,
    /// The raw lift target this frame (what the terrain/belt demands) — recorded so the harness
    /// can measure the fall lag directly.
    target: f32,
}

/// One station of the conformed belt: its hull-local side-plane position on the rigid reference loop
/// (z, y — *pre*-conform, used to tell the belly from the top run and to align with wheels) and its
/// conformed world position (raised onto terrain).
struct BeltSample {
    local: Vec2,
    world: Vec3,
}

/// Each side's conformed belt this frame — the belt path fitted around the articulated wheels and
/// conformed to terrain, in loop order. Built once per frame by the active view system
/// (`conform_belts_field` / `conform_belts_field_chain`); the drawn spline is exactly this.
#[derive(Resource, Default)]
struct ConformedBelts {
    left: Vec<BeltSample>,
    right: Vec<BeltSample>,
}

impl ConformedBelts {
    fn get(&self, side: Side) -> &[BeltSample] {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }
}

/// Marker for the hull body (the single dynamic rigid body, static for now in increment 1).
#[derive(Component)]
struct Hull;

/// How many frames (~2 s) the jitter probe remembers.
const JITTER_WINDOW: usize = 120;

/// Per-frame world-space samples of the jitter-suspect elements (ring buffers over
/// [`JITTER_WINDOW`] frames) — the element-first diagnosis instrument for the at-rest gizmo
/// jitter. `J` prints each element's
/// peak-to-peak amplitude: who actually moves, and by how much. Splits physics-side (hull pose)
/// from visual-side (conformed belt, wheel placement, contact-dot position/size) at a keypress.
#[derive(Resource, Default)]
struct JitterProbe {
    hull_y: std::collections::VecDeque<f32>,
    hull_pitch: std::collections::VecDeque<f32>,
    wheel_y: std::collections::VecDeque<f32>,
    belt_y: std::collections::VecDeque<f32>,
    dot_y: std::collections::VecDeque<f32>,
    dot_load: std::collections::VecDeque<f32>,
    /// Whole-ring channel: per-frame snapshot of every left-side conformed sample's world y,
    /// index-aligned across frames (the ring is index-stable at rest, which is when the probe is
    /// read; cleared if the sample count changes). Finds the worst-moving link *anywhere* on the
    /// loop — the "some links jump around" channel the single-spot channels can't see.
    ring_y: std::collections::VecDeque<Vec<f32>>,
    /// Latest frame's hull-local sample positions, to name where the worst link sits.
    ring_local: Vec<Vec2>,
}

/// The free-fly inspection camera (own copy, like `armor_sandbox`'s).
#[derive(Component)]
struct FreeFlyCam;

pub fn plugin(app: &mut App) {
    app.add_plugins(PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()))
        // Registers the `PhysicsGizmos` group for the collider-wireframe layer (`0`); starts
        // disabled in `configure_collider_gizmos`.
        .add_plugins(PhysicsDebugPlugin)
        .init_resource::<BeltContacts>()
        .init_resource::<SideDynamics>()
        .init_resource::<Paused>()
        .init_resource::<ResetSpot>()
        .init_resource::<RawDriveInput>()
        .init_resource::<ShapedDrive>()
        .init_resource::<BeltSpeed>()
        .init_resource::<BeltPhase>()
        .init_resource::<ConformedBelts>()
        .init_resource::<JitterProbe>()
        .init_resource::<VizLayers>()
        .init_resource::<ChainReference>()
        .init_resource::<TerrainField>()
        .init_resource::<RouteChain>()
        .init_resource::<TrackViewMode>()
        .add_systems(
            Startup,
            (
                spawn_camera,
                // A harness run must not steal the user's cursor while it captures.
                grab_cursor.run_if(not(resource_exists::<harness::Harness>)),
                spawn_environment,
                spawn_rig,
                init_pin_belt,
                spawn_viz_label,
                configure_collider_gizmos,
            ),
        )
        // Physics runs in the fixed step (before Avian integrates in FixedPostUpdate), NOT while
        // paused (else penalty force accumulates against a frozen sim and flings the rig on resume).
        //
        // `apply_belt_support_field`: the advected pin-line ring, penetration from the analytic
        // terrain field at fixed collocation stations (no narrow-phase queries).
        .add_systems(FixedUpdate, apply_belt_support_field.run_if(sim_running))
        .add_systems(
            Update,
            (
                fly_camera.run_if(cursor_locked),
                read_drive_input,
                // The visual chain, in data order — wheels-FIRST in BOTH views (ground → wheels →
                // belt, acyclic): the wheels read the field, then the wrap fits — or the
                // route-chain solves — around them (step 23: the chain↔wheel circular dependency
                // is gone). The stateful pieces gate on `sim_running` like the physics — Esc
                // pauses Avian's clock but NOT the Update schedule, so ungated they kept easing
                // wheels / re-solving the chain against a frozen sim ("deforms while paused" —
                // the second clock). The draw systems stay ungated: gizmos are immediate-mode and
                // must redraw the frozen state.
                (
                    articulate_wheels_field.run_if(sim_running),
                    conform_belts_field
                        .run_if(view_kinematic)
                        .run_if(sim_running),
                    conform_belts_field_chain
                        .run_if(view_chain)
                        .run_if(sim_running),
                    // Probe after the visual chain settles this frame's state, frozen while paused
                    // (constant samples would dilute the window).
                    sample_jitter_probe.run_if(sim_running),
                    draw_rig_gizmos,
                )
                    .chain(),
                toggle_view_mode,
                toggle_pause,
                reset_rig,
                log_state,
                report_jitter_probe,
                draw_contacts,
                // The viz-layer instrumentation: toggles, legend, mesh/collider mirrors, and the
                // diagnostic layers (collocation stations at the physics ring, reference ring).
                toggle_viz_layers,
                update_viz_label.run_if(resource_changed::<VizLayers>),
                apply_mesh_visibility.run_if(resource_changed::<VizLayers>),
                sync_collider_gizmos.run_if(resource_changed::<VizLayers>),
                draw_sample_points,
                draw_chain_reference,
            ),
        );

    // The scripted capture harness (`SANDBOX_HARNESS` env var): scenario in, JSONL out, exit.
    // Bit-REPEATABLE (step 25b): virtual time advances exactly one fixed tick per rendered frame
    // (wall clock never enters the sim), and the scripted throttle is written INSIDE FixedUpdate
    // before the force systems — its phase boundaries land on exact ticks. Without both, frame
    // pacing leaked into recorded trajectories (~mm-level hull drift between identical runs) and
    // A/B gates could only ever be statistical.
    if let Some(scenario) = harness::parse_env() {
        app.insert_resource(scenario)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_micros(15_625), // exactly 1/64 s
            ))
            .add_systems(
                Startup,
                harness::harness_setup
                    .after(spawn_rig)
                    .after(spawn_environment),
            )
            .add_systems(
                FixedUpdate,
                harness::harness_drive.before(apply_belt_support_field),
            )
            .add_systems(
                FixedUpdate,
                harness::harness_record.after(apply_belt_support_field),
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
struct BeltContacts([Vec<Contact>; 2]);

impl BeltContacts {
    fn all(&self) -> impl Iterator<Item = &Contact> {
        self.0.iter().flatten()
    }
}

/// Per-side belt-dynamics telemetry from the core report: engine force applied and ground
/// reaction, `[left, right]`. Harness rows only.
#[derive(Resource, Default)]
struct SideDynamics {
    engine: [f32; 2],
    reaction: [f32; 2],
}

/// Whether the sim is frozen (`Esc`). The belt model gates on this so it doesn't accumulate force
/// against a paused physics world.
#[derive(Resource, Default)]
struct Paused(bool);

fn sim_running(paused: Res<Paused>) -> bool {
    !paused.0
}

/// Per-layer visibility switches for every visual element in the sandbox, each on its own key
/// (number row; see [`viz_label_text`] for the legend). Defaults reproduce the pre-toggle look;
/// the diagnostic layers (forces, cast shapes, colliders, reference ring) start off.
#[derive(Resource)]
struct VizLayers {
    /// `1` — the hull's render mesh.
    hull: bool,
    /// `2` — the wheel render meshes.
    wheels: bool,
    /// `3` — the conformed belt/chain line (the pin line).
    chain: bool,
    /// `4` — the outer-face companion line.
    outer: bool,
    /// `5` — the hub marker spheres.
    hubs: bool,
    /// `6` — the contact dots (load-sized, slip-coloured).
    dots: bool,
    /// `7` — the contact-normal lines.
    normals: bool,
    /// `8` — force vectors per contact: support (magenta) + traction (orange), N-scaled.
    forces: bool,
    /// `9` — the collocation stations at the *physics* ring (where the physics thinks the shoes
    /// are, vs the drawn view).
    casts: bool,
    /// `0` — Avian collider wireframes (hull box, drive-wheel backstops, terrain).
    colliders: bool,
    /// `-` — the taut reference loop (the belt's rest path, vs the conformed/solved view).
    reference: bool,
}

impl Default for VizLayers {
    fn default() -> Self {
        Self {
            hull: true,
            wheels: true,
            chain: true,
            outer: true,
            hubs: true,
            dots: true,
            normals: true,
            forces: false,
            casts: false,
            colliders: false,
            reference: false,
        }
    }
}

fn toggle_viz_layers(keys: Res<ButtonInput<KeyCode>>, mut viz: ResMut<VizLayers>) {
    type Field = fn(&mut VizLayers) -> &mut bool;
    const TOGGLES: [(KeyCode, Field); 11] = [
        (KeyCode::Digit1, |v| &mut v.hull),
        (KeyCode::Digit2, |v| &mut v.wheels),
        (KeyCode::Digit3, |v| &mut v.chain),
        (KeyCode::Digit4, |v| &mut v.outer),
        (KeyCode::Digit5, |v| &mut v.hubs),
        (KeyCode::Digit6, |v| &mut v.dots),
        (KeyCode::Digit7, |v| &mut v.normals),
        (KeyCode::Digit8, |v| &mut v.forces),
        (KeyCode::Digit9, |v| &mut v.casts),
        (KeyCode::Digit0, |v| &mut v.colliders),
        (KeyCode::Minus, |v| &mut v.reference),
    ];
    for (key, field) in TOGGLES {
        if keys.just_pressed(key) {
            let flag = field(&mut viz);
            *flag = !*flag;
        }
    }
}

/// Mirror the mesh layers onto the render entities. The wheels are children of the hull, so a
/// hidden hull would inherit-hide them; `Visibility::Visible` is the unconditional override that
/// keeps wheels drawable with the hull mesh off.
fn apply_mesh_visibility(
    viz: Res<VizLayers>,
    mut hull: Query<&mut Visibility, (With<Hull>, Without<RigWheel>)>,
    mut wheels: Query<&mut Visibility, With<RigWheel>>,
) {
    for mut v in &mut hull {
        *v = if viz.hull {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    for mut v in &mut wheels {
        *v = if viz.wheels {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Avian's `PhysicsGizmos` group (collider wireframes) starts silent; the `0` layer enables it.
fn configure_collider_gizmos(mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<PhysicsGizmos>().0.enabled = false;
}

fn sync_collider_gizmos(viz: Res<VizLayers>, mut store: ResMut<GizmoConfigStore>) {
    store.config_mut::<PhysicsGizmos>().0.enabled = viz.colliders;
}

/// The taut reference loop in world space — the belt's rest path around the articulated wheels.
/// Written by the view systems, drawn by the `-` layer: belt-vs-reference deviation shows where
/// terrain, slack, and whip hold the belt off its rest path.
#[derive(Resource, Default)]
pub(super) struct ChainReference {
    pub(super) left: Vec<Vec3>,
    pub(super) right: Vec<Vec3>,
}

fn draw_chain_reference(mut gizmos: Gizmos, reference: Res<ChainReference>, viz: Res<VizLayers>) {
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
struct BeltSpeed {
    left: f32,
    right: f32,
}

impl BeltSpeed {
    fn get(&self, side: Side) -> f32 {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }
    fn set(&mut self, side: Side, value: f32) {
        match side {
            Side::Left => self.left = value,
            Side::Right => self.right = value,
        }
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
    // Single camera for now — a render-layer-scoped UI camera slots in when we add readouts (as
    // `armor_sandbox` does), not a bare second 3D camera (which would re-render the scene).
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(11.0, 3.5, 3.0).looking_at(Vec3::new(0.0, 0.8, 0.0), Vec3::Y),
        FreeFlyCam,
    ));
}

/// The on-screen viz-layer legend + key reference (top-left).
#[derive(Component)]
struct VizLabel;

fn viz_label_text(viz: &VizLayers) -> String {
    fn s(on: bool) -> &'static str {
        if on { "ON " } else { "off" }
    }
    format!(
        "viz  1 hull:{}  2 wheels:{}  3 chain:{}  4 outer:{}  5 hubs:{}  6 dots:{}\n     \
         7 normals:{}  8 forces:{}  9 casts:{}  0 colliders:{}  - reference:{}\n\
         esc pause/cursor | v view (wrap/chain) | r reset | l log | j probe | arrows drive | wasd fly",
        s(viz.hull),
        s(viz.wheels),
        s(viz.chain),
        s(viz.outer),
        s(viz.hubs),
        s(viz.dots),
        s(viz.normals),
        s(viz.forces),
        s(viz.casts),
        s(viz.colliders),
        s(viz.reference),
    )
}

fn spawn_viz_label(mut commands: Commands, viz: Res<VizLayers>) {
    commands.spawn((
        VizLabel,
        Text::new(viz_label_text(&viz)),
        TextFont {
            font_size: FontSize::Px(13.0),
            ..default()
        },
        TextColor(Color::srgb(0.6, 0.75, 0.8)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

fn update_viz_label(viz: Res<VizLayers>, label: Single<&mut Text, With<VizLabel>>) {
    label.into_inner().0 = viz_label_text(&viz);
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

/// Spawn the code-generated primitive rig: a hull box with two tracks of wheels (sprocket + N road
/// wheels + idler) as children. Static in increment 1 — it just sits at the resting pose so we can
/// see the running gear and the belt envelope drawn over it.
fn spawn_rig(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let hull_mesh = meshes.add(Cuboid::new(
        HULL_HALF.x * 2.0,
        HULL_HALF.y * 2.0,
        HULL_HALF.z * 2.0,
    ));
    let hull_mat = materials.add(Color::srgb(0.30, 0.33, 0.30));
    // Wheels: a short cylinder lying along X (the axle). Bevy's `Cylinder` is Y-up, so a −90° turn
    // about Z lays it along X.
    let road_mesh = meshes.add(Cylinder::new(ROAD_RADIUS, TRACK_HALF_WIDTH * 0.25));
    let drive_mesh = meshes.add(Cylinder::new(DRIVE_RADIUS, TRACK_HALF_WIDTH * 0.25));
    let road_mat = materials.add(Color::srgb(0.18, 0.19, 0.20));
    let drive_mat = materials.add(Color::srgb(0.24, 0.22, 0.16));
    let axle = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

    // Road-wheel hub line, centred on the hull, sitting at y = ROAD_RADIUS above the ground. In
    // hull-local space the hull centre is HULL_REST_Y up, so local y of a hub = ROAD_RADIUS − rest.
    let span = (ROAD_WHEELS as f32 - 1.0) * WHEEL_SPACING;
    let hub_local_y = ROAD_RADIUS - HULL_REST_Y;

    let mut wheels: Vec<(
        Side,
        WheelKind,
        f32,
        Vec3,
        Handle<Mesh>,
        Handle<StandardMaterial>,
    )> = Vec::new();
    for side in [Side::Left, Side::Right] {
        let x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        // Road wheels front (−Z) to rear (+Z).
        for i in 0..ROAD_WHEELS {
            let z = -span / 2.0 + i as f32 * WHEEL_SPACING;
            wheels.push((
                side,
                WheelKind::Road,
                ROAD_RADIUS,
                Vec3::new(x, hub_local_y, z),
                road_mesh.clone(),
                road_mat.clone(),
            ));
        }
        // Sprocket at the front, idler at the rear — overhung and lifted.
        let drive_local_y = hub_local_y + DRIVE_LIFT;
        wheels.push((
            side,
            WheelKind::Sprocket,
            DRIVE_RADIUS,
            Vec3::new(x, drive_local_y, -span / 2.0 - DRIVE_OVERHANG),
            drive_mesh.clone(),
            drive_mat.clone(),
        ));
        wheels.push((
            side,
            WheelKind::Idler,
            DRIVE_RADIUS,
            Vec3::new(x, drive_local_y, span / 2.0 + DRIVE_OVERHANG),
            drive_mesh.clone(),
            drive_mat.clone(),
        ));
    }

    commands
        .spawn((
            Hull,
            Mesh3d(hull_mesh),
            MeshMaterial3d(hull_mat),
            Transform::from_xyz(0.0, HULL_REST_Y, 0.0),
            RigidBody::Dynamic,
            // Solid-body collision (walls, hard bottoming) via a hull box + the sprocket/idler
            // cylinders below — the belt rays only probe downward and can't resist a vertical face.
            // At ride height these all sit above the ground, so the belt still carries the tank on
            // normal terrain (ADR-0005: the hull box is a collision shape + bottoming floor).
            Collider::cuboid(HULL_HALF.x * 2.0, HULL_HALF.y * 2.0, HULL_HALF.z * 2.0),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            // The backstop colliders are *penetration stops only* — ALL tangential surface physics
            // (traction, grinding-climb, skid) belongs to the belt. Avian colliders default to
            // μ = 0.5, which silently made them frictional surfaces: pressed against a trench wall,
            // the collider contact dragged *down* with 0.5·N exactly as the belt tried to grind up
            // it, locking the climb (the harder the tracks pressed, the harder it dragged). Zero
            // friction with `Min` combine (outranks the terrain's default `Average`) so the combined
            // contact is frictionless regardless of terrain material.
            Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
            // Mass properties are authored, not derived from the colliders (`NoAuto*`), as the game
            // does: a box of the hull extents at `HULL_MASS`.
            Mass(HULL_MASS),
            AngularInertia::from_shape(
                &Cuboid::new(HULL_HALF.x * 2.0, HULL_HALF.y * 2.0, HULL_HALF.z * 2.0),
                HULL_MASS,
            ),
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
        ))
        .with_children(|parent| {
            for (side, kind, radius, pos, mesh, mat) in wheels {
                let mut wheel = parent.spawn((
                    RigWheel { side, kind },
                    Mesh3d(mesh),
                    MeshMaterial3d(mat),
                    Transform::from_translation(pos).with_rotation(axle),
                ));
                // The sprocket and idler are rigidly fixed to the hull and are the track's front-most
                // and rear-most points, so a cylinder collider on each extends the collision silhouette
                // to the actual track ends — the tank stops where the track meets a wall, not where the
                // inset hull box does. Road wheels get none (they'll articulate on suspension later).
                // The entity's `axle` rotation lays the Y-cylinder along X, matching the mesh.
                if matches!(kind, WheelKind::Sprocket | WheelKind::Idler) {
                    wheel.insert((
                        Collider::cylinder(radius * DRIVE_COLLIDER_SCALE, TRACK_HALF_WIDTH * 0.25),
                        CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
                        // Pure penetration stop, like the hull box: frictionless so the belt owns
                        // all tangential physics (see the hull collider comment).
                        Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
                    ));
                }
                // Road wheels get a cosmetic placement state (they carry no force — the belt does).
                if matches!(kind, WheelKind::Road) {
                    wheel.insert(Suspension {
                        pivot_local: pos,
                        dy: 0.0,
                        dvel: 0.0,
                        target: 0.0,
                    });
                }
            }
        });
}

/// `R` cycles the rig through the reset spots (flat → narrow trench → wide trench → pit), dropping it
/// at rest — the test tour in one key.
fn reset_rig(
    keys: Res<ButtonInput<KeyCode>>,
    hull: Single<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity), With<Hull>>,
    mut spot: ResMut<ResetSpot>,
    mut belt: ResMut<BeltSpeed>,
    mut phase: ResMut<BeltPhase>,
    mut wheels: Query<&mut Suspension>,
) {
    if !keys.just_pressed(KeyCode::KeyR) {
        return;
    }
    let (z, label) = RESET_SPOTS[spot.0];
    spot.0 = (spot.0 + 1) % RESET_SPOTS.len();
    let (mut transform, mut lin, mut ang) = hull.into_inner();
    *transform = Transform::from_xyz(0.0, HULL_REST_Y, z);
    lin.0 = Vec3::ZERO;
    ang.0 = Vec3::ZERO;
    *belt = BeltSpeed::default();
    *phase = BeltPhase::default();
    // Stale cosmetic wheel lift survives the teleport otherwise: for the first ~100 ms the
    // conform solves against phantom raised wheel circles while the hull settles.
    for mut susp in &mut wheels {
        susp.dy = 0.0;
        susp.dvel = 0.0;
        susp.target = 0.0;
    }
    info!("reset → {label} (z = {z:.1})");
}

/// `L` logs the current state — hull height, grounded stations, support vs weight, and the belt
/// speeds vs the tank's actual forward speed (the gap between them is the slip / wheelspin) — so the
/// model can be read as exact numbers, not eyeballed.
fn log_state(
    keys: Res<ButtonInput<KeyCode>>,
    hull: Single<(&Transform, &LinearVelocity), With<Hull>>,
    contacts: Res<BeltContacts>,
    belt: Res<BeltSpeed>,
) {
    if !keys.just_pressed(KeyCode::KeyL) {
        return;
    }
    let (transform, lin) = *hull;
    let count = contacts.all().count();
    let total: f32 = contacts.all().map(|c| c.load).sum();
    let weight = HULL_MASS * 9.81;
    let speed = lin.0.dot(transform.forward().into());
    info!(
        "hull y = {:.3} m | stations = {count} | support = {:.0}% of weight | belt L/R = {:.1}/{:.1} m/s | tank = {:.1} m/s",
        transform.translation.y,
        100.0 * total / weight,
        belt.left,
        belt.right,
        speed,
    );
}

/// Sample the jitter suspects once per frame (see [`JitterProbe`]): hull pose (physics side), and —
/// on the left track, at hull-local z ≈ 0, so every channel watches the same spot — the articulated
/// wheel placement, the conformed belt sample, and the contact dot's drawn position + displayed
/// load (visual side). Elements picked spatially, not by index, so the advected rings don't rotate
/// the watched element away.
fn sample_jitter_probe(
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Suspension)>,
    belts: Res<ConformedBelts>,
    contacts: Res<BeltContacts>,
    mut probe: ResMut<JitterProbe>,
) {
    let gt = *hull;
    let affine = gt.affine();
    fn push(buf: &mut std::collections::VecDeque<f32>, v: f32) {
        buf.push_back(v);
        if buf.len() > JITTER_WINDOW {
            buf.pop_front();
        }
    }
    push(&mut probe.hull_y, gt.translation().y);
    push(
        &mut probe.hull_pitch,
        gt.rotation().to_euler(EulerRot::YXZ).1,
    );

    // The left-side road wheel nearest the hull centre, at its current articulated placement.
    let wheel = wheels
        .iter()
        .filter(|(w, _)| w.side == Side::Left && w.kind == WheelKind::Road)
        .min_by(|(_, a), (_, b)| a.pivot_local.z.abs().total_cmp(&b.pivot_local.z.abs()))
        .map(|(_, s)| affine.transform_point3(s.pivot_local + Vec3::Y * s.dy).y);
    push(&mut probe.wheel_y, wheel.unwrap_or(f32::NAN));

    // The left belly sample nearest hull-local z = 0 (under the hull centre).
    let hub_y = ROAD_RADIUS - HULL_REST_Y;
    let belt = belts
        .get(Side::Left)
        .iter()
        .filter(|s| s.local.y < hub_y)
        .min_by(|a, b| a.local.x.abs().total_cmp(&b.local.x.abs()))
        .map(|s| s.world.y);
    push(&mut probe.belt_y, belt.unwrap_or(f32::NAN));

    // The left contact dot nearest hull-local z = 0, where it's drawn (current pose), and its
    // displayed load (the dot/normal size — the "force gizmo" flicker channel).
    let dot = contacts.0[0]
        .iter()
        .min_by(|a, b| a.local.z.abs().total_cmp(&b.local.z.abs()));
    push(
        &mut probe.dot_y,
        dot.map_or(f32::NAN, |c| gt.transform_point(c.local).y),
    );
    push(&mut probe.dot_load, dot.map_or(f32::NAN, |c| c.load));

    // Whole left ring, index-aligned (see the field doc).
    let ring: Vec<f32> = belts.get(Side::Left).iter().map(|s| s.world.y).collect();
    if probe.ring_y.front().is_some_and(|f| f.len() != ring.len()) {
        probe.ring_y.clear();
    }
    probe.ring_y.push_back(ring);
    if probe.ring_y.len() > JITTER_WINDOW {
        probe.ring_y.pop_front();
    }
    probe.ring_local = belts.get(Side::Left).iter().map(|s| s.local).collect();
}

/// `J` prints the probe: peak-to-peak amplitude of each watched element over the ring window.
/// Position channels in mm, pitch in degrees, load as ± percent of its mean.
fn report_jitter_probe(keys: Res<ButtonInput<KeyCode>>, probe: Res<JitterProbe>) {
    if !keys.just_pressed(KeyCode::KeyJ) {
        return;
    }
    fn p2p(buf: &std::collections::VecDeque<f32>) -> f32 {
        let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
        for &v in buf {
            if !v.is_nan() {
                min = min.min(v);
                max = max.max(v);
            }
        }
        if max >= min { max - min } else { 0.0 }
    }
    let (mut sum, mut cnt) = (0.0_f32, 0u32);
    for &v in &probe.dot_load {
        if !v.is_nan() {
            sum += v;
            cnt += 1;
        }
    }
    let load_pct = if cnt > 0 && sum > 0.0 {
        p2p(&probe.dot_load) / (sum / cnt as f32) * 50.0 // half the p2p, as ±%
    } else {
        0.0
    };
    info!(
        "jitter p2p over {} frames: hull y {:.3} mm | hull pitch {:.4}° | wheel y {:.3} mm | belt y {:.3} mm | dot y {:.3} mm | dot load ±{:.1}%",
        probe.hull_y.len(),
        p2p(&probe.hull_y) * 1000.0,
        p2p(&probe.hull_pitch).to_degrees(),
        p2p(&probe.wheel_y) * 1000.0,
        p2p(&probe.belt_y) * 1000.0,
        p2p(&probe.dot_y) * 1000.0,
        load_pct,
    );

    // Whole-ring sweep: per-sample p2p over the window; the worst link + how many are visibly live.
    let m = probe.ring_y.front().map_or(0, |f| f.len());
    if m == 0 || probe.ring_local.len() != m {
        return;
    }
    let (mut worst, mut worst_i, mut over) = (0.0_f32, 0usize, 0u32);
    for i in 0..m {
        let (mut mn, mut mx) = (f32::INFINITY, f32::NEG_INFINITY);
        for frame in &probe.ring_y {
            mn = mn.min(frame[i]);
            mx = mx.max(frame[i]);
        }
        let p = mx - mn;
        if p > worst {
            worst = p;
            worst_i = i;
        }
        if p > 0.0005 {
            over += 1;
        }
    }
    let at = probe.ring_local[worst_i];
    info!(
        "ring sweep ({m} links): worst link y {:.3} mm at hull-local (z {:.2}, y {:.2}) | {over} links > 0.5 mm",
        worst * 1000.0,
        at.x,
        at.y,
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

    for side in [Side::Left, Side::Right] {
        if viz.chain {
            let mut world = belts.get(side).iter().map(|s| s.world);
            gizmos.linestrip(world.clone(), BELT_COLOR);
            if let (Some(a), Some(b)) = (world.next_back(), world.next()) {
                gizmos.line(a, b, BELT_COLOR);
            }
        }

        // The conformed line is the *pin line* — draw the **outer face** (each sample offset by
        // its local outward normal × t/2, from neighbour tangents of the solved chain) as a
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
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        let outer: Vec<Vec3> = (0..n)
            .map(|i| {
                let tan2 = (samples[(i + 1) % n].local - samples[(i + n - 1) % n].local)
                    .normalize_or_zero();
                let out2 = Vec2::new(tan2.y, -tan2.x);
                let p = samples[i].local + out2 * (TRACK_THICKNESS / 2.0);
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
) {
    if !(viz.dots || viz.normals || viz.forces) {
        return;
    }
    let hull = *hull;
    let k = SUPPORT_STIFFNESS_PER_M * CONTACT_SPACING;
    for c in contacts.all() {
        let p = hull.transform_point(c.local);
        // load / k ≈ the station's penetration (m) — a stable size cue for the contact.
        let r = 0.03 + (c.load / k).clamp(0.0, 0.1);
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

/// The sprocket (front) and idler (rear) circles in hull-local side-plane (z, y) + radius. Fixed to
/// the hull (they never articulate), so they anchor every belt-length computation.
fn drive_circles_local() -> ((Vec2, f32), (Vec2, f32)) {
    let span = (ROAD_WHEELS as f32 - 1.0) * WHEEL_SPACING;
    let drive_y = (ROAD_RADIUS - HULL_REST_Y) + DRIVE_LIFT;
    (
        (
            Vec2::new(-span / 2.0 - DRIVE_OVERHANG, drive_y),
            DRIVE_RADIUS,
        ),
        (
            Vec2::new(span / 2.0 + DRIVE_OVERHANG, drive_y),
            DRIVE_RADIUS,
        ),
    )
}

/// The rest-pose circles of one side (front→rear) in hull-local side-plane (z, y) + radius, computed
/// from the fixed geometry — used once at startup to fix the belt length from the taut perimeter.
fn rest_circles() -> Vec<(Vec2, f32)> {
    let span = (ROAD_WHEELS as f32 - 1.0) * WHEEL_SPACING;
    let hub_y = ROAD_RADIUS - HULL_REST_Y;
    let (sprocket, idler) = drive_circles_local();
    let mut circles = vec![sprocket];
    for i in 0..ROAD_WHEELS {
        circles.push((
            Vec2::new(-span / 2.0 + i as f32 * WHEEL_SPACING, hub_y),
            ROAD_RADIUS,
        ));
    }
    circles.push(idler);
    circles
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
