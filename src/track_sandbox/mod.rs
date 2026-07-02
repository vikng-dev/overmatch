//! The track-model sandbox — an isolated tool to develop the continuous-track locomotion model
//! (belt/envelope-based contact + procedural track) deterministically, decoupled from the game's
//! per-wheel raycast rig (ADR-0005). Mounted by `bin/track_sandbox`, not by `GamePlugin`. See
//! `.agents/docs/design/track-model/HQ.md`.
//!
//! Fully self-contained on purpose: its own code-generated primitive running gear (no glTF, no
//! `TankSpec`) and its own locomotion, so the new belt model can be iterated in isolation and only
//! promoted into the game once it's proven — exactly how `armor_sandbox` grew the penetration march.
//!
//! State: a free-fly camera (WASD/mouse); a deterministic test course (flat lane + two trenches +
//! step + ramp); a code-generated **dynamic** primitive rig carried by **belt contact sampled around
//! the whole loop along each station's outward normal** (down under the tracks, forward on the front
//! face, …), driven by a **belt-speed / slip model** (each track has a belt speed; friction =
//! μ·load·saturate(slip); the front face's drive axis points up, so a spinning belt grinds up walls).
//! Hull + sprocket/idler colliders back it as a hard stop (frictionless — pure penetration stops;
//! the belt owns all tangential physics). Arrow keys drive; contact dots colour green→red by slip.
//! `R` tours the reset spots, `M` cycles the registered locomotion models (the live A/B — see
//! [`Model`]), `L` logs state, `J` prints the jitter probe, `Esc` pauses. The procedural (animated)
//! track lands in a later step.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, CoefficientCombine, Collider, CollisionLayers, Forces,
    Friction, LayerMask, LinearVelocity, Mass, NoAutoAngularInertia, NoAutoCenterOfMass,
    NoAutoMass, Physics, PhysicsInterpolationPlugin, PhysicsPlugins, PhysicsTime,
    ReadRigidBodyForces, RigidBody, SpatialQuery, SpatialQueryFilter, WriteRigidBodyForces,
};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::Layer;

// One file per locomotion model (the `M` A/B): shared course/rig/belt machinery lives here in
// `mod.rs`; each model's force systems live in their own file and are gated on `ActiveModel`.
mod model1;
mod model2;
mod model3;

use model1::apply_belt_support;
use model2::{
    BeltPhase, ChainMemory, apply_belt_support_links, conform_belts_links, init_link_count,
};
use model3::{TRACK_THICKNESS, apply_belt_support_boxes, conform_belts_boxes, init_pin_belt};

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
/// line); the chain solver's wheel-circle constraints then catch the drape and the track *rides the
/// wheels*, hanging in short spans between them (model 2; model 1's point-conform spline has no
/// wheel collision on the top run and just draws the deep parabola).
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
// track-model spec, so it's shared by all models (the A/B holds the vehicle constant). ---
/// Top belt surface speed (m/s) at full command — the governed top speed (T-34: ~53 km/h road).
const MAX_BELT_SPEED: f32 = 15.0;
/// Engine power available to one track (W): V-2 diesel, 373 kW total. The engine delivers a
/// **constant-power curve** — available force = power / belt speed (see [`engine_available`]) — so
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
/// Input ramp (per second): smooths the binary keys into an analog throttle/steer signal.
const DRIVE_RAMP: f32 = 4.0;

// --- Wheels carry NO force in Option 1: the belt is the *sole* ground-contact system (carries the
// tank, tractions, does walls/gaps) — and the belt is also the sole ground *reader*. The visual model
// is belt-primary all the way down: `conform_belts` reads the terrain once per frame (the hull-fixed
// taut loop raised onto the ground), the drawn spline IS that conformed belt, and the road wheels
// RIDE it (`articulate_wheels` — a rigid roller resting on the belt polyline, no raycast of its own).
// One source of truth, one data direction: ground → belt → wheels. Purely visual: the *physics* belt
// penalizes terrain against the rigid reference line, so the drape never nulls the support. Real
// force-bearing per-wheel springs (the opposite dependency direction: ground → wheels → belt) are
// Option 2. ---
/// How fast a wheel's visible placement rises toward a higher target (m/s): terrain *forces* a
/// wheel up, so rising is quick.
const SUSP_RISE_RATE: f32 = 4.0;
/// How fast it falls toward a lower target (m/s): a wheel drops under gravity (plus track weight),
/// so falling is the slower, softer motion — the asymmetry that keeps wheels from snapping down
/// into every dip.
const SUSP_FALL_RATE: f32 = 1.5;
/// Clamp on the cosmetic lift (m): a tall obstacle can't fling the visual wheel arbitrarily far.
const SUSP_MAX_LIFT: f32 = 0.5;

