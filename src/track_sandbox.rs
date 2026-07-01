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
//! Hull + sprocket/idler colliders back it as a hard stop. Arrow keys drive; contact dots colour
//! green→red by slip. `R` tours the reset spots, `L` logs state, `Esc` pauses. Bump-stops and the
//! procedural (animated) track land in later steps.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, Collider, CollisionLayers, Forces, LayerMask, LinearVelocity,
    Mass, NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, Physics,
    PhysicsInterpolationPlugin, PhysicsPlugins, PhysicsTime, ReadRigidBodyForces, RigidBody,
    SpatialQuery, SpatialQueryFilter, WriteRigidBodyForces,
};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::Layer;

// --- Rig geometry (metres). A generic, tank-ish primitive; every value here is a knob the model is
// meant to be tested against (wheel count, spacing, track length, overhangs). ---

/// Number of road wheels per side.
const ROAD_WHEELS: usize = 5;
/// Road-wheel radius. Also the effective belt half-thickness at the hub for now.
const ROAD_RADIUS: f32 = 0.35;
/// Hub-to-hub spacing of road wheels along the track (forward axis).
const WHEEL_SPACING: f32 = 1.15;
/// Sprocket/idler radius — larger than the road wheels, in the usual way.
const DRIVE_RADIUS: f32 = 0.45;
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
/// Hull box half-extents (x = half width, y = half height, z = half length).
const HULL_HALF: Vec3 = Vec3::new(1.05, 0.55, 3.4);
/// Hull centre height when resting on flat ground (road-wheel hubs sit at y = ROAD_RADIUS).
const HULL_REST_Y: f32 = 1.15;
/// Hull mass (kg) for the primitive — modest, to keep the contact-spring forces sane while still
/// feeling weighty. Not a real tank's 57 t; this rig exists to test the model, not a variant.
const HULL_MASS: f32 = 12_000.0;

// --- Test course (module-level so the reset + trench floors can reference the trenches) ---
/// Two trenches down the −Z lane, each `(centre z, width)`. Narrow: some road wheels still catch the
/// lips. Wide (> the road-wheel span): all road wheels float and only the sprocket/idler diagonal
/// runs catch — the pure trench-bridging case. Ordered nearest→farthest.
const TRENCHES: [(f32, f32); 2] = [(8.0, 2.2), (18.0, 5.0)];
/// Lane extent (Z) of the ground: from `LANE_NEAR` in front of spawn out to `LANE_FAR`.
const LANE_NEAR: f32 = 20.0;
const LANE_FAR: f32 = -60.0;
/// Lane width (X) of the ground slabs and obstacles.
const LANE_W: f32 = 14.0;
/// Top of the trench floors: a hard bottom below belt reach, so a *failed* bridge rests the rig in
/// the ditch instead of dropping into a bottomless gap.
const TRENCH_FLOOR_Y: f32 = -1.2;

// --- Belt contact model ---
/// Arc-length spacing of belt contact stations along the lower run (m). Denser = smoother contact
/// (less bump as a station crosses a ledge), more rays. Because the coefficients below are **per
/// metre of belt**, changing this changes only smoothness — never the total support/traction — so
/// resolution and the physics are decoupled (the fix for "finer spacing launched the rig").
const CONTACT_SPACING: f32 = 0.15;
/// Downward ray length used to find ground just beneath each station (m); also the sink at which
/// support saturates.
const CONTACT_PROBE: f32 = 0.5;
/// Slack (m) in the belt beyond the taut rest perimeter: the fixed track length is `rest perimeter +
/// this`. As the wheels articulate, the taut perimeter changes and the leftover slack redistributes
/// onto the return (top) run as sag. Sag depth grows as ~√slack, so a little goes a long way — tune.
const TRACK_SLACK: f32 = 0.02;
/// Contact-spring stiffness per **metre of belt** (N/m per m): as the *sole* carrier now (Option 1),
/// the grounded belt length holds ~mg at ~5 cm of sink — soft enough for a compliant, well-engaged
/// ride (deep stations don't flicker) rather than the old stiff 2 cm bed that see-sawed. Multiplied by
/// `CONTACT_SPACING` for the per-station value. Ride frequency ≈ √(g / sink) is mass-independent, so
/// this generalizes: pick a target sink, not a per-vehicle spring constant.
const SUPPORT_STIFFNESS_PER_M: f32 = 250_000.0;
/// Contact-spring damping per **metre of belt** (N·s/m per m): ~0.85 critical for the vertical mode at
/// the softened stiffness above (over-damping here just makes it sluggish).
const SUPPORT_DAMPING_PER_M: f32 = 30_000.0;
/// Soft-engagement depth (m): a station ramps its contact force in over the first this-many metres of
/// penetration (quadratic near zero) instead of switching full force on the instant it crosses the
/// belt surface. Kills the on/off flicker at the belt ends that see-saws the rigid rig at rest — the
/// principled fix, since a real track is compliant, not a hard edge. Well below the ~5 cm static sink,
/// so it doesn't change the resting height, only the behaviour right at the contact boundary.
const CONTACT_ENGAGE: f32 = 0.02;

