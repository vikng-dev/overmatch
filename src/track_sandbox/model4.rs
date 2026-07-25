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
//! The shoe enters laterally as three **columns** — the MEASURED shoe faces + centre
//! ([`RigGeom::grip_column_offsets`](crate::track::rig_geom::RigGeom::grip_column_offsets),
//! Simpson-weighted, per-side signed because the shoe is not centred on its pins): each column
//! samples its own three stations, owns its share of the per-metre coefficients, and applies its
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
use crate::track::transmission::{self, TransmissionInput, TransmissionParams};
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

/// The belt as the physics sees it: the MATERIAL loop's length and its link count. Both come
/// straight off [`RigGeom`] — `pitch × link_count` and `link_count` — because the loop is a real
/// chain of rigid links, not a length budget: there is no slack term to add and no rounding to do.
/// (The old rig derived both from a taut wrap plus an authored `TRACK_SLACK`; slack is now simply
/// what the authored link count leaves over, reported by `RigGeom::window`.)
#[derive(Resource, Default)]
pub(super) struct PinBelt {
    pub(super) length: f32,
    pub(super) count: usize,
}

impl PinBelt {
    pub(super) fn for_rig(geom: &RigGeom) -> Self {
        Self {
            length: geom.belt_len(),
            count: geom.link_count,
        }
    }
}

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
// Node mass, pin friction torque, and the articulation stop are VEHICLE data, authored per tank
// (`track.link_mass` / `hinge_torque` / `link_angle`) and carried on [`RigSpec`] — the game's
// `track::view` reads the same fields. What they mean:
//   * node mass (kg) — one link assembly. It enters the XPBD denominators (w = 1/m), which is what
//     makes the bending compliance and the friction torque REAL units instead of normalized view
//     parameters.
//   * pin dry-friction torque (N·m) — a torque-LIMITED XPBD hinge constraint toward the joint's
//     previous material angle (multiplier accumulated across sweeps, clamped once per substep at
//     |λ| ≤ τ·h²). THE physical rope-vs-track differentiator: real track pins are heavily-loaded
//     dry steel bearings, so flutter dies within a link or two and slack settles near-polygonal,
//     while bulk yank passes through because it doesn't articulate joints.
//   * the link-angle pair (rad, both positive) — the hard link-geometry stops, distinct from the
//     bending energy. ASYMMETRIC: the inward one (toward the wheels) is the wrap direction and
//     must clear every running-gear circle's demand per joint; the outward one is the tighter
//     stop the ground-side structure imposes, and it is what limits sag and bump crests.
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
    // Tuple params: the function is at Bevy's 16-arg SystemParam ceiling.
    transmission_params: (
        Res<TransSwitch>,
        Res<RigTransmission>,
        ResMut<TransState>,
        ResMut<TransTelemetry>,
    ),
    rig: (Res<RigGeom>, Res<RigSpec>, Res<EnvelopeLaw>),
) {
    let (trans, transmission, mut trans_state, mut telemetry) = transmission_params;
    let (geom, rig_spec, envelope) = rig;
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

    let params = force_params(&geom, &rig_spec, &envelope);

    // Phase 1 — both sides' contact passes at their pre-tick belt speeds (transmission-design
    // §2 scheduling: the joint drivetrain needs both reactions before either belt
    // integrates). Force application stays all-left-then-all-right below; within a tick
    // application never feeds back into the velocity field, so evaluating R's contacts
    // before applying L's forces is exact — the governor parity captures pin it byte-level.
    let mut reports: [SideReport; 2] = [SideReport::default(), SideReport::default()];
    let mut live = [false; 2];
    for side in Side::ALL {
        // The fixed advected ring on this side's REST pin line, closed for the core. Per side, not
        // one mirrored loop: the derived running gear is measured per side, so nothing here assumes
        // the two sides are exact mirror images.
        let mut loop_pts = belt_loop(geom.rest.get(side));
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let side_input = SideInput {
            loop_pts: &loop_pts,
            count: pin_belt.count,
            plane_x: side.plane_x(geom.plane_x),
            columns: geom.grip_column_offsets(side),
            command: side_commands[side.index()],
            // The envelope's per-position reach profile: full droop over the road-wheel span,
            // tapering to zero at the unsprung sprocket/idler centres.
            travel: Some(crate::track::forces::TravelField {
                knots: &envelope.knots.get(side)[..],
            }),
        };
        let state = SideState {
            speed: belt.get(side),
            phase: phase.get(side),
            grip: *grip.0.get(side),
        };
        let elements = grip_elements.0.get_mut(side);
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

    // Phase 2 — ONE joint drivetrain solve. The governor adapter runs the direct belt math
    // verbatim; the regenerative adapters consume the Tiger's declared L600 tables.
    let tr = transmission::step(
        trans.0,
        &params,
        Some(&transmission.0),
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
            // Phase advects at the PRE-update speed (the belt advects before it re-integrates).
            let pre = belt.get(side);
            let advected = phase.get(side) + f64::from(pre * dt);
            belt.set(side, tr.next_speeds[si]);
            phase.set(side, advected);
        }
    }
}

