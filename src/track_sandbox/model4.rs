//! Field-belt model: an advected pin-line chain with terrain contact read from a **deterministic
//! analytic field** instead of narrow-phase queries — the promoted model the sandbox hosts.
//!
//! The terrain oracle is a rounded-box SDF union over the course's authored blocks
//! ([`TerrainField`], filled by `spawn_environment`). Per link, penetration is evaluated at
//! **fixed link-local collocation stations** (the two pins + the midpoint, on the outer face) and
//! fed to a closed-form pressure profile. There is no witness point, no tie-breaking, and no
//! collision engine anywhere in the loop: depth is a pure fixed-order arithmetic function of
//! pose — pose-continuous (C0) and bit-deterministic by construction (the contact-oracle
//! research verdict; see `.agents/docs/design/track-model/contact-oracle-research.md`).
//!
//! The field is **rounded** ([`FIELD_ROUNDING`]): box edges in the SDF turn instead of snapping,
//! so normals and depths stay smooth as links cross bump corners — the "round the field, not the
//! mesh" hardening (Drake margin / Jolt active-edge lesson), and the cure for the washboard
//! slap-down.
//!
//! **Width** ([`TRACK_WIDTH`]) enters as three lateral **columns** (the true shoe edges at
//! ±[`COLUMN_OFFSET`] + the centerline, Simpson-weighted — see [`COLUMNS`]): each column samples
//! its own three stations, owns its share of the per-metre coefficients, and applies its
//! resultant at its own point — curb-under-one-edge roll torque, cross-slope contact, and
//! half-off-a-ledge support emerge from the application points.
//!
//! The sandbox's DEFAULT track view is a stateless kinematic wrap (step 22): the road wheels read
//! the field directly ([`articulate_wheels_field`]), the belt path is *fitted* around the
//! articulated wheels every frame ([`conform_belts_field`]) — tangent wrap + terrain conform +
//! budgeted sag — and nothing about the drawn track is simulated or remembered. The step-24 route
//! chain rides behind the `V` toggle as its live A/B partner ([`conform_belts_field_chain`], the
//! same [`track::chain`](crate::track::chain) core the game runs) — and it is the view that WON and
//! SHIPPED as the game's own (`track::view`). Neither is awaiting deletion.

use super::*;

use crate::track::chain::{ChainInput, ChainParams, ChainSideInput, ChainState};
use crate::track::forces::{
    ForceParams, SideInput, SideReport, SideState, contact_side, phase_decompose,
};
use crate::track::oracle::{BlockField, TerrainOracle};
use crate::track::transmission::{
    self, TransmissionAuthoring, TransmissionInput, TransmissionParams,
};
use crate::track::wheels::{WheelParams, wheel_lift_step, wheel_lift_target};

/// The sandbox's terrain resource: the track core's [`BlockField`] behind the sandbox's fixed
/// probe reach ([`CONTACT_PROBE`]), so every call site keeps the historical two-argument shape.
#[derive(Resource, Default)]
pub(super) struct TerrainField(pub(super) BlockField);

impl TerrainField {
    pub(super) fn depth_along(&self, station: Vec3, out: Vec3) -> f32 {
        self.0.depth_along(station, out, CONTACT_PROBE)
    }
}

/// Link (shoe) thickness (m): the T-34's cast shoe is ~40 mm between the ground face and the
/// wheel path. Half of it is the offset between the pin line and either face.
pub(super) const TRACK_THICKNESS: f32 = 0.04;

/// Each side's **total** belt travel (m) along the reference loop — the core's advected state
/// ([`SideState::phase`]), committed back verbatim each tick. `f64` exactly like the shared
/// core (and the game's `TrackDrive`): unbounded travel accumulates past f32 ULP within
/// minutes. Kept unwrapped: consumers wrap mod the link pitch for the sampling offset, and its
/// quotient is the **link-identity shift** the chain warm-start needs.
#[derive(Resource, Default)]
pub(super) struct BeltPhase(PerSide<f64>);

impl BeltPhase {
    pub(super) fn get(&self, side: Side) -> f64 {
        *self.0.get(side)
    }
    fn set(&mut self, side: Side, phase: f64) {
        *self.0.get_mut(side) = phase;
    }
}

/// The belt lives on the **pin line** — `rest_circles` inflated by t/2 — whose perimeter is
/// ~π·t longer than the belt-line loop, so the pin belt owns its own length and link count.
#[derive(Resource, Default)]
pub(super) struct PinBelt {
    pub(super) length: f32,
    pub(super) count: usize,
}

pub(super) fn init_pin_belt(mut commands: Commands) {
    let length = polyline_len(&belt_loop(&pin_circles())) + TRACK_SLACK;
    commands.insert_resource(PinBelt {
        length,
        count: (length / CONTACT_SPACING).round() as usize,
    });
}

/// The rest-pose wheel circles inflated to the pin line (radius + t/2): the wheels touch the
/// inner face, so the pins run a half-thickness outside every wheel surface.
pub(super) fn pin_circles() -> Vec<(Vec2, f32)> {
    rest_circles()
        .iter()
        .map(|&(c, r)| (c, r + TRACK_THICKNESS / 2.0))
        .collect()
}

/// Shoe (link) width (m): the T-34's 500 mm plate.
const TRACK_WIDTH: f32 = 0.5;