// --- Drive: belt-speed / slip model. Each track has a belt *speed*; friction comes from the slip
// between belt and ground, so wheelspin, skid, engine-braking, hill-hold, and top speed all emerge. ---
/// Top belt surface speed (m/s) at full command — the governed top speed.
const MAX_BELT_SPEED: f32 = 11.0;
/// Max force the engine can put into spinning one track's belt (N). If it exceeds the available grip
/// the belt over-spins the ground → wheelspin; on grippy ground the belt and ground find rolling.
const ENGINE_FORCE: f32 = 90_000.0;
/// Governor gain (N per m/s of belt-speed error): how hard the engine chases the commanded belt
/// speed, clamped to `ENGINE_FORCE`. Also gives engine-braking when the command drops.
const BELT_GOVERNOR_GAIN: f32 = 60_000.0;
/// Effective linear inertia of one track's belt (kg): how quickly it spins up / down. Smaller = more
/// responsive and more prone to wheelspin.
const BELT_INERTIA: f32 = 3_000.0;
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
// tank, tractions, does walls/gaps). But the wheels are placed **cosmetically on the draped belt** so
// the track visibly wraps the terrain — each road wheel rides up onto the highest ground its radius can
// reach, never dropping below the taut belt line (so dips are bridged, not fallen into). This is purely
// visual (`articulate_wheels`): the *physics* belt stays on the hull-fixed rigid line, decoupled, so
// the drape never nulls the support. Real force-bearing per-wheel springs are the Option-2 step. ---
/// Number of down-probes across a wheel's contact width (±`ROAD_RADIUS`) for the cosmetic placement — a
/// discretised cylinder cast, so a wheel rests on the highest terrain its surface can touch (and bridges
/// dips narrower than itself) instead of a thin ray poking through to a valley floor.
const FOOTPRINT_SAMPLES: usize = 7;
/// How far the cosmetic probe reaches for ground (m).
const SUSP_RAY_LENGTH: f32 = 1.5;
/// How fast a wheel's visible placement eases toward its target (m/s), so it travels rather than snaps.
const SUSP_TRAVEL_RATE: f32 = 2.5;
/// Clamp on the cosmetic lift (m): a tall obstacle can't fling the visual wheel arbitrarily far.
const SUSP_MAX_LIFT: f32 = 0.5;

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

/// A single wheel of the code-generated rig: its side, role, and radius. Spawned as a child of the
/// hull, so its `GlobalTransform` follows the hull (and, later, its own suspension travel).
#[derive(Component)]
struct RigWheel {
    side: Side,
    kind: WheelKind,
    radius: f32,
}

/// A road wheel's cosmetic placement state: the rest pivot in hull-local space (fixed probe source)
/// and the current eased vertical offset that rides it onto the draped belt. Visual only — no force.
#[derive(Component)]
struct Suspension {
    pivot_local: Vec3,
    dy: f32,
}