/// The locomotion models the sandbox can run, selectable at runtime with `M`. Competing iterations
/// share the course, rig, camera, input, and belt/conform machinery, and differ only in the gated
/// systems that generate forces / articulate wheels — so they can be A/B'd live on identical terrain,
/// which is the point: the eventual pick is on feel/maintenance/scaling, compared side by side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    /// MODEL 1 — belt-primary (tag `checkpoint/track-model-1`): the belt is the sole ground contact
    /// *and* sole ground reader; wheels are rigid to the hull and ride the conformed belt
    /// cosmetically (ground → belt → wheels).
    BeltPrimary,
    /// MODEL 2 — link-belt: iteration on model 1 where the sampling stations are **virtual track
    /// links** that *travel with the belt* (each carries an arc-phase advanced by belt speed). Real
    /// kinematics fall out: rolling without slip = links stationary on the ground while the hull
    /// passes over (no contact scrubbing); wheelspin/skid = links visibly sliding. Step 1 advects the
    /// stations; segment (plate) contact and link rendering come next.
    LinkBelt,
    /// MODEL 3 — box-belt: model 2 with the actual T-34 shoe (500 × 172 × 40 mm box) as the contact
    /// primitive, hung on the **pin line** (the true pitch line). Wheels ride the inner face, terrain
    /// meets the outer face (oriented box casts), the chain solve rides the pins — three parallel
    /// offsets of one solved curve.
    BoxBelt,
}

impl Model {
    fn label(self) -> &'static str {
        match self {
            Model::BeltPrimary => "1 — belt-primary (belt sole contact, cosmetic wheels)",
            Model::LinkBelt => "2 — link-belt (stations advect with the belt)",
            Model::BoxBelt => "3 — box-belt (pin-line chain, box-cast links)",
        }
    }
}

/// The models registered for the `M` cycle, in order. Adding a model = a `Model` variant, an entry
/// here, and its gated systems.
const MODELS: [Model; 3] = [Model::BeltPrimary, Model::LinkBelt, Model::BoxBelt];

/// Which model's systems are live. Switched by `switch_model`; model-specific systems gate on
/// [`model_is`].
#[derive(Resource)]
struct ActiveModel(Model);

impl Default for ActiveModel {
    fn default() -> Self {
        // Model 3 is the live iteration front; models 1–2 stay registered as frozen baselines.
        Self(Model::BoxBelt)
    }
}

/// Run condition: the given model is active.
fn model_is(model: Model) -> impl Fn(Res<ActiveModel>) -> bool {
    move |active: Res<ActiveModel>| active.0 == model
}

/// The drivetrain force available to spin one track's belt at the given belt speed: a
/// **constant-power** curve (force × speed can't exceed [`ENGINE_POWER`]) under the low-speed
/// torque cap [`ENGINE_FORCE`]. Shared by all models — the drivetrain is vehicle spec, and the A/B
/// comparison holds the vehicle constant.
fn engine_available(belt_speed: f32) -> f32 {
    (ENGINE_POWER / belt_speed.abs().max(0.5)).min(ENGINE_FORCE)
}