/// Lateral offset of the edge columns from the track centerline: the TRUE shoe edges (±w/2).
/// These are point samples — there is no lateral query radius — so anything short of ±0.25
/// leaves a blind rim at the shoe edge (the step-20 value (w − t)/2 = ±0.23 borrowed the pill's
/// cast radius as reach the field oracle doesn't have; codex finding, step 21c).
const COLUMN_OFFSET: f32 = TRACK_WIDTH / 2.0;

/// Edge-column weight, solved so the edge pair reproduces a laterally-UNIFORM pressure strip's
/// second moment exactly: `2·w_e·off² = w²/12` → with the columns at ±w/2 this is exactly 1/6
/// (Simpson weights, fittingly). Three columns give: exact total load, exact uniform-strip roll
/// stiffness, detection to the true shoe edges, and a mid-track detection row — the lateral
/// sampling gap is 0.25 m.
const EDGE_WEIGHT: f32 = (TRACK_WIDTH * TRACK_WIDTH / 12.0) / (2.0 * COLUMN_OFFSET * COLUMN_OFFSET);

/// The lateral columns: (offset along the link's lateral axis, share of the per-metre
/// coefficients). Weights sum to 1 — flat-ground totals are exactly the single-column model's.
const COLUMNS: [(f32, f32); 3] = [
    (-COLUMN_OFFSET, EDGE_WEIGHT),
    (0.0, 1.0 - 2.0 * EDGE_WEIGHT),
    (COLUMN_OFFSET, EDGE_WEIGHT),
];

// Route-chain solve knobs (step 23, from the codex chain deep dive — every knob has physical
// units; per-frame damping factors, per-frame pass counts, and stiffness-by-iteration are gone).
/// Fixed internal solve step (s): the chain advances on its OWN clock via a frame-time
/// accumulator, so feel is identical at 30/60/144 fps (the old 0.88-per-frame damping + 20
/// passes-per-frame was "three different chains" across render rates).
const CHAIN_SUBSTEP: f32 = 1.0 / 120.0;
/// Catch-up budget: at most this many substeps per rendered frame; longer hitches drop debt
/// instead of integrating a monster step.
const CHAIN_MAX_SUBSTEPS: usize = 8;
/// Constraint sweeps per substep (many small steps beat many sweeps in one big step — XPBD
/// "small steps" result).
const CHAIN_SWEEPS: usize = 4;
/// Damping as real-time half-lives (s), ANISOTROPIC in the route frame (step 24, codex T-34
/// review): isotropic drag is rope physics — it kills the longitudinal yank along with the
/// flutter. Tangential motion (yank, slack migration) barely decays; route-normal motion
/// (transverse flutter) dies fast. The other half of transverse deadness is the pin friction.
const CHAIN_HALF_LIFE_TAN: f32 = 0.60;
const CHAIN_HALF_LIFE_NORM: f32 = 0.060;
/// Node mass (kg): one T-34 link assembly — ~16 kg cast shoe + its share of pin hardware
/// (~1.15 t per 72-link track). Enters the XPBD denominators (w = 1/m), which makes the bending
/// compliance and the friction torque REAL units instead of normalized view parameters.
const CHAIN_NODE_MASS: f32 = 16.0;
/// Pin dry-friction torque (N·m): μ≈0.15 on a ~12 mm pin under 10–50 kN tension gives ~18–90;
/// 25 is the unloaded starting point. Implemented as a torque-LIMITED XPBD hinge constraint
/// toward the joint's previous material angle, multiplier accumulated across sweeps and clamped
/// once per substep (|λ| ≤ τ·h²). This is the physical rope-vs-track differentiator: real track
/// pins are heavily-loaded dry steel bearings — flutter dies within a link or two and slack
/// settles near-polygonal, while bulk yank passes through because it doesn't articulate joints.
const CHAIN_HINGE_TORQUE: f32 = 25.0;
/// Belt length trimmed off the chain view's loop (m) — the tensioner PRELOAD. The T-34 manual
/// spec is ~30–50 mm of return-run sag between wheel tops when correctly tensioned; of the
/// authored 0.13 m TRACK_SLACK this trim leaves ~0.02 m, which drapes to ~40 mm over the ~0.8 m
/// top spans. (Strictly a tensioner consumes ROUTE length, not material length — this shortens
/// links by a cosmetically-nil 0.8%; the honest idler-shift version is parked.)
const CHAIN_SLACK_TRIM: f32 = 0.11;
/// Sprocket motor response time (s): how fast joints engaged on the drive wheel converge to the
/// belt's surface speed. Drive is applied ONLY there — the old all-joint advected anchor
/// injected compression around the whole loop and was itself a zigzag cause (codex, step 22b);
/// the length constraints now transmit drive, so tight and slack sides emerge.
const CHAIN_MOTOR_TAU: f32 = 0.05;
/// Bending stiffness (N·m², REAL units now that node mass is real) of the XPBD turning-angle
/// constraint relative to the route's own curvature. Small on purpose: a pinned track has no
/// bending spring away from its stops — the old normalized B=10 with unit masses was secretly
/// ~160 N·m² of route-shaped spring (part of the rubber-band read). This is a numerical
/// regularizer; the anti-zigzag/anti-flutter duty moved to the pin friction + the route tube.
const CHAIN_BEND_STIFFNESS: f32 = 2.0;
/// Max articulation between consecutive links (rad): must clear the T-34 sprocket's wrap demand
/// of ~31°/joint. A hard link-geometry stop, distinct from the bending energy.
const MAX_LINK_ANGLE: f32 = 35.0 * std::f32::consts::PI / 180.0;
/// Post-solve velocity guardrails (m/s), decomposed in the route frame: route-normal speed caps
/// hard (whip is real but bounded); tangential caps at max(8, |belt| + 5) computed inline. These
/// clamp the STORED velocity after reconstruction — containment, not the root fix (that's the
/// no-restitution reconstruction below).
const CHAIN_MAX_NORMAL_SPEED: f32 = 4.0;
/// Route-tube half-widths (m): how far a joint may sit OUTSIDE the loop (whip overshoot) and
/// INSIDE it (terrain holds the belly a board-stack in off the taut line; slack droops under
/// spans). Both stay below half the belly↔top-run route gap (~0.85 m) so the tube atlas never
/// overlaps — one 2D point, one (s,u). A joint clamped to the tube can never be "off the tank"
/// no matter what the solve did — and on wheel arcs the inner bound is zero, which is what makes
/// wrong-side capture UNREPRESENTABLE (codex Priority B): a node on a wheel sector can only move
/// radially off the rim.
const CHAIN_TUBE_OUT: f32 = 0.30;
const CHAIN_TUBE_IN: f32 = 0.40;
/// Half-width (m) of the windowed route-projection search around a joint's previous route
/// coordinate — ±2 pitches: comfortably above the largest legal per-substep motion (~0.17 m),
/// far below the distance to any other route branch. A window (not a global nearest-point
/// query) is what keeps the rebase from tunneling `s` across overlapping parts of the loop.
const CHAIN_REBASE_WINDOW: f32 = 0.35;