/// The rig's DECLARED transmission — the Tiger's own `track.powertrain.transmission` block, built
/// in [`build_rig`](super::build_rig) through the same validated seam (`TrackSpec::transmission_params`)
/// the game builds it with. It replaces the sandbox's old hand-authored T-34 lab tables: those
/// existed to exercise the regenerative adapters on a lab vehicle, and the lab vehicle is now the
/// shipped one, so the adapters run the real L600 fixed-radius box against the real HL230 curve.
#[derive(Resource)]
pub(super) struct RigTransmission(pub(super) TransmissionParams);

/// The calibrated contact-envelope suspension law (the "green band" model): the support
/// spring's free length sits at the DROOP envelope (maximum suspension reach, chain-clamped),
/// its travel is the green→rest band, and its rate is derived from the authored ride
/// frequency — so on flat ground the envelope compresses from green to EXACTLY the authored
/// rest pose under the tank's own weight, by construction. Recalibrated whenever the rig or the
/// suspension knobs move.
#[derive(Resource, Clone, Debug)]
pub(super) struct EnvelopeLaw {
    /// Rest outer face → green envelope (m): `RigGeom::droop_travel(..).effective`
    /// (chain-clamped), floored at [`crate::track::envelope::MIN_ENVELOPE_TRAVEL`].
    pub(super) free_travel: f32,
    /// Per-side free-travel profile knots ((z, travel): 0 at the unsprung sprocket/idler
    /// centres, full travel at every road wheel) — the [`TravelField`] the force step rides,
    /// so the envelope reaches only where the suspension physically can.
    pub(super) knots: PerSide<Vec<(f32, f32)>>,
    /// Derived support stiffness (N/m per metre of contacting belt): `weight / A₀`, where
    /// `A₀` is the law's own engage-weighted penetration area at the authored rest pose on
    /// flat ground — measured by running [`contact_side`] once at unit stiffness, so the
    /// calibration shares every branch (columns, engage ramp, profile clipping) with the law
    /// it calibrates. Linear in `k` ⇒ the rest pose carries exactly the weight.
    pub(super) stiffness_per_m: f32,
    /// Derived support damping (N·s/m per metre): the ride mode's `2ζ√(k_total·m)` spread
    /// over the rest contact length (`A₀ / travel`, the engaged band's mean-value length).
    pub(super) damping_per_m: f32,
}