/// Marker for the hull body (the single dynamic rigid body, static for now in increment 1).
#[derive(Component)]
struct Hull;

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
        .add_systems(
            Startup,
            (
                spawn_camera,
                grab_cursor,
                spawn_environment,
                spawn_rig,
                init_belt_length,
            ),
        )
        // The belt contact model runs in the fixed step (before Avian integrates in FixedPostUpdate),
        // NOT while paused (else its penalty force accumulates against a frozen sim and flings the rig
        // on resume). It is the single ground-contact system: it carries the hull, provides traction,
        // and integrates belt speed — the wheels are rigid to the hull (Option 1).
        .add_systems(FixedUpdate, apply_belt_support.run_if(sim_running))
        .add_systems(
            Update,
            (
                fly_camera.run_if(cursor_locked),
                read_drive_input,
                articulate_wheels,
                toggle_pause,
                reset_rig,
                log_state,
                draw_rig_gizmos,
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
const RESET_SPOTS: [(f32, &str); 3] = [
    (0.0, "flat ground"),
    (-TRENCHES[0].0, "narrow trench"),
    (-TRENCHES[1].0, "wide trench (pure diagonal bridge)"),
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

    // A step / curb (top at y=0.45): a hard vertical edge to climb.
    block(
        &mut commands,
        Transform::from_xyz(0.0, 0.225, -28.0).with_scale(Vec3::new(LANE_W, 0.45, 4.0)),
        &obstacle_mat,
    );

    // A 20° ramp beyond the step (flush entry, crest with a drop) to check climb + envelope over a
    // slope. Low-edge top sunk ~1 m under the ground plane so the approach is step-free.
    let (run, thick, deg) = (10.0_f32, 2.0_f32, 20.0_f32);
    let (sin, cos) = deg.to_radians().sin_cos();
    let center_y = -1.0 - (thick / 2.0) * cos + (run / 2.0) * sin;
    block(
        &mut commands,
        Transform::from_xyz(0.0, center_y, -40.0)
            .with_rotation(Quat::from_rotation_x(deg.to_radians()))
            .with_scale(Vec3::new(LANE_W, thick, run)),
        &obstacle_mat,
    );

    // A washboard just in front of spawn: bumps spaced wider than a wheel (period 1.5 m, gap 1.0 m >
    // the 0.7 m wheel) and taller, so each wheel visibly drops between and rides over — the wheels can
    // resolve these, unlike a fine ripple they'd just bridge. The clearest "suspension is working" demo.
    for i in 0..4 {
        let z = -3.0 - i as f32 * 1.5;
        block(
            &mut commands,
            Transform::from_xyz(0.0, 0.09, z).with_scale(Vec3::new(LANE_W, 0.18, 0.5)),
            &obstacle_mat,
        );
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
                    RigWheel { side, kind, radius },
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

/// Cosmetically ride each road wheel up onto the terrain it can reach, so the drawn belt (which wraps
/// the wheels) visibly conforms to the ground instead of cutting a rigid plate across it. The wheels
/// bear no load — the belt is the sole carrier — so this is pure visuals; the physics belt stays on the
/// hull-fixed rigid line (see `apply_belt_support`), decoupled, so this drape never nulls the support.
/// A wheel rides up onto the highest terrain its radius can touch (a discretised cylinder cast) but
/// never drops below the taut belt line, so dips narrower than the wheel — and gaps — are bridged.
fn articulate_wheels(
    hull: Single<&GlobalTransform, With<Hull>>,
    mut wheels: Query<(&RigWheel, &mut Suspension, &mut Transform)>,
    spatial: SpatialQuery,
    time: Res<Time>,
) {
    let affine = hull.affine();
    let travel = SUSP_TRAVEL_RATE * time.delta_secs();
    for (wheel, mut susp, mut transform) in &mut wheels {
        if wheel.kind != WheelKind::Road {
            continue;
        }
        let rest_world = affine.transform_point3(susp.pivot_local);
        // The wheel-centre world height needed to rest on the highest terrain its surface can touch
        // across ±ROAD_RADIUS: for column `dz`, a ground hit at `terrain_y` supports the centre at
        // `terrain_y + sqrt(R² − dz²)`; take the max (highest terrain the rigid roller first meets).
        let mut best_center_y = f32::NEG_INFINITY;
        for s in 0..FOOTPRINT_SAMPLES {
            let dz = -ROAD_RADIUS + 2.0 * ROAD_RADIUS * (s as f32 / (FOOTPRINT_SAMPLES - 1) as f32);
            let origin = affine.transform_point3(susp.pivot_local + Vec3::new(0.0, 0.0, dz));
            let Some(hit) = spatial.cast_ray(
                origin,
                Dir3::NEG_Y,
                SUSP_RAY_LENGTH,
                true,
                &SpatialQueryFilter::from_mask(Layer::Terrain),
            ) else {
                continue;
            };
            let terrain_y = origin.y - hit.distance;
            let center_y = terrain_y + (ROAD_RADIUS * ROAD_RADIUS - dz * dz).max(0.0).sqrt();
            best_center_y = best_center_y.max(center_y);
        }
        // Ride up onto terrain (positive lift), but never below the taut rest line → dips/gaps bridge.
        let target_dy = (best_center_y - rest_world.y).clamp(0.0, SUSP_MAX_LIFT);
        susp.dy = approach(susp.dy, target_dy, travel);
        transform.translation.y = susp.pivot_local.y + susp.dy;
    }
}

/// Belt contact — the core of the model. Sample the **whole** belt loop (not just the lower run) and,
/// at each station, probe along the belt's **outward normal** (down under the tracks, forward on the
/// front face, etc.). Wherever the belt meets terrain: (1) push back with a damped penalty spring
/// along the contact normal (**support**); (2) apply **slip-based friction** — `μ·load ×
/// saturate(slip / SLIP_SATURATION)` — where the belt's longitudinal drive axis is the belt-travel
/// direction (down the front face, so friction reacts *up* → grinding-climb), capped on the friction
/// ellipse (**traction**). The longitudinal friction reacts back on the belt, which the engine
/// governor drives, so wheelspin/skid/engine-braking/hill-hold emerge. One mechanism covers ground,
/// walls, ledges, and ditch faces alike.
fn apply_belt_support(
    mut hull: Query<(&GlobalTransform, Forces), With<Hull>>,
    spatial: SpatialQuery,
    input: Res<DriveInput>,
    time: Res<Time>,
    mut belt: ResMut<BeltSpeed>,
    mut contacts: ResMut<BeltContacts>,
) {
    let Ok((hull_gt, mut forces)) = hull.single_mut() else {
        return;
    };
    let affine = hull_gt.affine();
    contacts.0.clear(); // the sole contact system now — nothing ran before us this tick
    let dt = time.delta_secs();

    // Per-station support coefficients = per-metre × the arc-length each station represents, so the
    // totals are independent of `CONTACT_SPACING` (resolution decoupled from the physics).
    let k = SUPPORT_STIFFNESS_PER_M * CONTACT_SPACING;
    let c = SUPPORT_DAMPING_PER_M * CONTACT_SPACING;

    for side in [Side::Left, Side::Right] {
        // Physics belt = the hull-fixed rigid taut line (`rest_circles`), NOT the cosmetically-draped
        // wheels — otherwise draping the wheels onto terrain would flatten the line onto the ground and
        // null the penetration that carries the tank. Terrain rising above this rigid line generates
        // support; terrain dropping below it is bridged straight.
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        let circles = rest_circles();
        // Additive differential: steer adds to the left track, subtracts from the right, so a pure
        // steer pivots in place and a steer biases the turn the same way at any throttle.
        let command = match side {
            Side::Left => input.throttle + input.steer,
            Side::Right => input.throttle - input.steer,
        }
        .clamp(-1.0, 1.0);
        let belt_speed = belt.get(side); // this tick's belt surface speed (constant over the loop)
        // Sum the longitudinal ground friction across this side's belt stations so the belt-speed
        // integrator sees the full ground reaction (traction is all on the belt now).
        let mut belt_reaction = 0.0;

        // The full closed belt loop, resampled at uniform spacing. Close it (append the first point)
        // so the seam has a segment, then use modular indices for the tangent.
        let mut loop_pts = belt_loop(&circles, None);
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let stations = resample(&loop_pts, CONTACT_SPACING);
        let n = stations.len();
        if n < 3 {
            continue;
        }

        for i in 0..n {
            let point = stations[i];
            // Belt tangent (loop-traversal direction) and outward normal, both in the side plane.
            // Winding is CCW in (z, y), so the outward normal is the tangent rotated −90°.
            let tan2 = (stations[(i + 1) % n] - stations[(i + n - 1) % n]).normalize_or_zero();
            if tan2 == Vec2::ZERO {
                continue;
            }
            let out2 = Vec2::new(tan2.y, -tan2.x);

            let p = affine.transform_point3(Vec3::new(track_x, point.y, point.x));
            // Side-plane (z, y) direction → world: local (x = 0, y = v.y, z = v.x).
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let Ok(out_dir) = Dir3::new(out) else {
                continue;
            };

            // Probe from just inside the belt surface, outward, for terrain the belt has met.
            let origin = p - out * CONTACT_PROBE;
            let Some(hit) = spatial.cast_ray(
                origin,
                out_dir,
                CONTACT_PROBE + 0.02,
                true,
                &SpatialQueryFilter::from_mask(Layer::Terrain),
            ) else {
                continue;
            };
            // Penetration of terrain past the belt surface. No deadband: the belt is the sole carrier
            // now, so on flat ground it settles at a small continuous sink (no parallel wheel springs
            // holding it at the surface to buzz against), and every grounded station carries its share.
            let pen = CONTACT_PROBE - hit.distance;
            if pen <= 0.0 {
                continue;
            }

            // (1) Support: penalty spring along the **belt's own inward normal** (−outward), NOT the
            // terrain hit-normal. The belt normal is smooth (from the spline), whereas the terrain
            // normal flips between "up" and "sideways" when a ray lands on an edge (a ditch lip),
            // which shoved the rig in alternating directions and made it chatter/wedge. `−out` still
            // pushes off a wall (outward points into it) and up off the ground; only the direction is
            // stabilised. Damped by the hull's speed along it.
            let normal = -out;
            let vel = forces.velocity_at_point(p);
            // Soft engagement: ramp the whole contact force in over the first CONTACT_ENGAGE metres of
            // penetration, so a station crossing the belt surface eases its force from zero instead of
            // snapping a large force on/off (which see-sawed the rigid rig at rest). Full force once
            // well engaged (the resting flat run sits far past this).
            let engage = (pen / CONTACT_ENGAGE).clamp(0.0, 1.0);
            let load = (k * pen - c * vel.dot(normal)).max(0.0) * engage;
            if load <= 0.0 {
                continue;
            }
            forces.apply_force_at_point(normal * load, p);

            // (2) Traction. The belt's drive axis is the belt-travel direction (−tangent: belt_speed
            // > 0 lays ground backward), projected into the contact plane; lateral is across it. Slip
            // is belt speed minus the ground's speed along the drive axis; friction saturates at
            // μ·load. On the front face the drive axis points *up*, so a spinning belt climbs.
            let mut slip_long = 0.0;
            let drive = -affine.transform_vector3(Vec3::new(0.0, tan2.y, tan2.x));
            let long_plane = drive - drive.dot(normal) * normal;
            if long_plane.length() > 1e-4 {
                let long_dir = long_plane.normalize();
                let lat_dir = normal.cross(long_dir).normalize_or_zero();
                slip_long = belt_speed - vel.dot(long_dir);
                let s_lat = vel.dot(lat_dir);
                let grip = MU * load;
                let grip_lat = grip * LATERAL_GRIP_RATIO;
                let mut f_long = grip * (slip_long / SLIP_SATURATION).clamp(-1.0, 1.0);
                let mut f_lat = -grip_lat * (s_lat / SLIP_SATURATION).clamp(-1.0, 1.0);
                let e = (f_long / grip).powi(2) + (f_lat / grip_lat).powi(2);
                if e > 1.0 {
                    let s = e.sqrt().recip();
                    f_long *= s;
                    f_lat *= s;
                }
                forces.apply_force_at_point(long_dir * f_long + lat_dir * f_lat, p);
                belt_reaction += f_long; // the belt feels the longitudinal friction as a load
            }

            contacts.0.push(Contact {
                local: Vec3::new(track_x, point.y, point.x),
                load,
                normal,
                slip: slip_long,
            });
        }

        // Belt dynamics: the engine governor chases the commanded belt speed with force limited to
        // ENGINE_FORCE; the ground friction reaction opposes it. When the engine out-muscles the
        // available grip the belt over-spins the ground → wheelspin; otherwise they find rolling.
        let target = command * MAX_BELT_SPEED;
        let engine =
            (BELT_GOVERNOR_GAIN * (target - belt_speed)).clamp(-ENGINE_FORCE, ENGINE_FORCE);
        let next = belt_speed + (engine - belt_reaction) / BELT_INERTIA * dt;
        belt.set(side, next.clamp(-MAX_BELT_SPEED, MAX_BELT_SPEED));
    }
}

/// `R` cycles the rig through the reset spots (flat → narrow trench → wide trench), dropping it at
/// rest — the test tour in one key.
fn reset_rig(
    keys: Res<ButtonInput<KeyCode>>,
    hull: Single<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity), With<Hull>>,
    mut spot: ResMut<ResetSpot>,
    mut belt: ResMut<BeltSpeed>,
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
    let count = contacts.0.len();
    let total: f32 = contacts.0.iter().map(|c| c.load).sum();
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

/// Draw the rig skeleton (hub markers) and the full **belt envelope** per side: the taut lower run,
/// the rear arc wrapping the idler, the top run, and the front arc wrapping the sprocket — a closed
/// loop that hugs every wheel (so it coincides with the sprocket/idler colliders, and is the exact
/// path the procedural track will lay links along later).
fn draw_rig_gizmos(
    mut gizmos: Gizmos,
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &GlobalTransform)>,
    belt_length: Res<BeltLength>,
) {
    let hull = *hull;
    let to_local = hull.affine().inverse();

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
        let Some((track_x, circles)) = side_circles(&wheels, &to_local, side) else {
            continue;
        };
        let world: Vec<Vec3> = belt_loop(&circles, Some(belt_length.0))
            .iter()
            .map(|p| hull.transform_point(Vec3::new(track_x, p.y, p.x)))
            .collect();
        gizmos.linestrip(world.iter().copied(), BELT_COLOR);
        if let (Some(&a), Some(&b)) = (world.last(), world.first()) {
            gizmos.line(a, b, BELT_COLOR);
        }
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

/// This side's wheels as side-plane circles in hull-local space, ordered front→rear (z ascending).
/// Returns the track's local x (all one side's wheels share it) and the `((z, y), radius)` circles.
fn side_circles(
    wheels: &Query<(&RigWheel, &GlobalTransform)>,
    to_local: &Affine3A,
    side: Side,
) -> Option<(f32, Vec<(Vec2, f32)>)> {
    let mut track_x = 0.0;
    let mut circles: Vec<(Vec2, f32)> = Vec::new();
    for (wheel, gt) in wheels {
        if wheel.side != side {
            continue;
        }
        let local = to_local.transform_point3(gt.translation());
        track_x = local.x;
        circles.push((Vec2::new(local.z, local.y), wheel.radius));
    }
    if circles.is_empty() {
        return None;
    }
    circles.sort_by(|a, b| a.0.x.partial_cmp(&b.0.x).unwrap());
    Some((track_x, circles))
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

/// Resample a polyline at uniform arc-length `spacing`, so contact stations are evenly spread along
/// the belt (not bunched at the tangent vertices). Standard arc-length walk; degenerate short
/// segments (the tiny hops across a wheel bottom) are skipped.
fn resample(points: &[Vec2], spacing: f32) -> Vec<Vec2> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    let mut since = 0.0; // arc length accumulated since the last emitted station
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