/// The belt contact — an advected pin-line ring, penetration from the field at three fixed
/// stations per link (pin a, midpoint, pin b — on the outer face):
///
/// - the two-piece linear profile between the stations interpolates the interior instead of
///   searching it, so there is nothing to tie-break;
/// - stations are signed (clearance below zero), so the profile's closed-form clipping still
///   finds the lift-off point between stations;
/// - support + traction applied at the profile centroid on the terrain surface
///   (`+ out·(t/2 − pen_c)`), so the lever arm includes the shoe.
pub(super) fn apply_belt_support_field(
    // Tick-truth Position/Rotation, NEVER GlobalTransform: the render pose updates once per
    // FRAME, so on a multi-tick frame the second tick would probe terrain against a stale
    // hull — phantom slip/penetration that the grip state INTEGRATES into real force
    // oscillation (measured: period-2 load alternation, 212↔32 kN, with a perfectly smooth
    // hull). This was "model4's one game-illegal habit" (architecture v3 §0) — now retired;
    // the sim core and the game adapter always agreed on tick truth.
    mut hull: Query<
        (
            &avian3d::prelude::Position,
            &avian3d::prelude::Rotation,
            Forces,
        ),
        With<Hull>,
    >,
    field: Res<TerrainField>,
    raw: Res<RawDriveInput>,
    mut shaped: ResMut<ShapedDrive>,
    time: Res<Time>,
    pin_belt: Res<PinBelt>,
    mut belt: ResMut<BeltSpeed>,
    mut phase: ResMut<BeltPhase>,
    mut contacts: ResMut<BeltContacts>,
    mut dynamics: ResMut<SideDynamics>,
    mut grip: ResMut<BeltGrip>,
    mut grip_elements: ResMut<BeltGripElements>,
    grip_on: Res<GripSwitch>,
    // One tuple param: the function is at Bevy's 16-arg SystemParam ceiling.
    transmission_params: (
        Res<TransSwitch>,
        Res<T34Transmission>,
        ResMut<TransState>,
        ResMut<TransTelemetry>,
    ),
) {
    let (trans, t34, mut trans_state, mut telemetry) = transmission_params;
    let Ok((hull_pos, hull_rot, mut forces)) = hull.single_mut() else {
        return;
    };
    let affine = bevy::math::Affine3A::from_rotation_translation(hull_rot.0, hull_pos.0);
    let to_local = affine.inverse();
    for cs in contacts.0.values_mut() {
        cs.clear(); // the sole contact system this tick
    }
    let dt = time.delta_secs();
    // The shared command seam, on the fixed tick exactly like the game adapter: slew the raw
    // intent, then mix per side.
    shaped.0 = crate::track::drive::shape_drive(shaped.0, raw.0, dt);
    let side_commands = shaped.0.side_commands();

    // The fixed advected ring on the pin line, closed for the core.
    let mut loop_pts = belt_loop(&pin_circles());
    if let Some(&first) = loop_pts.first() {
        loop_pts.push(first);
    }
    let params = force_params(grip_on.0);

    // Phase 1 — both sides' contact passes at their pre-tick belt speeds (transmission-design
    // §2 scheduling: the joint drivetrain needs both reactions before either belt
    // integrates). Force application stays all-left-then-all-right below; within a tick
    // application never feeds back into the velocity field, so evaluating R's contacts
    // before applying L's forces is exact — the governor parity captures pin it byte-level.
    let mut reports: [SideReport; 2] = [SideReport::default(), SideReport::default()];
    let mut live = [false; 2];
    for side in Side::ALL {
        let side_input = SideInput {
            loop_pts: &loop_pts,
            count: pin_belt.count,
            plane_x: side.plane_x(TRACK_HALF_WIDTH),
            command: side_commands[side.index()],
        };
        let state = SideState {
            speed: belt.get(side),
            phase: phase.get(side),
            grip: *grip.0.get(side),
        };
        let elements = match grip_on.0 {
            GripMode::Elements => Some(grip_elements.0.get_mut(side)),
            _ => None,
        };
        let (report, ok) = contact_side(
            &side_input,
            state,
            affine,
            dt,
            &params,
            &field.0,
            |p| forces.velocity_at_point(p),
            elements,
        );
        reports[side.index()] = report;
        live[side.index()] = ok;
    }

    // Phase 2 — ONE joint drivetrain solve. The governor adapter runs the legacy belt math
    // verbatim; the regenerative adapters consume the T-34 lab tables.
    let tr = transmission::step(
        trans.0,
        &params,
        Some(&t34.0),
        &mut trans_state.0,
        &TransmissionInput {
            throttle: shaped.0.throttle,
            steer: shaped.0.steer,
            side_commands,
            speeds: [belt.get(Side::Left), belt.get(Side::Right)],
            reactions: [reports[0].belt_reaction, reports[1].belt_reaction],
            dt,
        },
    );
    telemetry.0 = match trans.0 {
        transmission::TransmissionMode::Governor => None,
        _ => Some(tr),
    };

    // Phase 3 — apply forces in the same per-side report order as ever, commit the state.
    for (si, report) in reports.into_iter().enumerate() {
        let side = Side::ALL[si];
        // Apply in report order — accumulation order is part of bit-reproducibility.
        for app in &report.apps {
            forces.apply_force_at_point(app.force, app.point);
        }
        for c in &report.contacts {
            contacts.0.get_mut(side).push(Contact {
                local: to_local.transform_point3(c.point),
                load: c.load,
                load_elastic: c.load_elastic,
                normal: c.normal,
                slip: c.slip,
                slip_lat: c.slip_lat,
                f_long: c.f_long,
                f_lat: c.f_lat,
                traction: c.traction,
            });
        }
        *dynamics.engine.get_mut(side) = tr.forces[si];
        *dynamics.reaction.get_mut(side) = report.belt_reaction;
        *grip.0.get_mut(side) = report.state.grip;
        if live[si] {
            // Phase advects at the PRE-update speed, exactly like the legacy tail.
            let pre = belt.get(side);
            let advected = phase.get(side) + f64::from(pre * dt);
            belt.set(side, tr.next_speeds[si]);
            phase.set(side, advected);
        }
    }
}