/// `M` cycles through the registered models in place (same pose, same course spot) — the live A/B.
/// Belt state is zeroed so the incoming model starts from rest, not from the outgoing model's spin.
fn switch_model(
    keys: Res<ButtonInput<KeyCode>>,
    mut active: ResMut<ActiveModel>,
    mut belt: ResMut<BeltSpeed>,
    mut phase: ResMut<BeltPhase>,
    mut chain: ResMut<ChainMemory>,
) {
    if !keys.just_pressed(KeyCode::KeyM) {
        return;
    }
    let i = MODELS.iter().position(|&m| m == active.0).unwrap_or(0);
    active.0 = MODELS[(i + 1) % MODELS.len()];
    *belt = BeltSpeed::default();
    *phase = BeltPhase::default();
    *chain = ChainMemory::default();
    info!("model → {}", active.0.label());
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

/// A road wheel's cosmetic placement state: the rest pivot in hull-local space and the current eased
/// vertical offset that rides it on the conformed belt. Visual only — no force.
#[derive(Component)]
struct Suspension {
    pivot_local: Vec3,
    dy: f32,
}

/// One station of the conformed belt: its hull-local side-plane position on the rigid reference loop
/// (z, y — *pre*-conform, used to tell the belly from the top run and to align with wheels) and its
/// conformed world position (raised onto terrain).
struct BeltSample {
    local: Vec2,
    world: Vec3,
}

/// Each side's conformed belt this frame — the hull-fixed taut loop resampled fine and raised onto the
/// ground (`y = max(line, terrain)`), in loop order. The **single ground-read of the visual model**:
/// built once per frame by `conform_belts`, then the drawn spline is exactly this and the road wheels
/// ride it (ground → belt → wheels, never the other way).
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
/// jitter, shared by all models since the suspect paths are shared. `J` prints each element's
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
        .init_resource::<BeltContacts>()
        .init_resource::<Paused>()
        .init_resource::<ResetSpot>()
        .init_resource::<DriveInput>()
        .init_resource::<BeltSpeed>()
        .init_resource::<BeltLength>()
        .init_resource::<BeltPhase>()
        .init_resource::<ConformedBelts>()
        .init_resource::<ActiveModel>()
        .init_resource::<JitterProbe>()
        .add_systems(
            Startup,
            (
                spawn_camera,
                grab_cursor,
                spawn_environment,
                spawn_rig,
                init_belt_length,
                init_link_count,
                init_pin_belt,
                spawn_model_label,
            ),
        )
        // Physics runs in the fixed step (before Avian integrates in FixedPostUpdate), NOT while
        // paused (else penalty force accumulates against a frozen sim and flings the rig on resume).
        // Model-specific force systems gate on the active model (the `M` A/B switch).
        //
        // MODEL 1: `apply_belt_support` — single ground-contact system, stations fixed in hull space.
        // MODEL 2: `apply_belt_support_links` — same contact physics, stations advect with the belt.
        // MODEL 3: `apply_belt_support_boxes` — advected links contact as oriented boxes on the pin
        // line (the real shoe: thickness live, width in increment 2).
        .add_systems(
            FixedUpdate,
            (
                apply_belt_support
                    .run_if(sim_running)
                    .run_if(model_is(Model::BeltPrimary)),
                apply_belt_support_links
                    .run_if(sim_running)
                    .run_if(model_is(Model::LinkBelt)),
                apply_belt_support_boxes
                    .run_if(sim_running)
                    .run_if(model_is(Model::BoxBelt)),
            ),
        )
        .add_systems(
            Update,
            (
                fly_camera.run_if(cursor_locked),
                read_drive_input,
                // The visual chain, in data order: read the ground into the conformed belt once
                // (model 1: per-point conform; model 2: rigid-link conform on the advected ring;
                // model 3: the same chain solve on the pin line with box-model offsets), then the
                // wheels ride it cosmetically (all models), then it's drawn. The stateful
                // pieces gate on `sim_running` like the physics — Esc pauses Avian's clock but NOT
                // the Update schedule, so ungated they kept easing wheels / re-solving the chain
                // against a frozen sim ("deforms while paused" — the second clock). The draw systems
                // stay ungated: gizmos are immediate-mode and must redraw the frozen state.
                (
                    conform_belts
                        .run_if(model_is(Model::BeltPrimary))
                        .run_if(sim_running),
                    conform_belts_links
                        .run_if(model_is(Model::LinkBelt))
                        .run_if(sim_running),
                    conform_belts_boxes
                        .run_if(model_is(Model::BoxBelt))
                        .run_if(sim_running),
                    articulate_wheels.run_if(sim_running),
                    // Probe after the visual chain settles this frame's state, frozen while paused
                    // (constant samples would dilute the window).
                    sample_jitter_probe.run_if(sim_running),
                    draw_rig_gizmos,
                )
                    .chain(),
                switch_model,
                update_model_label,
                toggle_pause,
                reset_rig,
                log_state,
                report_jitter_probe,
                draw_contacts,
            ),
        );
}