/// Calibrate the envelope law against the rig at its authored rest pose (see [`EnvelopeLaw`]).
/// A thin adapter over the promoted [`crate::track::envelope::calibrate`] — the SAME derivation
/// the game's `init_track_gear` runs, fed the sandbox's own per-side belt loops.
pub(super) fn calibrate_envelope(
    geom: &RigGeom,
    sus: &SuspensionParams,
    weight_n: f32,
    engage_depth: f32,
) -> EnvelopeLaw {
    use crate::track::envelope::{self, EnvelopeSide, MIN_ENVELOPE_TRAVEL};

    let travel = geom.droop_travel(sus);
    if travel.effective < MIN_ENVELOPE_TRAVEL {
        warn!(
            "envelope: link_count {} leaves {:.1} mm of droop (floor {:.0} mm) — the law \
             degenerates to a near-rigid pad; retune the link count",
            geom.link_count,
            travel.effective * 1e3,
            MIN_ENVELOPE_TRAVEL * 1e3,
        );
    }
    let mut loops = Side::ALL.map(|side| belt_loop(geom.rest.get(side)));
    for pts in &mut loops {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    // Floor BEFORE building the profile, so the knots and the calibrated travel agree.
    let floored = travel.effective.max(MIN_ENVELOPE_TRAVEL);
    let knots = Side::ALL.map(|side| envelope::wheel_travel_knots(geom.rest.get(side), floored));
    let sides = Side::ALL.map(|side| EnvelopeSide {
        loop_pts: &loops[side.index()][..],
        plane_x: side.plane_x(geom.plane_x),
        knots: &knots[side.index()][..],
        columns: geom.grip_column_offsets(side),
    });
    let cal = envelope::calibrate(
        &sides,
        geom.link_count,
        geom.model.pin_to_outer,
        engage_depth,
        CONTACT_PROBE,
        weight_n,
        travel.effective,
        sus.ride_frequency,
        sus.damping_ratio,
    );
    let [left_knots, right_knots] = knots;
    EnvelopeLaw {
        free_travel: cal.free_travel,
        stiffness_per_m: cal.stiffness_per_m,
        damping_per_m: cal.damping_per_m,
        knots: PerSide::new(left_knots, right_knots),
    }
}

/// The sandbox's force parameters: the Tiger's own geometry ([`RigGeom`]) and vehicle data
/// ([`RigSpec`]) + the calibrated contact-envelope support law + the shared friction law,
/// assembled for [`track::forces`](crate::track::forces) — the promoted single implementation. The
/// contact datum is the measured `pin_to_outer`: the face offset is geometry, not law policy.
fn force_params(geom: &RigGeom, rig: &RigSpec, envelope: &EnvelopeLaw) -> ForceParams {
    ForceParams {
        face_offset: geom.model.pin_to_outer,
        free_travel: envelope.free_travel,
        support_stiffness_per_m: envelope.stiffness_per_m,
        support_damping_per_m: envelope.damping_per_m,
        engage_depth: rig.engage_depth,
        probe_reach: CONTACT_PROBE,
        mu: MU,
        slip_saturation: SLIP_SATURATION,
        max_speed: rig.max_speed,
        engine_power: rig.engine_power,
        engine_force: rig.engine_force,
        governor_gain: rig.governor_gain,
        inertia: rig.belt_inertia,
        // The declared park-target stiffness (forces.rs provenance doc): it gates the traction
        // regime and the belt-hold; the shear law derives its own stiffness from μ·load/K.
        grip_stiffness: crate::track::forces::grip_stiffness(MU, rig.weight_n),
    }
}

/// The route-chain view's solver state — `track::chain::ChainState` behind a sandbox resource.
/// Reset to default for a canonical cold start (view toggle, model switch).
#[derive(Resource, Default)]
pub(super) struct RouteChain(pub(super) ChainState);

/// The sandbox's chain parameters: the rig's measured geometry + global solver policy, assembled
/// for [`track::chain`](crate::track::chain). Every field is either vehicle data or quality policy —
/// none is a per-vehicle feel knob (architecture §7).
fn chain_params(geom: &RigGeom, rig: &RigSpec) -> ChainParams {
    ChainParams {
        substep: CHAIN_SUBSTEP,
        max_substeps: CHAIN_MAX_SUBSTEPS,
        sweeps: CHAIN_SWEEPS,
        half_life_tan: CHAIN_HALF_LIFE_TAN,
        half_life_norm: CHAIN_HALF_LIFE_NORM,
        node_mass: rig.link_mass,
        hinge_torque: rig.hinge_torque,
        motor_tau: CHAIN_MOTOR_TAU,
        bend_stiffness: CHAIN_BEND_STIFFNESS,
        link_angle_inward: rig.link_angle_inward,
        link_angle_outward: rig.link_angle_outward,
        max_normal_speed: CHAIN_MAX_NORMAL_SPEED,
        tube_out: CHAIN_TUBE_OUT,
        tube_in: CHAIN_TUBE_IN,
        rebase_window: CHAIN_REBASE_WINDOW,
        thickness: geom.thickness,
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
    geom: Res<RigGeom>,
    rig: Res<RigSpec>,
    // Perf probe: (busy seconds, substep-sides, frames) — the promotion-budget number.
    mut perf: Local<(f64, u64, u64)>,
) {
    let t_perf = std::time::Instant::now();
    let hull = *hull;
    let affine = hull.affine();
    let to_local = affine.inverse();
    let g3 = to_local.transform_vector3(Vec3::NEG_Y * 9.81);
    let g2 = Vec2::new(g3.z, g3.y);

    // Per-side pin-line circles, front→rear: the hull-fixed drive circles from the derived rest
    // list (already ON the pin line — no inflation) + the ARTICULATED road wheels at their pin
    // radius, sorted so the envelope scan and the frame-to-frame interpolation see a stable order.
    let wheel_r = geom.wheel_pin_radius();
    let side_circles: [Vec<(Vec2, f32)>; 2] = Side::ALL.map(|side| {
        let rest = geom.rest.get(side);
        let mut roads: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _)| w.side == side && w.kind == WheelKind::Road)
            .map(|(_, t)| (Vec2::new(t.translation.z, t.translation.y), wheel_r))
            .collect();
        roads.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        let mut circles = vec![rest[0]];
        circles.extend(roads);
        circles.push(*rest.last().expect("a side has a sprocket and an idler"));
        circles
    });

    // The IMMUTABLE material length: the authored loop, `pitch × link_count`, exact. The old
    // `CHAIN_SLACK_TRIM` (a tensioner-preload stand-in that shortened a slack-budgeted belt) is
    // retired with the budget it corrected — the link count IS the tension now, and trimming a
    // derived loop below its own taut wrap would simply make the chain unsolvable.
    let chain_len = pin_belt.length;
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
                plane_x: Side::Left.plane_x(geom.plane_x),
                lateral_stations: geom.grip_stations(Side::Left),
            },
            ChainSideInput {
                circles: &side_circles[1],
                belt_speed: belt.get(Side::Right),
                phase: phase.get(Side::Right).rem_euclid(f64::from(chain_len)) as f32,
                plane_x: Side::Right.plane_x(geom.plane_x),
                lateral_stations: geom.grip_stations(Side::Right),
            },
        ],
    };
    let mut out: [Vec<Vec2>; 2] = [Vec::new(), Vec::new()];
    let report = chain
        .0
        .step(&input, &chain_params(&geom, &rig), &field.0, &mut out);
    if report.tears + report.overruns > 0 {
        // `debug!`, not `warn!`: a tear-fuse reseed is a cosmetic self-heal (the chain view is
        // reseedable-from-data by construction, never sim state), and it recurs every frame
        // past the chain's speed ceiling — the game's `track::view` demoted the same message
        // for the same reason.
        debug!(
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
    geom: Res<RigGeom>,
    suspension: Res<RigSuspension>,
    mut wheels: Query<(&RigWheel, &mut Suspension, &mut Transform)>,
) {
    let affine = hull.affine();
    let down = affine.transform_vector3(Vec3::NEG_Y).normalize_or_zero();
    // Per side: the lateral stations are the measured shoe columns, signed per-side.
    let params = Side::ALL.map(|side| WheelParams {
        // Hub to the plate's GROUND face: the measured tread the wheel rolls on, plus the whole
        // plate riding between it and the ground (pin→inner + pin→outer, both measured — no
        // mid-plate assumption).
        reach: geom.wheel_pin_radius() + geom.model.pin_to_outer,
        ease_omega: WHEEL_EASE_OMEGA,
        // The physical contact envelope: up to the bump stop, down to the CHAIN-CLAMPED droop —
        // so a fully-drooped view wheel's drawn belt wrap stays feasible at chain-limited link
        // counts (the same `effective` the envelope law and hard-stop are built on).
        max_lift: suspension.0.bump_stop,
        max_droop: geom.droop_travel(&suspension.0).effective,
        lateral_stations: geom.grip_stations(side),
        probe_reach: CONTACT_PROBE,
    });
    for (wheel, mut susp, mut transform) in &mut wheels {
        if wheel.kind != WheelKind::Road {
            continue;
        }
        let params = &params[wheel.side.index()];
        let target = wheel_lift_target(&field.0, &affine, down, susp.pivot_local, params);
        susp.target = target;
        let (mut dy, mut dvel) = (susp.dy, susp.dvel);
        wheel_lift_step(&mut dy, &mut dvel, target, time.delta_secs(), params);
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
///    tangents, clipped from above onto the wheel circles (a slack return run rides its road
///    wheels); the conform feeds the length budget FORWARD, so belly lift shortens the sag
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
    geom: Res<RigGeom>,
    // Perf probe: (busy seconds, frames) — the wrap's side of the promotion budget.
    mut perf: Local<(f64, u64)>,
) {
    let t_perf = std::time::Instant::now();
    let affine = hull.affine();
    let wheel_r = geom.wheel_pin_radius();
    for side in Side::ALL {
        let track_x = side.plane_x(geom.plane_x);
        // Pin-line circles, front→rear: the hull-fixed sprocket + idler straight from the derived
        // rest list (already pin-line), the ARTICULATED road wheels at their pin radius.
        let rest = geom.rest.get(side);
        let mut roads: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _)| w.side == side && w.kind == WheelKind::Road)
            .map(|(_, s)| (Vec2::new(s.pivot_local.z, s.pivot_local.y + s.dy), wheel_r))
            .collect();
        roads.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        let mut circles = vec![rest[0]];
        circles.extend(roads.iter().copied());
        circles.push(*rest.last().expect("a side has a sprocket and an idler"));

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
                let s2 = bottom[i] + out2 * (geom.thickness / 2.0);
                let w = affine.transform_point3(Vec3::new(track_x, s2.y, s2.x));
                let out = affine
                    .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                    .normalize_or_zero();
                // Station offsets are hull-x measurements (shoe faces relative to the pin
                // plane) — shift along the hull's lateral axis, per-side signed.
                let lat_axis = affine.transform_vector3(Vec3::X);
                let mut d = 0.0_f32;
                for offset in geom.grip_stations(side) {
                    d = d.max(field.depth_along(w + lat_axis * offset, out));
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
        // Space the pins at the MATERIAL pitch, not the drawn one. The links are rigid — the loop
        // is exactly `geom.pitch · count` long — so the conformed polyline is read as a uniform-
        // strain image of it and sampled in material arc-length ([`station_params`]). Resampling at
        // the naive `polyline_len / count` spaced pins at the ~0.08%-off DRAWN pitch, which walked
        // the belt out from under the sprocket tooth lock at one tooth per ~160 m.
        let (spacing, offset) = station_params(
            phase.get(side),
            geom.pitch,
            polyline_len(&loop_pts),
            pin_belt.count,
        );
        let mut joints = resample(&loop_pts, spacing, offset);
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

/// Resample spacing and phase offset that place the drawn pin stations exactly `material_pitch`
/// apart along a conformed loop — the pin spacing the sprocket phase lock ([`super::wheel_view`])
/// assumes, and the one that keeps the drawn belt registered to the teeth over any travel.
///
/// The links are RIGID: the material loop is exactly `material_pitch · count`. The conformed
/// polyline only approximates it — arc and sag polyline discretisation leave `poly_len` ~0.08% off
/// — so resampling at the naive `poly_len / count` spaces the pins at the DRAWN pitch, and the
/// drawn belt walks out from under the material-pitch tooth lock at ~one tooth per 160 m.
///
/// The cure is a uniform-strain reparametrisation: treat the polyline as the drawn image of the
/// material loop (`strain = poly_len / (material_pitch · count)` drawn metres per material metre)
/// and sample at material positions `offset_m + i · material_pitch` mapped back through the strain.
/// Pin spacing is then the material pitch to float precision, pin 0 advances exactly one station
/// per material pitch of travel (so the sprocket's one-tooth-per-pitch lock never drifts), and the
/// loop closes: `phase += count · material_pitch` returns `offset_m`, and every station, to itself.
fn station_params(phase: f64, material_pitch: f32, poly_len: f32, count: usize) -> (f32, f32) {
    let material_len = material_pitch * count.max(1) as f32;
    let strain = if material_len > 1e-6 {
        poly_len / material_len
    } else {
        1.0
    };
    let (_, offset_m) = phase_decompose(phase, material_pitch);
    (material_pitch * strain, offset_m * strain)
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

/// The `9` viz layer: the collocation stations at the **physics** ring — the CONTACT ENVELOPE
/// itself, pins + mids pushed to the measured outer face PLUS the local free travel (the same
/// per-position [`TravelField`] the live law probes from: full droop over the road-wheel span,
/// tapering to zero at the unsprung sprocket/idler). Grey when clear of terrain, orange when
/// penetrating — the whole oracle, visible, at the law's ACTUAL probe points.
pub(super) fn draw_sample_points(
    mut gizmos: Gizmos,
    viz: Res<VizLayers>,
    hull: Single<&GlobalTransform, With<Hull>>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    field: Res<TerrainField>,
    geom: Res<RigGeom>,
    envelope: Res<EnvelopeLaw>,
) {
    if !viz.casts {
        return;
    }
    let affine = hull.affine();
    // Hull lateral axis: column offsets are hull-x measurements (per-side signed shoe faces).
    let lat_axis = affine.transform_vector3(Vec3::X);
    for side in Side::ALL {
        let cols = geom.grip_column_offsets(side);
        let track_x = side.plane_x(geom.plane_x);
        let mut loop_pts = belt_loop(geom.rest.get(side));
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
            // The live law's datum, exactly: measured pin→outer face plus the local travel,
            // gated to ground-facing segments like the law (the return run has no band).
            let grounded = out2.y < 0.0;
            let travel = |z: f32| -> f32 {
                if grounded {
                    crate::track::forces::TravelField {
                        knots: envelope.knots.get(side),
                    }
                    .at(z)
                } else {
                    0.0
                }
            };
            let face_a = out * (geom.model.pin_to_outer + travel(a.x));
            let face_m = out * (geom.model.pin_to_outer + travel((a.x + b.x) / 2.0));
            let face_b = out * (geom.model.pin_to_outer + travel(b.x));
            for (offset, _) in cols {
                let shift = lat_axis * offset;
                let (ca, cb) = (wa + shift, wb + shift);
                for s in [ca + face_a, (ca + cb) / 2.0 + face_m, cb + face_b] {
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

#[cfg(test)]
mod tests {
    use super::super::rig_geom::{RigGeom, tiger_rig};
    use super::super::wheel_view::tooth_tip_angle;
    use super::*;

    /// Signed representative of `angle` within one `period`, nearest zero — the same `fold` the
    /// sprocket phase tests use to ask "how far off a tooth, and which way".
    fn fold(angle: f32, period: f32) -> f32 {
        (angle + period / 2.0).rem_euclid(period) - period / 2.0
    }

    /// The flat-ground conformed loop of a side: rest circles (road wheels un-articulated), taut
    /// envelope + sag budgeted to the material length — the same construction family
    /// [`conform_belts_field`] runs, so its polyline carries the same discretisation strain the fix
    /// corrects. Closed (first == last), ready for [`resample`].
    fn flat_loop(geom: &RigGeom, side: Side) -> Vec<Vec2> {
        build_route(geom.rest.get(side), geom.belt_len()).pts
    }

    /// The contact-envelope model's one non-negotiable invariant, on the REAL shipped Tiger:
    /// with the spring's free length at the droop envelope and the rate calibrated by the law's
    /// own area integral, the tank parked at its authored rest height on flat ground carries
    /// exactly its declared weight — "green compresses to exactly orange", by construction, so
    /// the authored pose IS the flat-ground equilibrium. Also pins the calibrated travel to the
    /// chain-clamped droop and the derived rate's order of magnitude (a ride-frequency spring,
    /// orders of magnitude below a near-rigid meganewton pad).
    #[test]
    fn the_calibrated_envelope_holds_the_tiger_at_its_authored_rest_pose() {
        use super::super::derive::{self, SuspensionParams};

        let geom = tiger_rig();
        let sus = SuspensionParams::default();
        let spec = super::super::rig_geom::tiger_spec();
        let weight_n = spec.mass * derive::G;
        let engage = spec.track.suspension.engage;
        let law = calibrate_envelope(&geom, &sus, weight_n, engage);

        let droop = geom.droop_travel(&sus);
        assert!(
            (law.free_travel - droop.effective.max(0.01)).abs() < 1e-6,
            "calibrated travel {} must be the chain-clamped droop {}",
            law.free_travel,
            droop.effective,
        );

        // Park the hull at the authored rest height and total the elastic load the calibrated
        // law reports — the equilibrium claim, verified through the live contact pass.
        let params = ForceParams {
            face_offset: geom.model.pin_to_outer,
            free_travel: law.free_travel,
            support_stiffness_per_m: law.stiffness_per_m,
            support_damping_per_m: law.damping_per_m,
            engage_depth: engage,
            probe_reach: CONTACT_PROBE,
            mu: MU,
            slip_saturation: SLIP_SATURATION,
            max_speed: 1.0,
            engine_power: 0.0,
            engine_force: 0.0,
            governor_gain: 0.0,
            inertia: 1.0,
            grip_stiffness: 0.0,
        };
        let affine = bevy::math::Affine3A::from_translation(Vec3::new(0.0, geom.hull_rest_y, 0.0));
        let mut total = 0.0;
        for side in Side::ALL {
            let mut loop_pts = belt_loop(geom.rest.get(side));
            if let Some(&first) = loop_pts.first() {
                loop_pts.push(first);
            }
            let input = SideInput {
                loop_pts: &loop_pts,
                count: geom.link_count,
                plane_x: side.plane_x(geom.plane_x),
                columns: geom.grip_column_offsets(side),
                command: 0.0,
                travel: Some(crate::track::forces::TravelField {
                    knots: law.knots.get(side),
                }),
            };
            let (report, live) = contact_side(
                &input,
                SideState::default(),
                affine,
                1.0 / 64.0,
                &params,
                &crate::track::envelope::FlatRest,
                |_| Vec3::ZERO,
                &mut crate::track::forces::GripElements::for_links(geom.link_count),
            );
            assert!(live);
            total += report.contacts.iter().map(|c| c.load_elastic).sum::<f32>();
        }
        assert!(
            (total - weight_n).abs() / weight_n < 1e-3,
            "parked at the authored pose the envelope must carry the weight: {total:.0} N vs \
             {weight_n:.0} N",
        );

        // The derived rate is a ride-frequency spring: with its full droop travel it must sit
        // orders of magnitude below a near-rigid meganewton contact pad.
        assert!(
            law.stiffness_per_m > 0.0 && law.stiffness_per_m < 1.0e6,
            "envelope stiffness {} N/m per m should sit far below a near-rigid pad's",
            law.stiffness_per_m,
        );
    }

    /// The residual the bug is about, measured on the shipped Tiger: the conformed polyline is a
    /// fraction of a percent off the immutable material length `pitch · link_count`. Printed with
    /// the drift distance it implies so the number is on the record, asserted only loosely (it is a
    /// discretisation artefact, not a contract) to catch a gross regression in the loop builder.
    #[test]
    fn the_conformed_polyline_differs_from_the_material_length_by_a_fraction_of_a_percent() {
        let geom = tiger_rig();
        for side in [Side::Left, Side::Right] {
            let poly = polyline_len(&flat_loop(&geom, side));
            let material = geom.belt_len();
            let strain = poly / material;
            println!(
                "{side:?}: conformed polyline {poly:.6} m vs material {material:.6} m — \
                 strain {strain:.6} ({:+.4}%), naive resampling drifts one tooth per {:.0} m",
                (strain - 1.0) * 100.0,
                geom.pitch / (strain - 1.0).abs(),
            );
            assert!(
                (strain - 1.0).abs() < 0.005,
                "{side:?} polyline strain {strain} is implausible for the loop builder",
            );
        }
    }

    /// THE FIX: the wrap view spaces its pins at the MATERIAL pitch and keeps them seated in the
    /// sprocket's gullets over a whole session of driving. Reuses the sprocket machinery
    /// ([`tooth_tip_angle`] + the "a tip bisects the pin pair" rule of
    /// `wheel_view::a_pin_lands_in_a_gullet_at_every_phase`) against the pins THIS module actually
    /// draws — the `station_params` reparametrisation feeding the real `resample`.
    #[test]
    fn the_wrap_view_keeps_its_pins_seated_over_three_hundred_metres() {
        let geom = tiger_rig();
        let tooth = std::f32::consts::TAU / geom.teeth as f32;
        for side in [Side::Left, Side::Right] {
            let loop_pts = flat_loop(&geom, side);
            let poly = polyline_len(&loop_pts);
            let strain = poly / geom.belt_len();
            let origin = geom.belt_origin_angle(side);
            let steps = 20_000;
            // Metres of travel — a session, well past the ~160 m the bug drifts a whole tooth in.
            let total = 350.0_f64;
            for k in 0..=steps {
                let travel = total * k as f64 / steps as f64;
                let (spacing, offset) = station_params(travel, geom.pitch, poly, geom.link_count);

                // Pin spacing IS the material pitch (to float precision) and the loop closes: the
                // `link_count` stations at `spacing` exactly tile the polyline, so there is no
                // stretched seam at pin 0.
                assert!(
                    (spacing / strain - geom.pitch).abs() < geom.pitch * 1e-5,
                    "{side:?}: drawn pin spacing {} m is not the material pitch",
                    spacing / strain,
                );
                assert!(
                    (spacing * geom.link_count as f32 - poly).abs() < poly * 1e-5,
                    "{side:?}: stations do not close the loop — a seam gap at pin 0",
                );

                // Registration: `resample` lands station 0 at arc `offset`, so its material-link
                // phase is `offset / spacing`. The drawn pin nearest the sprocket-tangent origin
                // therefore sits at `origin + phase·tooth`; the tooth tip is at `tooth_tip_angle`.
                // A tip bisects the pin pair straddling the origin, so the offset is exactly half a
                // tooth — the pin seats in a gullet — at EVERY travel, with no drift.
                let drawn_link_phase = offset / spacing;
                let pin = origin + drawn_link_phase * tooth;
                let tip = tooth_tip_angle(travel, geom.pitch, geom.teeth, origin);
                let seat = fold(tip - pin, tooth).abs();
                assert!(
                    (seat - tooth / 2.0).abs() < 3e-4,
                    "{side:?} at {travel:.1} m ({:.0} links): the drawn pin sits {:.4}° off a tip, \
                     must be {:.4}° (half a tooth) to seat in a gullet",
                    travel / f64::from(geom.pitch),
                    seat.to_degrees(),
                    tooth.to_degrees() / 2.0,
                );
            }
        }
    }

    /// The receipt for the bug the fix retires: resampling at the NAIVE drawn pitch
    /// (`polyline_len / count`) walks the drawn pins off the material-pitch tooth lock — within the
    /// same 350 m the fixed view holds, a pin that must sit in a gullet ends up on a TOOTH (a
    /// visible mis-mesh), and the crossover happens near the `pitch / (strain − 1)` drift distance.
    #[test]
    fn the_naive_drawn_pitch_would_drift_a_pin_onto_a_tooth() {
        let geom = tiger_rig();
        let tooth = std::f32::consts::TAU / geom.teeth as f32;
        for side in [Side::Left, Side::Right] {
            let loop_pts = flat_loop(&geom, side);
            let poly = polyline_len(&loop_pts);
            let strain = poly / geom.belt_len();
            let origin = geom.belt_origin_angle(side);
            let naive_pitch = poly / geom.link_count.max(1) as f32; // the retired spacing
            let drift_distance = geom.pitch / (strain - 1.0).abs(); // one tooth of drift, metres

            let mut worst = 0.0_f32; // largest departure from a seated half-tooth
            let steps = 20_000;
            let total = 350.0_f64;
            for k in 0..=steps {
                let travel = total * k as f64 / steps as f64;
                // The pre-fix parametrisation: offset wraps mod the DRAWN pitch, not the material.
                let (_, offset) = phase_decompose(travel, naive_pitch);
                let drawn_link_phase = offset / naive_pitch;
                let pin = origin + drawn_link_phase * tooth;
                let tip = tooth_tip_angle(travel, geom.pitch, geom.teeth, origin);
                let seat = fold(tip - pin, tooth).abs();
                worst = worst.max((seat - tooth / 2.0).abs());
            }
            // A full mis-seat is half a tooth of departure; the naive scheme reaches most of it.
            assert!(
                worst > 0.4 * tooth,
                "{side:?}: the naive pitch drifted only {:.4}° — the bug should mis-seat a pin",
                worst.to_degrees(),
            );
            assert!(
                drift_distance < total as f32,
                "{side:?}: expected a tooth of drift within {total} m (drift {drift_distance:.0} m)",
            );
        }
    }
}