/// The T-34 lab vehicle's DECLARED transmission (phase 2.5): plausible tables in the authored
/// shape — the V-2 diesel's envelope (500 hp @ 1800, peak torque @ ~1100), a 5F/1R ladder
/// whose top gear at governed rpm lands the historical ~52 km/h, and an L600-style radii
/// table anchored at a plausible 3.0 m 1st-gear tight radius with the Tiger-derived 2.958
/// tight:wide ratio. PLACEHOLDER-plausible (the T-34's real box is clutch-and-brake — this
/// config exists to exercise the regenerative adapters on the lab rig), all INFERRED.
#[derive(Resource)]
pub(super) struct T34Transmission(pub(super) TransmissionParams);

impl Default for T34Transmission {
    fn default() -> Self {
        Self(TransmissionParams::from_authoring(&TransmissionAuthoring {
            idle_rpm: 600.0,
            governed_rpm: 1800.0,
            rated_rpm: 1800.0,
            // V-2 diesel: peak ~2200 N·m @ 1100 (INFERRED from the 500 hp/1800 rating);
            // torque runs out at the governed 1800 so the top-speed root is emergent.
            torque_nm: &[
                (600.0, 1650.0),
                (1100.0, 2200.0),
                (1700.0, 1950.0),
                (1800.0, 0.0),
            ],
            // Per-gear top speeds @ 1800 rpm (km/h): geometric-ish ladder to the historical
            // ~52 km/h top; gearing-implied top speed = 14.5 m/s (the straight-line gate).
            forward_speeds_kmh: &[8.0, 12.7, 20.4, 32.6, 52.2],
            reverse_speeds_kmh: &[8.0],
            shift_up_rpm: 1700.0,
            shift_down_rpm: 950.0,
            // (tight, wide) per gear: tight ∝ 1/G from 3.0 m; wide = tight × 2.958.
            steer_radii_m: &[
                (3.0, 8.9),
                (4.8, 14.2),
                (7.7, 22.8),
                (12.3, 36.4),
                (19.7, 58.3),
            ],
            // Steering-member PER-OUTPUT capacity (the fixed convention: the difference
            // axis F_s carries 2× this — 480 kN → 600 kN·m of pivot moment vs the lab's
            // ~224 kN·m scrub). The phase-2.5 sizing (2× the per-track force cap) is kept
            // verbatim: it was chosen under the old F_s-bound reading, and under the fixed
            // convention it simply strengthens the lab's steering authority — the T-34
            // gates re-measure it. Generous for a lab vehicle, all INFERRED.
            steer_capacity_n: 240_000.0,
            neutral_fraction: 0.5,
            recirculation: 0.9,
            // Brake ≈ traction limit (μ·W/2 ≈ 117 kN) — the sound sizing rule.
            brake_capacity_n: 120_000.0,
            // Compression braking ~25% of peak torque (diesel 20–30% band) — INFERRED.
            drag_fraction: 0.25,
            sprocket_radius_m: DRIVE_RADIUS + TRACK_THICKNESS / 2.0,
            half_tread_m: TRACK_HALF_WIDTH,
        }))
    }
}