/// A live belt contact station for visualization: the station in **hull-local** space (so the dot
/// rides the interpolated rig instead of jittering against the last fixed-tick pose), its load, the
/// ground normal it pushes along, and its longitudinal slip speed (m/s — colours the dot green→red).
struct Contact {
    local: Vec3,
    load: f32,
    normal: Vec3,
    slip: f32,
}

/// The belt contact stations found this tick — filled by `apply_belt_support` in the fixed step,
/// drawn by `draw_contacts` per frame. Visualization only.
#[derive(Resource, Default)]
struct BeltContacts(Vec<Contact>);

/// Whether the sim is frozen (`Esc`). The belt model gates on this so it doesn't accumulate force
/// against a paused physics world.
#[derive(Resource, Default)]
struct Paused(bool);

fn sim_running(paused: Res<Paused>) -> bool {
    !paused.0
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
        "pit (swallows the whole rig — drop in, grind out)",
    ),
];

/// Smoothed driver intent in [-1, 1]: throttle (↑/↓) and steer (→/←). Arrow keys, so WASD stays the
/// free-fly camera.
#[derive(Resource, Default)]
struct DriveInput {
    throttle: f32,
    steer: f32,
}

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

/// The fixed total length of one belt (m), computed once at startup from the rest perimeter +
/// `TRACK_SLACK`. Constant thereafter — the whole point of a fixed-length track. Both sides share it
/// (symmetric rig).
#[derive(Resource, Default)]
struct BeltLength(f32);

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

/// The on-screen label of the active model (top-left).
#[derive(Component)]
struct ModelLabel;

fn model_label_text(model: Model) -> String {
    format!("model {}   [M switches]", model.label())
}

fn spawn_model_label(mut commands: Commands, active: Res<ActiveModel>) {
    commands.spawn((
        ModelLabel,
        Text::new(model_label_text(active.0)),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.75, 0.95, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

/// Keep the label current when `M` switches the model.
fn update_model_label(active: Res<ActiveModel>, label: Single<&mut Text, With<ModelLabel>>) {
    if !active.is_changed() {
        return;
    }
    label.into_inner().0 = model_label_text(active.0);
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

    let block = |commands: &mut Commands, transform: Transform, mat: &Handle<StandardMaterial>| {
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
    let ground = |commands: &mut Commands, z_hi: f32, z_lo: f32| {
        block(
            commands,
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
        ground(&mut commands, cursor, near_lip);
        block(
            &mut commands,
            Transform::from_xyz(0.0, TRENCH_FLOOR_Y - 0.5, -tz)
                .with_scale(Vec3::new(LANE_W, 1.0, tw)),
            &ground_mat,
        );
        cursor = far_lip;
    }
    ground(&mut commands, cursor, LANE_FAR);

    // A step / curb (top at y=0.45), past the trenches: a hard vertical edge to climb.
    block(
        &mut commands,
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
                Transform::from_xyz(0.0, height / 2.0, z)
                    .with_scale(Vec3::new(OBSTACLE_W, height, thickness)),
                &obstacle_mat,
            );
        }
    }
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
                    });
                }
            }
        });
}