/// The sandbox's force parameters: the T-34 lab vehicle + the shared support/friction law,
/// assembled for [`track::forces`](crate::track::forces) — the promoted single implementation.
fn force_params(grip: GripMode) -> ForceParams {
    ForceParams {
        thickness: TRACK_THICKNESS,
        columns: COLUMNS,
        support_stiffness_per_m: SUPPORT_STIFFNESS_PER_M,
        support_damping_per_m: SUPPORT_DAMPING_PER_M,
        engage_depth: CONTACT_ENGAGE,
        probe_reach: CONTACT_PROBE,
        mu: MU,
        lateral_ratio: LATERAL_GRIP_RATIO,
        slip_saturation: SLIP_SATURATION,
        max_speed: MAX_BELT_SPEED,
        engine_power: ENGINE_POWER,
        engine_force: ENGINE_FORCE,
        governor_gain: BELT_GOVERNOR_GAIN,
        inertia: BELT_INERTIA,
        // The declared park-target stiffness (forces.rs provenance doc); `Off` = the
        // harness parity switch (`grip=off`): kinetic-only law, bit-identical to the
        // pre-grip baseline. (`Elements` needs it nonzero too — it gates the regime and
        // the belt-hold; the element law derives its stiffness from μ·load/K directly.)
        grip_stiffness: match grip {
            GripMode::Off => 0.0,
            _ => crate::track::forces::grip_stiffness(MU, HULL_MASS * 9.81),
        },
    }
}

/// The route-chain view's solver state — `track::chain::ChainState` behind a sandbox resource.
/// Reset to default for a canonical cold start (view toggle, model switch).
#[derive(Resource, Default)]
pub(super) struct RouteChain(pub(super) ChainState);

/// The sandbox's chain parameters: the T-34 spec values + global solver policy, assembled for
/// [`track::chain`](crate::track::chain). Every field is either vehicle data or quality policy —
/// none is a per-vehicle feel knob (architecture §7).
fn chain_params() -> ChainParams {
    ChainParams {
        substep: CHAIN_SUBSTEP,
        max_substeps: CHAIN_MAX_SUBSTEPS,
        sweeps: CHAIN_SWEEPS,
        half_life_tan: CHAIN_HALF_LIFE_TAN,
        half_life_norm: CHAIN_HALF_LIFE_NORM,
        node_mass: CHAIN_NODE_MASS,
        hinge_torque: CHAIN_HINGE_TORQUE,
        motor_tau: CHAIN_MOTOR_TAU,
        bend_stiffness: CHAIN_BEND_STIFFNESS,
        max_link_angle: MAX_LINK_ANGLE,
        max_normal_speed: CHAIN_MAX_NORMAL_SPEED,
        tube_out: CHAIN_TUBE_OUT,
        tube_in: CHAIN_TUBE_IN,
        rebase_window: CHAIN_REBASE_WINDOW,
        thickness: TRACK_THICKNESS,
        lateral_stations: [COLUMNS[0].0, COLUMNS[1].0, COLUMNS[2].0],
        probe_reach: CONTACT_PROBE,
    }
}

/// The **route-chain view** (`V` toggle) — the simulated chain tier, step 24 math, now
/// living in [`track::chain`](crate::track::chain) (step 25 extraction): the sandbox side of the
/// seam only gathers inputs (articulated circles, belt scalars, hull affine, gravity), calls
/// `ChainState::step`, and writes the outputs into the sandbox's draw resources. The game's
/// phase-A view plugin will consume the identical core behind the tank rig.
pub(super) fn conform_belts_field_chain(
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Transform)>,
    field: Res<TerrainField>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    belt: Res<BeltSpeed>,
    time: Res<Time>,
    mut chain: ResMut<RouteChain>,
    mut belts: ResMut<ConformedBelts>,
    mut reference: ResMut<ChainReference>,
    // Perf probe: (busy seconds, substep-sides, frames) — the promotion-budget number.
    mut perf: Local<(f64, u64, u64)>,
) {
    let t_perf = std::time::Instant::now();
    let hull = *hull;
    let affine = hull.affine();
    let to_local = affine.inverse();
    let g3 = to_local.transform_vector3(Vec3::NEG_Y * 9.81);
    let g2 = Vec2::new(g3.z, g3.y);

    // Per-side pin-line circles, front→rear: fixed drive circles + the ARTICULATED road wheels,
    // sorted so the envelope scan and the frame-to-frame interpolation see a stable order.
    let (sprocket, idler) = drive_circles_local();
    let side_circles: [Vec<(Vec2, f32)>; 2] = Side::ALL.map(|side| {
        let mut roads: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _)| w.side == side && w.kind == WheelKind::Road)
            .map(|(_, t)| {
                (
                    Vec2::new(t.translation.z, t.translation.y),
                    ROAD_RADIUS + TRACK_THICKNESS / 2.0,
                )
            })
            .collect();
        roads.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        let mut circles = vec![(sprocket.0, sprocket.1 + TRACK_THICKNESS / 2.0)];
        circles.extend(roads);
        circles.push((idler.0, idler.1 + TRACK_THICKNESS / 2.0));
        circles
    });

    // The IMMUTABLE material length: the authored belt minus the tensioner-preload stand-in.
    let chain_len = pin_belt.length - CHAIN_SLACK_TRIM;
    let input = ChainInput {
        dt: time.delta_secs(),
        affine,
        gravity_local: g2,
        belt_len: chain_len,
        count: pin_belt.count,
        sides: [
            ChainSideInput {
                circles: &side_circles[0],
                belt_speed: belt.get(Side::Left),
                phase: phase.get(Side::Left).rem_euclid(f64::from(chain_len)) as f32,
                plane_x: -TRACK_HALF_WIDTH,
            },
            ChainSideInput {
                circles: &side_circles[1],
                belt_speed: belt.get(Side::Right),
                phase: phase.get(Side::Right).rem_euclid(f64::from(chain_len)) as f32,
                plane_x: TRACK_HALF_WIDTH,
            },
        ],
    };
    let mut out: [Vec<Vec2>; 2] = [Vec::new(), Vec::new()];
    let report = chain.0.step(&input, &chain_params(), &field.0, &mut out);
    if report.tears + report.overruns > 0 {
        warn!(
            "route-chain reseed: {} tear-fuse, {} overrun",
            report.tears, report.overruns
        );
    }

    for (si, side) in Side::ALL.into_iter().enumerate() {
        let track_x = input.sides[si].plane_x;
        // The current route is the `-` viz layer: chain-vs-route deviation shows exactly where
        // terrain, slack, and whip hold the belt off its taut path.
        let route_now = build_route(&side_circles[si], chain_len);
        let ref_world: Vec<Vec3> = route_now
            .pts
            .iter()
            .map(|p| affine.transform_point3(Vec3::new(track_x, p.y, p.x)))
            .collect();
        let samples: Vec<BeltSample> = out[si]
            .iter()
            .map(|&p| BeltSample {
                local: p,
                world: affine.transform_point3(Vec3::new(track_x, p.y, p.x)),
            })
            .collect();
        match side {
            Side::Left => reference.left = ref_world,
            Side::Right => reference.right = ref_world,
        }
        *belts.get_mut(side) = samples;
    }
    perf.0 += t_perf.elapsed().as_secs_f64();
    perf.1 += report.substeps as u64 * 2;
    perf.2 += 1;
    if perf.2.is_multiple_of(512) {
        info!(
            "route-chain perf: {:.0} µs/frame avg | {:.1} µs/substep-side ({} substep-sides / {} frames)",
            perf.0 / perf.2 as f64 * 1e6,
            perf.0 / (perf.1 as f64).max(1.0) * 1e6,
            perf.1,
            perf.2
        );
    }
}

/// Critically-damped ease frequency (rad/s) of a wrap-view wheel's RISE (settle ≈ 4.7/ω ≈
/// 100 ms). Integrated implicitly — see [`articulate_wheels_field`].
const WHEEL_EASE_OMEGA: f32 = 45.0;

/// The road wheels, placed directly from the terrain FIELD — wheels first, then the belt
/// wraps them (`ground → wheels → belt`, acyclic; the step-21 circular order was the root of
/// the teleport/settle wrong-side captures). Probe + easing live in
/// [`track::wheels`](crate::track::wheels) (step 25 extraction): implicit critically-damped
/// rise, ballistic fall, deepest of the physics' lateral columns along the lower arc.
pub(super) fn articulate_wheels_field(
    hull: Single<&GlobalTransform, With<Hull>>,
    field: Res<TerrainField>,
    time: Res<Time>,
    mut wheels: Query<(&RigWheel, &mut Suspension, &mut Transform)>,
) {
    let affine = hull.affine();
    let down = affine.transform_vector3(Vec3::NEG_Y).normalize_or_zero();
    let params = WheelParams {
        // Wheel surface + the track plate riding between it and the ground.
        reach: ROAD_RADIUS + TRACK_THICKNESS,
        ease_omega: WHEEL_EASE_OMEGA,
        max_lift: SUSP_MAX_LIFT,
        lateral_stations: [COLUMNS[0].0, COLUMNS[1].0, COLUMNS[2].0],
        probe_reach: CONTACT_PROBE,
    };
    for (wheel, mut susp, mut transform) in &mut wheels {
        if wheel.kind != WheelKind::Road {
            continue;
        }
        let target = wheel_lift_target(&field.0, &affine, down, susp.pivot_local, &params);
        susp.target = target;
        let (mut dy, mut dvel) = (susp.dy, susp.dvel);
        wheel_lift_step(&mut dy, &mut dvel, target, time.delta_secs(), &params);
        susp.dy = dy;
        susp.dvel = dvel;
        transform.translation.y = susp.pivot_local.y + susp.dy;
    }
}