/// Build each side's **conformed belt** — the one ground-read of the visual model. Take the hull-fixed
/// rigid reference loop (`rest_circles`, the same line the physics penalizes against), resample it
/// fine, and press each station out of the terrain **along its own outward normal** — the same probe
/// the physics uses, so the conform asks the same question the contact does. Terrain penetrating the
/// belt surface moves the station to the terrain surface *in the direction the belt is pressed*: up
/// onto bumps under the belly (even between wheels, as a taut track under tension would), back off a
/// wall face at the nose (never up onto the wall's *top* — the normal ray can't see it, which is what
/// made the old vertical-ray conform snap the belt onto ledges). Terrain outside the surface leaves
/// the station on the taut line, so dips and gaps stay bridged. Everything visual derives from this —
/// the drawn spline is exactly it, and the road wheels ride it.
fn conform_belts(
    hull: Single<&GlobalTransform, With<Hull>>,
    spatial: SpatialQuery,
    belt_length: Res<BeltLength>,
    mut belts: ResMut<ConformedBelts>,
) {
    let hull = *hull;
    let affine = hull.affine();
    // The reference loop is side-agnostic (symmetric rig): resample once, conform per side. Close the
    // loop (append the first point) so the seam has a segment, then use modular indices for tangents.
    let mut loop_pts = belt_loop(&rest_circles(), Some(belt_length.0));
    if let Some(&first) = loop_pts.first() {
        loop_pts.push(first);
    }
    let stations = resample(&loop_pts, BELT_DRAW_SPACING, 0.0);
    let n = stations.len();
    if n < 3 {
        return;
    }
    for side in [Side::Left, Side::Right] {
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        let samples: Vec<BeltSample> = (0..n)
            .map(|i| {
                let p = stations[i];
                let mut w = affine.transform_point3(Vec3::new(track_x, p.y, p.x));
                // Outward normal in the side plane (CCW winding → tangent rotated −90°), as the
                // physics computes it: out2 = (tan.y, −tan.x) in (z, y) → world (x = 0, y = out2.y,
                // z = out2.x).
                let tan2 = (stations[(i + 1) % n] - stations[(i + n - 1) % n]).normalize_or_zero();
                let out = affine
                    .transform_vector3(Vec3::new(0.0, -tan2.x, tan2.y))
                    .normalize_or_zero();
                if let Ok(out_dir) = Dir3::new(out) {
                    // Probe from just inside the belt surface, outward. A hit short of the surface is
                    // terrain penetrating the belt: the hit point IS the conformed station. A zero
                    // distance means the origin itself is buried (extreme clip mid-transient) — the
                    // surface is unknowable from here, so leave the station taut and let the physics
                    // push the rig out.
                    let origin = w - out * CONTACT_PROBE;
                    if let Some(hit) = spatial.cast_ray(
                        origin,
                        out_dir,
                        CONTACT_PROBE,
                        true,
                        &SpatialQueryFilter::from_mask(Layer::Terrain),
                    ) && hit.distance > 0.0
                    {
                        w = origin + out * hit.distance;
                    }
                }
                BeltSample { local: p, world: w }
            })
            .collect();
        match side {
            Side::Left => belts.left = samples,
            Side::Right => belts.right = samples,
        }
    }
}

/// Ride each road wheel on the **conformed belt** — the wheels follow the *track*, not a ground probe
/// of their own (belt-primary: the belt reads the ground once; wheels and spline both derive from it).
/// The wheel is a rigid roller resting on the belt polyline: over a segment with slope `m`, the centre
/// resting on it sits at `y(dz) + √(R²−dz²)`, which peaks at `dz* = mR/√(1+m²)` — solved in closed
/// form per segment (plus the clipped ends), so the wheel's path is smooth as it rolls over bumps and
/// corners instead of quantised to probe columns. Lift-only about the rest pose: a wheel rides up onto
/// a raised belt but never drops below the taut line, so dips and gaps stay bridged. Visual only — the
/// wheels bear no load (the belt is the sole carrier).
fn articulate_wheels(
    hull: Single<&GlobalTransform, With<Hull>>,
    belts: Res<ConformedBelts>,
    active: Res<ActiveModel>,
    mut wheels: Query<(&RigWheel, &mut Suspension, &mut Transform)>,
    time: Res<Time>,
) {
    let affine = hull.affine();
    let dt = time.delta_secs();
    // Model 3's conformed belt is the *pin line*; the wheels rest on the inner face, a
    // half-thickness above it. Models 1–2 conform the belt surface itself.
    let face = match active.0 {
        Model::BoxBelt => TRACK_THICKNESS / 2.0,
        _ => 0.0,
    };
    for (wheel, mut susp, mut transform) in &mut wheels {
        if wheel.kind != WheelKind::Road {
            continue;
        }
        let rest_world = affine.transform_point3(susp.pivot_local);
        let zc = susp.pivot_local.z;
        let mut best = f32::NEG_INFINITY;
        for pair in belts.get(wheel.side).windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            // Only the belly of the loop can support a wheel: skip the top run and the drive-wheel
            // arcs (they sit above the road-wheel hub line in the pre-conform reference).
            if a.local.y > susp.pivot_local.y || b.local.y > susp.pivot_local.y {
                continue;
            }
            // Segment span in wheel-relative dz (hull-local z), clipped to the wheel's width.
            let (z0, z1) = (a.local.x - zc, b.local.x - zc);
            let (lo, hi) = if z0 <= z1 { (z0, z1) } else { (z1, z0) };
            let (lo, hi) = (lo.max(-ROAD_RADIUS), hi.min(ROAD_RADIUS));
            if lo >= hi {
                continue;
            }
            // Conformed world height, linear in dz across the segment.
            let m = (b.world.y - a.world.y) / (z1 - z0);
            let peak = (m * ROAD_RADIUS) / (1.0 + m * m).sqrt();
            for dz in [lo, hi, peak.clamp(lo, hi)] {
                let y = a.world.y + m * (dz - z0);
                let c = y + face + (ROAD_RADIUS * ROAD_RADIUS - dz * dz).max(0.0).sqrt();
                best = best.max(c);
            }
        }
        if best == f32::NEG_INFINITY {
            continue;
        }
        // Ride up onto a raised belt (positive lift), never below the taut line → dips/gaps bridge.
        // Rise fast (terrain forces the wheel up), fall slower (it drops under gravity).
        let target_dy = (best - rest_world.y).clamp(0.0, SUSP_MAX_LIFT);
        let rate = if target_dy > susp.dy {
            SUSP_RISE_RATE
        } else {
            SUSP_FALL_RATE
        };
        susp.dy = approach(susp.dy, target_dy, rate * dt);
        transform.translation.y = susp.pivot_local.y + susp.dy;
    }
}