/// The default track view — a **stateless kinematic wrap** (step 22): no integration, no
/// constraints, no per-frame memory. The path is recomputed from scratch every frame as a pure
/// function of the articulated wheels, the terrain field, and the belt phase:
///
/// 1. **taut wrap** — the lower convex envelope of the pin-line circles (tangent segments + wheel
///    arcs, front→rear; a wheel above the taut line between its neighbours simply drops out);
/// 2. **terrain conform** — every ground-facing station displaced along its outward normal by the
///    directional field depth, max over the SAME 3 lateral columns the physics samples (the
///    visual≡physics invariant, kept);
/// 3. **top run** — the leftover belt length as a sag parabola between the drive wheels' upper
///    tangents, clipped from above onto the wheel circles (the loose T-34 return run rides its
///    road wheels); the conform feeds the length budget FORWARD, so belly lift shortens the sag
///    the same frame (no smoothed `belly_extra` feedback);
/// 4. **links** — the closed path resampled at link pitch with the belt phase.
///
/// Wrong-side wheel capture, compression zigzag, teleport transients, and solver stability are
/// not tuned away here — they are unrepresentable: there is no state to capture, buckle, stale,
/// or diverge. Remote tanks render identically on every client as a pure function of replicated
/// pose + phase (ADR-0014 satisfied by construction).
pub(super) fn conform_belts_field(
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Suspension)>,
    field: Res<TerrainField>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    mut belts: ResMut<ConformedBelts>,
    mut reference: ResMut<ChainReference>,
    // Perf probe: (busy seconds, frames) — the wrap's side of the promotion budget.
    mut perf: Local<(f64, u64)>,
) {
    let t_perf = std::time::Instant::now();
    let affine = hull.affine();
    for side in Side::ALL {
        let track_x = side.plane_x(TRACK_HALF_WIDTH);
        // Pin-line circles, front→rear: sprocket, the ARTICULATED road wheels, idler.
        let (sprocket, idler) = drive_circles_local();
        let mut roads: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _)| w.side == side && w.kind == WheelKind::Road)
            .map(|(_, s)| {
                (
                    Vec2::new(s.pivot_local.z, s.pivot_local.y + s.dy),
                    ROAD_RADIUS + TRACK_THICKNESS / 2.0,
                )
            })
            .collect();
        roads.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        let mut circles = vec![(sprocket.0, sprocket.1 + TRACK_THICKNESS / 2.0)];
        circles.extend(roads.iter().copied());
        circles.push((idler.0, idler.1 + TRACK_THICKNESS / 2.0));

        // 1. Lower convex envelope over the ordered circles (Graham-style scan): a circle whose
        // body stays above its neighbours' lower tangent is not part of the taut run and drops
        // out — a lifted wheel is skipped, never wrapped from the wrong side (the route-selection
        // rule; fixed logical order, no per-frame hull search).
        let mut active: Vec<usize> = vec![0];
        for k in 1..circles.len() {
            while active.len() >= 2 {
                let (p, a) = (active[active.len() - 2], active[active.len() - 1]);
                let (t0, _) =
                    external_tangent(circles[p].0, circles[p].1, circles[k].0, circles[k].1, -1.0);
                // Unit lower normal of the p→k tangent line (t0 sits on circle p by construction).
                let n = (t0 - circles[p].0) / circles[p].1;
                // Keep `a` only if it protrudes below that line.
                if (circles[a].0 - t0).dot(n) + circles[a].1 > 1e-4 {
                    break;
                }
                active.pop();
            }
            active.push(k);
        }

        // The taut bottom polyline, sprocket_up → front arc → tangents/arcs → idler_up.
        let (sprocket_c, sprocket_r) = circles[0];
        let (idler_c, idler_r) = *circles.last().unwrap();
        let (idler_up, sprocket_up) =
            external_tangent(idler_c, idler_r, sprocket_c, sprocket_r, 1.0);
        let mut bottom: Vec<Vec2> = Vec::new();
        let mut cursor = sprocket_up;
        for w in active.windows(2) {
            let (i, j) = (w[0], w[1]);
            let (t0, t1) =
                external_tangent(circles[i].0, circles[i].1, circles[j].0, circles[j].1, -1.0);
            let toward = if i == 0 {
                Vec2::new(-1.0, 0.0) // the sprocket wraps around its front
            } else {
                Vec2::new(0.0, -1.0) // road wheels wrap under
            };
            bottom.extend(arc(circles[i].0, circles[i].1, cursor, t0, toward));
            bottom.push(t1);
            cursor = t1;
        }
        bottom.extend(arc(idler_c, idler_r, cursor, idler_up, Vec2::new(1.0, 0.0)));

        // The taut (unconformed) loop is the `-` reference layer: chain-vs-reference deviation
        // shows exactly where terrain holds the belt off its rest path.
        let ref_loop = close_loop(&bottom, idler_up, sprocket_up, pin_belt.length, &roads);
        let ref_world: Vec<Vec3> = ref_loop
            .iter()
            .map(|p| affine.transform_point3(Vec3::new(track_x, p.y, p.x)))
            .collect();
        match side {
            Side::Left => reference.left = ref_world,
            Side::Right => reference.right = ref_world,
        }

        // 2. Terrain conform: displace each ground-facing station AGAINST its outward normal by
        // the directional field depth — a buried station is lifted back INSIDE the loop until its
        // outer face sits on the terrain surface (belly rises onto boards, nose backs off a
        // wall). The step-22 first cut had this sign inverted, pushing the belly INTO boards and
        // the nose off the sprocket — Yan's wall/phase-through findings. Deepest of the physics'
        // 3 lateral columns; C0 because the field is rounded.
        //
        // Conform on a DENSE resample, not the wrap's vertices: a tangent segment between two
        // wheels is one long edge — with only its endpoints conformed, a board mid-segment goes
        // unsampled and the belt cuts through it (the second half of the phase-through finding).
        let mut bottom = resample(&bottom, BELT_DRAW_SPACING, 0.0);
        bottom.push(idler_up);
        let m = bottom.len();
        let outs: Vec<Vec2> = (0..m)
            .map(|i| {
                let tan =
                    (bottom[(i + 1).min(m - 1)] - bottom[i.saturating_sub(1)]).normalize_or_zero();
                Vec2::new(tan.y, -tan.x)
            })
            .collect();
        let depths: Vec<f32> = (0..m)
            .map(|i| {
                let out2 = outs[i];
                if out2 == Vec2::ZERO {
                    return 0.0;
                }
                let s2 = bottom[i] + out2 * (TRACK_THICKNESS / 2.0);
                let w = affine.transform_point3(Vec3::new(track_x, s2.y, s2.x));
                let out = affine
                    .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                    .normalize_or_zero();
                let tan = Vec2::new(-out2.y, out2.x);
                let axis = affine
                    .transform_vector3(Vec3::new(0.0, tan.y, tan.x))
                    .normalize_or_zero();
                let lat = out.cross(axis);
                let mut d = 0.0_f32;
                for (offset, _) in COLUMNS {
                    d = d.max(field.depth_along(w + lat * offset, out));
                }
                d.max(0.0)
            })
            .collect();
        // A rigid link OVERHANGS a board edge: the line stays high for about half a pitch before
        // the pin clears the edge, then articulates down over the next — the chain got this from
        // its per-link constraint. Reproduce it on the displacement field: a ±1-station max
        // filter (the overhang; never sinks a lift) followed by a 3-tap triangular smooth (the
        // articulation rounding). Without it, the pointwise ramp starts AT the edge and the belt
        // shaves the corner (~100 mm transients at the 0.18 m boards).
        let widened: Vec<f32> = (0..m)
            .map(|i| {
                depths[i.saturating_sub(1)]
                    .max(depths[i])
                    .max(depths[(i + 1).min(m - 1)])
            })
            .collect();
        let conformed: Vec<Vec2> = (0..m)
            .map(|i| {
                let d = 0.25 * widened[i.saturating_sub(1)]
                    + 0.5 * widened[i]
                    + 0.25 * widened[(i + 1).min(m - 1)];
                if d > 0.0 {
                    bottom[i] - outs[i] * d
                } else {
                    bottom[i]
                }
            })
            .collect();

        // 3 + 4. Close with the budgeted sag and scroll the links along the loop.
        let mut loop_pts = close_loop(&conformed, idler_up, sprocket_up, pin_belt.length, &roads);
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / pin_belt.count.max(1) as f32;
        // Resample offset from the canonical decomposition (wrap count unused for the wrap view).
        let (_, offset) = phase_decompose(phase.get(side), pitch);
        let mut joints = resample(&loop_pts, pitch, offset);
        joints.truncate(pin_belt.count);
        if joints.len() < 3 {
            continue;
        }
        let samples: Vec<BeltSample> = joints
            .iter()
            .map(|&p| BeltSample {
                local: p,
                world: affine.transform_point3(Vec3::new(track_x, p.y, p.x)),
            })
            .collect();
        *belts.get_mut(side) = samples;
    }
    perf.0 += t_perf.elapsed().as_secs_f64();
    perf.1 += 1;
    if perf.1.is_multiple_of(512) {
        info!(
            "kinematic-wrap perf: {:.0} µs/frame avg ({} frames)",
            perf.0 / perf.1 as f64 * 1e6,
            perf.1
        );
    }
}

/// Close a bottom polyline (sprocket_up → … → idler_up) into the full belt loop: the belt length
/// left over after the bottom run becomes the return run's drape ([`sag_span`]). The
/// `max(0)` on the excess is the explicit length-budget clamp: a conform-lengthened bottom run
/// beyond the total belt length runs the top taut instead of laundering the deficit into the
/// shape (the step-22 infeasibility rule).
fn close_loop(
    bottom: &[Vec2],
    idler_up: Vec2,
    sprocket_up: Vec2,
    belt_length: f32,
    wheels: &[(Vec2, f32)],
) -> Vec<Vec2> {
    let mut pts = bottom.to_vec();
    let chord = idler_up.distance(sprocket_up);
    let excess = (belt_length - polyline_len(bottom) - chord).max(0.0);
    sag_span(idler_up, sprocket_up, excess, wheels, 0, &mut pts);
    pts
}

/// The `9` viz layer: the collocation stations at the **physics** ring (pins + mids
/// on the outer face) — grey when clear of terrain, orange when penetrating. The whole oracle,
/// visible.
pub(super) fn draw_sample_points(
    mut gizmos: Gizmos,
    viz: Res<VizLayers>,
    hull: Single<&GlobalTransform, With<Hull>>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    field: Res<TerrainField>,
) {
    if !viz.casts {
        return;
    }
    let affine = hull.affine();
    for side in Side::ALL {
        let track_x = side.plane_x(TRACK_HALF_WIDTH);
        let mut loop_pts = belt_loop(&pin_circles());
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / pin_belt.count.max(1) as f32;
        // Resample offset from the canonical decomposition (wrap count unused here).
        let (_, offset) = phase_decompose(phase.get(side), pitch);
        let mut stations = resample(&loop_pts, pitch, offset);
        stations.truncate(pin_belt.count);
        let n = stations.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = stations[i];
            let b = stations[(i + 1) % n];
            let seg = b - a;
            let len = seg.length();
            if len < 1e-4 {
                continue;
            }
            let tan2 = seg / len;
            let out2 = Vec2::new(tan2.y, -tan2.x);
            let wa = affine.transform_point3(Vec3::new(track_x, a.y, a.x));
            let wb = affine.transform_point3(Vec3::new(track_x, b.y, b.x));
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let axis = (wb - wa) / len;
            let lat = out.cross(axis);
            let face = out * (TRACK_THICKNESS / 2.0);
            for (offset, _) in COLUMNS {
                let shift = lat * offset;
                let (ca, cb) = (wa + shift, wb + shift);
                for s in [ca + face, (ca + cb) / 2.0 + face, cb + face] {
                    let color = if field.depth_along(s, out) > 0.0 {
                        TRACTION_FORCE_COLOR
                    } else {
                        CAST_COLOR
                    };
                    gizmos.sphere(Isometry3d::from_translation(s), 0.015, color);
                }
            }
        }
    }
}