/// `R` cycles the rig through the reset spots (flat → narrow trench → wide trench → pit), dropping it
/// at rest — the test tour in one key.
fn reset_rig(
    keys: Res<ButtonInput<KeyCode>>,
    hull: Single<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity), With<Hull>>,
    mut spot: ResMut<ResetSpot>,
    mut belt: ResMut<BeltSpeed>,
    mut phase: ResMut<BeltPhase>,
    mut chain: ResMut<ChainMemory>,
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
    *chain = ChainMemory::default();
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
    active: Res<ActiveModel>,
) {
    if !keys.just_pressed(KeyCode::KeyL) {
        return;
    }
    let (transform, lin) = *hull;
    let count = contacts.0.len();
    let total: f32 = contacts.0.iter().map(|c| c.load).sum();
    let weight = HULL_MASS * 9.81;
    let speed = lin.0.dot(transform.forward().into());
    info!(
        "model {} | hull y = {:.3} m | stations = {count} | support = {:.0}% of weight | belt L/R = {:.1}/{:.1} m/s | tank = {:.1} m/s",
        active.0.label(),
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
    let dot = contacts
        .0
        .iter()
        .filter(|c| c.local.x < 0.0)
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

/// Draw the rig skeleton (hub markers) and the **conformed belt** of each side — exactly the loop the
/// wheels ride (`ConformedBelts`, built by `conform_belts` this frame): taut lower run raised onto any
/// terrain it meets, the drive-wheel arcs, and the sagging top run. Pure presentation; also the exact
/// path the procedural track will lay links along later.
fn draw_rig_gizmos(
    mut gizmos: Gizmos,
    wheels: Query<(&RigWheel, &GlobalTransform)>,
    belts: Res<ConformedBelts>,
    active: Res<ActiveModel>,
    hull: Single<&GlobalTransform, With<Hull>>,
) {
    // Hub markers, coloured by role so the drive wheels (sprocket/idler) read apart from the road
    // wheels. `kind` is also the seam for later drive/animation (e.g. torque on the sprocket).
    for (wheel, gt) in &wheels {
        let color = match wheel.kind {
            WheelKind::Road => HUB_COLOR,
            WheelKind::Sprocket | WheelKind::Idler => DRIVE_HUB_COLOR,
        };
        gizmos.sphere(Isometry3d::from_translation(gt.translation()), 0.05, color);
    }

    for side in [Side::Left, Side::Right] {
        let mut world = belts.get(side).iter().map(|s| s.world);
        gizmos.linestrip(world.clone(), BELT_COLOR);
        if let (Some(a), Some(b)) = (world.next_back(), world.next()) {
            gizmos.line(a, b, BELT_COLOR);
        }

        // MODEL 3: the conformed line is the *pin line* — draw the **outer face** (each sample
        // offset by its local outward normal × t/2, from neighbour tangents of the solved chain) as
        // a dimmer companion, so the shoe thickness reads: the dark line rides the ground, the
        // wheels ride the light one.
        if active.0 != Model::BoxBelt {
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
/// interpolated rig; plus a short line along the support normal. Contact distribution, load, push
/// direction, and where the track is slipping all read at a glance.
fn draw_contacts(
    mut gizmos: Gizmos,
    hull: Single<&GlobalTransform, With<Hull>>,
    contacts: Res<BeltContacts>,
) {
    let hull = *hull;
    let k = SUPPORT_STIFFNESS_PER_M * CONTACT_SPACING;
    for c in &contacts.0 {
        let p = hull.transform_point(c.local);
        // load / k ≈ the station's penetration (m) — a stable size cue for the contact.
        let r = 0.03 + (c.load / k).clamp(0.0, 0.1);
        // Slip fraction 0→1 grades green (grip) to red (sliding at μ·load).
        let t = (c.slip.abs() / SLIP_SATURATION).clamp(0.0, 1.0);
        let color = Color::srgb(t, 1.0 - 0.7 * t, 0.2);
        gizmos.sphere(Isometry3d::from_translation(p), r, color);
        gizmos.line(p, p + c.normal * (0.15 + r), NORMAL_COLOR);
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
/// → rear) → rear arc wrapping the idler → top run (rear → front) → front arc wrapping the sprocket.
/// `circles` must be front→rear.
///
/// With `length = Some(L)` the **return (top) run sags**: the fixed belt length L minus everything
/// else is the arc length available for the top run, and its excess over the straight span becomes a
/// parabolic droop (the fixed-length constraint made visible). `None` keeps the top run taut/straight
/// — used by the physics, which only samples the lower + front where the belt meets ground.
fn belt_loop(circles: &[(Vec2, f32)], length: Option<f32>) -> Vec<Vec2> {
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
    match length {
        // The top run gets whatever belt length is left after the rest of the loop; its excess over
        // the straight span sags.
        Some(l) => {
            let non_top = polyline_len(&pts) + polyline_len(&sprocket_arc);
            let top_arc = (l - non_top).max(idler_up.distance(sprocket_up));
            pts.extend(sagging_top(idler_up, sprocket_up, top_arc));
        }
        None => pts.push(sprocket_up),
    }
    pts.extend_from_slice(&sprocket_arc);
    pts
}

/// Total length of a polyline (sum of segment lengths).
fn polyline_len(pts: &[Vec2]) -> f32 {
    pts.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// A top (return) run from `from` to `to` that sags downward to consume `arc_len` of belt. Parabolic
/// droop; depth from the arc-length excess over the straight span (`s ≈ d(1 + 8h²/3d²)`).
fn sagging_top(from: Vec2, to: Vec2, arc_len: f32) -> Vec<Vec2> {
    const SEGMENTS: usize = 12;
    let d = from.distance(to);
    let h = (3.0 * d * (arc_len - d).max(0.0) / 8.0).sqrt();
    (0..=SEGMENTS)
        .map(|i| {
            let t = i as f32 / SEGMENTS as f32;
            let base = from.lerp(to, t);
            Vec2::new(base.x, base.y - 4.0 * h * t * (1.0 - t)) // parabola, max droop at mid-span
        })
        .collect()
}

/// Fix the belt length once at startup: the taut perimeter of the rest pose plus `TRACK_SLACK`.
fn init_belt_length(mut length: ResMut<BeltLength>) {
    length.0 = polyline_len(&belt_loop(&rest_circles(), None)) + TRACK_SLACK;
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

/// Resample a polyline at uniform arc-length `spacing`, stations at arc positions `offset + i·spacing`
/// (evenly spread along the belt, not bunched at the tangent vertices). `offset = 0` starts at the
/// polyline's first point; MODEL 2 passes its advancing belt phase so the stations *travel with the
/// belt*. Standard arc-length walk; degenerate short segments (the tiny hops across a wheel bottom)
/// are skipped.
fn resample(points: &[Vec2], spacing: f32, offset: f32) -> Vec<Vec2> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut out = Vec::new();
    // Arc length remaining until the next station: the first lands at `offset` along the polyline.
    let mut since = spacing - offset.rem_euclid(spacing);
    if since >= spacing {
        out.push(points[0]); // offset 0: a station at the very start, as before
        since = 0.0;
    }
    for w in points.windows(2) {
        let seg = w[1] - w[0];
        let len = seg.length();
        if len < 1e-6 {
            continue;
        }
        let dir = seg / len;
        let mut pos = 0.0;
        loop {
            let step = spacing - since;
            if pos + step > len {
                since += len - pos;
                break;
            }
            pos += step;
            since = 0.0;
            out.push(w[0] + dir * pos);
        }
    }
    out
}

const HUB_COLOR: Color = Color::srgb(1.0, 0.85, 0.2);
const DRIVE_HUB_COLOR: Color = Color::srgb(1.0, 0.45, 0.15);
const BELT_COLOR: Color = Color::srgb(0.2, 0.9, 1.0);
/// Model 3's outer-face companion line: dimmer/darker than the pin line, so the two parallel curves
/// read as inner vs ground face at a glance.
const BELT_OUTER_COLOR: Color = Color::srgb(0.1, 0.45, 0.55);
const NORMAL_COLOR: Color = Color::srgb(1.0, 0.9, 0.2);

/// The two tangent points of an external tangent line shared by two circles in a plane, on the side
/// selected by `side_sign` (−1 = lower / smaller y, +1 = upper). Returns (point on circle 0, point on
/// circle 1). Assumes neither circle contains the other (true for running gear).
fn external_tangent(c0: Vec2, r0: f32, c1: Vec2, r1: f32, side_sign: f32) -> (Vec2, Vec2) {
    let d = c1 - c0;
    let dist = d.length().max(1e-4);
    let dir = d / dist;
    // Unit normal `n` with n·dir = (r0 − r1)/dist; the remaining component is perpendicular. Pick the
    // perpendicular sign so n points to the requested side (its y has `side_sign`).
    let along = ((r0 - r1) / dist).clamp(-1.0, 1.0);
    let perp_mag = (1.0 - along * along).max(0.0).sqrt();
    let perp = Vec2::new(-dir.y, dir.x);
    let perp = if perp.y.signum() == side_sign.signum() {
        perp
    } else {
        -perp
    };
    let n = dir * along + perp * perp_mag;
    (c0 + n * r0, c1 + n * r1)
}

/// Points along a circle's arc from `from` to `to` (both on the circle), taking whichever sweep has
/// its midpoint heading toward `toward` — so the belt wraps the *outer* side (the front of the
/// sprocket, the rear of the idler) rather than cutting across. Endpoints included.
fn arc(center: Vec2, radius: f32, from: Vec2, to: Vec2, toward: Vec2) -> Vec<Vec2> {
    const SEGMENTS: usize = 10;
    use std::f32::consts::{PI, TAU};
    let a0 = (from - center).to_angle();
    let mut delta = (to - center).to_angle() - a0;
    // Reduce to the shortest signed sweep, then flip to the complement if it faces away from `toward`.
    while delta <= -PI {
        delta += TAU;
    }
    while delta > PI {
        delta -= TAU;
    }
    if Vec2::from_angle(a0 + delta * 0.5).dot(toward) < 0.0 {
        delta -= delta.signum() * TAU;
    }
    (0..=SEGMENTS)
        .map(|i| center + Vec2::from_angle(a0 + delta * (i as f32 / SEGMENTS as f32)) * radius)
        .collect()
}

/// Read the driver's arrow-key intent into a smoothed throttle/steer signal. Zeroed while the cursor
/// is free (paused / unfocused) so a released window doesn't keep driving.
fn read_drive_input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    cursors: Query<&CursorOptions>,
    mut input: ResMut<DriveInput>,
) {
    let locked = cursors
        .single()
        .map(|c| c.grab_mode == CursorGrabMode::Locked)
        .unwrap_or(false);
    let axis = |pos, neg| keys.pressed(pos) as i8 as f32 - keys.pressed(neg) as i8 as f32;
    let (target_throttle, target_steer) = if locked {
        (
            axis(KeyCode::ArrowUp, KeyCode::ArrowDown),
            axis(KeyCode::ArrowRight, KeyCode::ArrowLeft),
        )
    } else {
        (0.0, 0.0)
    };
    let step = DRIVE_RAMP * time.delta_secs();
    input.throttle = approach(input.throttle, target_throttle, step);
    input.steer = approach(input.steer, target_steer, step);
}

/// Move `current` toward `target` by at most `step`.
fn approach(current: f32, target: f32, step: f32) -> f32 {
    if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
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
