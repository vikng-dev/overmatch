//! Field-belt model: an advected pin-line chain with terrain contact read from a **deterministic
//! analytic field** instead of narrow-phase queries — the promoted model the sandbox hosts.
//!
//! The terrain oracle is a rounded-box SDF union over the course's authored blocks
//! ([`TerrainField`], filled by `spawn_environment`). Per link, penetration is evaluated at
//! **fixed link-local collocation stations** (the two pins + the midpoint, on the outer face) and
//! fed to a closed-form pressure profile. There is no witness point, no tie-breaking, and no
//! collision engine anywhere in the course's loop: depth is a pure fixed-order arithmetic function
//! of pose — pose-continuous (C0) and bit-deterministic by construction (the contact-oracle
//! research verdict; see `.agents/docs/design/track-model/contact-oracle-research.md`). The rubble
//! yard's wall and pieces are avian colliders, folded on only where they are near the rig
//! ([`crate::track::rubble`]).
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
//! The sandbox's track view is the memory-enabled kinematic wrap ([`crate::track::wrap`]) — the
//! SAME module the game's `track::view` runs, and the only track view there is: the road wheels
//! read the field directly ([`articulate_wheels_field`]), then the belt path is *fitted* around the
//! articulated wheels every frame ([`conform_belts_field`]) — tangent wrap + terrain conform +
//! budgeted sag, over a self-healing filter memory.

use super::*;

use crate::track::forces::{
    ForceParams, SideInput, SideReport, SideState, contact_side, phase_decompose,
};
use crate::track::oracle::DepthOracle;
use crate::track::rubble::{PieceData, RubbleBodies, RubbleQuery};
use crate::track::transmission::{self, TransmissionInput, TransmissionParams};
use crate::track::wheels::{
    WheelParams, wheel_lift_rise_omega, wheel_lift_step, wheel_lift_target,
};
use crate::track::wrap;

/// The sandbox's terrain resource: the track core's [`BlockField`](crate::track::oracle::BlockField), probed at [`contact_probe`]
/// reach by every call site.
pub(super) use crate::track::terrain::TrackField as TerrainField;

/// Each side's **total** belt travel (m) along the reference loop — the core's advected state
/// ([`SideState::phase`]), committed back verbatim each tick. `f64` exactly like the shared
/// core (and the game's `TrackDrive`): unbounded travel accumulates past f32 ULP within
/// minutes. Kept unwrapped: consumers wrap mod the link pitch for the sampling offset.
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
/// Slack is simply what the authored link count leaves over, reported by `RigGeom::window`.
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

/// The belt contact — an advected pin-line ring, penetration from the field at three fixed
/// stations per link (pin a, midpoint, pin b — on the outer face):
///
/// - the two-piece linear profile between the stations interpolates the interior instead of
///   searching it, so there is nothing to tie-break;
/// - stations are signed (clearance below zero), so the profile's closed-form clipping still
///   finds the lift-off point between stations;
/// - support + total tangent force applied at the profile centroid on the struck surface:
///   the pin line plus `out * (pin_to_outer + travel_c - depth_c)`, using the measured outer-face
///   offset, local suspension travel and struck-surface depth, so the lever arm includes the shoe.
#[expect(
    clippy::too_many_arguments,
    reason = "the sandbox's belt contact: hull, field, drive input, belt state, grip, transmission and rig"
)]
pub(super) fn apply_belt_support_field(
    // Tick-truth Position/Rotation, NEVER GlobalTransform: the render pose updates once per
    // FRAME, so on a multi-tick frame the second tick would probe terrain against a stale
    // hull — phantom slip/penetration that the grip state INTEGRATES into real force
    // oscillation (MEASURED: period-2 load alternation, 212↔32 kN, with a smooth hull).
    mut hull: Query<
        (
            Entity,
            &avian3d::prelude::Position,
            &avian3d::prelude::Rotation,
            (
                Forces,
                &avian3d::prelude::ComputedMass,
                &avian3d::prelude::ComputedAngularInertia,
                &avian3d::prelude::ComputedCenterOfMass,
            ),
            &'static mut crate::track::sim::TrackDrive,
        ),
        With<Hull>,
    >,
    field: Res<TerrainField>,
    raw: Res<RawDriveInput>,
    mut shaped: ResMut<ShapedDrive>,
    (time, gravity): (Res<Time>, Res<avian3d::prelude::Gravity>),
    mut belt: ResMut<BeltSpeed>,
    mut phase: ResMut<BeltPhase>,
    mut contacts: ResMut<BeltContacts>,
    mut dynamics: ResMut<SideDynamics>,
    mut grip: ResMut<BeltGrip>,
    mut grip_elements: ResMut<BeltGripElements>,
    // The transmission's shared parameters stay grouped.
    transmission_params: (
        Res<RigTransmission>,
        ResMut<TransState>,
        ResMut<TransTelemetry>,
    ),
    rig: (Res<RigGeom>, Res<RigSpec>, Res<EnvelopeLaw>),
    rubble: RubbleQuery,
    hard_contacts: (
        Res<crate::contact_transfer::ContactTransfers>,
        Query<(
            &crate::track::stop_contacts::BeltFace,
            &avian3d::prelude::ColliderOf,
        )>,
    ),
    // Every body but the hull, read for the support snapshot.
    (bodies, mut touches): (
        Query<PieceData, Without<Hull>>,
        ResMut<super::rubble::RubbleTouches>,
    ),
) {
    let (transmission, mut trans_state, mut telemetry) = transmission_params;
    let (geom, rig_spec, envelope) = rig;
    let (hull_entity, hull_pos, hull_rot, (mut forces, mass, inertia, com), mut motion) = hull
        .single_mut()
        .expect("RigGeom is published with exactly one sandbox hull in the same command flush");
    let affine = bevy::math::Affine3A::from_rotation_translation(hull_rot.0, hull_pos.0);
    // The game adapter's hull body (`track::sim::apply_track_forces`).
    let hull_body = crate::track::sim::HullBody {
        linear: forces.linear_velocity(),
        angular: forces.angular_velocity(),
        center_of_mass: hull_pos.0 + hull_rot.0 * com.0,
        inverse_mass: mass.inverse(),
        inverse_inertia: inertia.rotated(hull_rot.0).inverse().to_mat3(),
        gravity: gravity.0,
    };
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
    let construction = transmission::DriveConstruction::new(rig_spec.grip, rig_spec.belt_inertia);
    // The piece query cannot read the hull (the hull query holds its `Forces`), so the hull's own
    // colliders never enter the gather; the identity rule is the self-support exclusion.
    let near: Vec<Entity> = rubble
        .near(
            hull_pos.0,
            super::rubble::footprint_sphere(&geom, &envelope),
        )
        .into_iter()
        .filter(|&collider| rubble.body_of(collider) != hull_entity)
        .collect();
    let mut pieces = RubbleBodies::gather(&rubble, &near, &bodies);
    pieces.select_source(hull_entity, &near);
    let oracle = field
        .field
        .as_ref()
        .expect("the sandbox publishes its field before its rig");

    // Phase 1 — both sides' contact passes at their pre-tick belt speeds (transmission-design
    // §2 scheduling: the joint drivetrain needs both reactions before either belt
    // integrates). Force application stays all-left-then-all-right below; within a tick
    // application never feeds back into the velocity field, so evaluating R's contacts
    // before applying L's forces is exact.
    let mut reports: [SideReport; 2] = [SideReport::default(), SideReport::default()];
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
            count: crate::track::forces::CONTACT_RESOLUTION.stations,
            plane_x: geom.side_plane_x(side),
            columns: crate::track::forces::LateralColumns::from(geom.grip_column_offsets(side)),
            // The envelope's per-position reach profile: full droop over the road-wheel span,
            // tapering to zero at the unsprung sprocket/idler centres.
            travel: crate::track::forces::TravelField {
                knots: &envelope.knots.get(side)[..],
            },
        };
        let state = SideState {
            speed: belt.get(side),
            phase: phase.get(side),
            grip: *grip.0.get(side),
        };
        let elements = grip_elements.0.get_mut(side);
        let report = contact_side(
            &side_input,
            state,
            affine,
            dt,
            &params,
            oracle,
            &pieces,
            |p| hull_body.velocity_at(p),
            elements,
        );
        reports[side.index()] = report;
    }

    let (transfers, faces) = hard_contacts;
    let owners = crate::track::stop_contacts::by_owner(&transfers, &faces);
    let hard_reactions = crate::track::stop_contacts::reactions(
        owners.get(&hull_entity).map_or(&[], Vec::as_slice),
        transfers.dt(),
        hull_pos.0,
        hull_rot.0,
        &faces,
    );
    let sensed = crate::track::sim::feel_tick(
        &mut trans_state.1,
        &mut reports,
        hard_reactions,
        &hull_body,
        affine,
        dt,
    );
    let (reactions, reaction_slopes) =
        crate::track::sim::belt_reactions(&reports, &hull_body, hull_pos.0, dt);

    // Phase 2 — ONE joint drivetrain solve on the vehicle's declared tables.
    let tr = transmission::step(
        &construction,
        &transmission.0,
        &mut trans_state.0,
        &TransmissionInput {
            supply: transmission::DriveSupply::WHOLE,
            intent: crate::track::drive::DriveIntent::Axes(raw.0),
            throttle: shaped.0.throttle,
            steer: shaped.0.steer,
            side_commands,
            speeds: [belt.get(Side::Left), belt.get(Side::Right)],
            reactions,
            reaction_slopes,
            sensed,
            dt,
        },
    );
    if tr.shifting {
        trans_state.1.cut();
    }
    telemetry.0 = Some(tr);

    // Phase 3 — apply forces in per-side report order, commit the state.
    touches.0.clear();
    for (si, report) in reports.into_iter().enumerate() {
        let side = Side::ALL[si];
        // Apply in report order — accumulation order is part of bit-reproducibility.
        for app in &report.apps {
            forces.apply_force_at_point(app.force, app.point);
        }
        for c in &report.contacts {
            if let Some(collider) = c.collider {
                touches.0.push(super::rubble::Touch {
                    point: c.point,
                    body: rubble.body_of(collider),
                });
            }
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
                capacity_n: c.mu * c.load_elastic,
                tangential_force: c.tangential_force(),
            });
        }
        *dynamics.engine.get_mut(side) = tr.forces[si];
        *dynamics.reaction.get_mut(side) = report.belt_reaction;
        *grip.0.get_mut(side) = report.state.grip;
        // Phase advects at the PRE-update speed (the belt advects before it re-integrates).
        let pre = belt.get(side);
        let advected = phase.get(side) + f64::from(pre * dt);
        let speed = if hard_reactions[si].is_some() && tr.held[si] {
            0.0
        } else {
            tr.next_speeds[si]
        };
        belt.set(side, speed);
        phase.set(side, advected);
        motion.sides[si] = crate::track::sim::TrackDriveSide {
            speed,
            phase: advected,
        };
    }
}

/// The rig's declared transmission, built in [`build_rig`](super::build_rig) through the
/// same validated seam (`TrackSpec::transmission_params`) as the game.
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
    /// (chain-clamped), admitted only at or above [`crate::track::envelope::MIN_ENVELOPE_TRAVEL`].
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

impl EnvelopeLaw {
    /// The law the rig's belt segments carry against loose pieces.
    pub(super) fn segment_law(&self) -> crate::track::belt_segment::SegmentLaw {
        crate::track::belt_segment::SegmentLaw {
            stiffness_per_m: self.stiffness_per_m,
            damping_per_m: self.damping_per_m,
        }
    }
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
    use crate::track::envelope::{self, EnvelopeSide};

    let travel = geom.droop_travel(sus);
    geom.validate(sus).expect(
        "bake and accepted operator tuning validate the running gear before envelope calibration",
    );
    let mut loops = Side::ALL.map(|side| belt_loop(geom.rest.get(side)));
    for pts in &mut loops {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    let knots = Side::ALL.map(|side| {
        envelope::wheel_travel_knots(
            geom.rest.get(side),
            geom.side_droop_travel(side, sus).effective,
        )
    });
    let sides = Side::ALL.map(|side| EnvelopeSide {
        loop_pts: &loops[side.index()][..],
        plane_x: geom.side_plane_x(side),
        knots: &knots[side.index()][..],
        columns: crate::track::forces::LateralColumns::from(geom.grip_column_offsets(side)),
    });
    let cal = envelope::calibrate(
        &sides,
        crate::track::forces::CONTACT_RESOLUTION.stations,
        geom.model.pin_to_outer,
        engage_depth,
        contact_probe(),
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
        probe_reach: contact_probe(),

        grip: Some(rig.grip),
    }
}

/// The road wheels, placed directly from the terrain FIELD — wheels first, then the belt
/// wraps them (`ground → wheels → belt`, acyclic; a belt-first order is circular). Probe + easing
/// live in [`track::wheels`](crate::track::wheels): implicit critically-damped rise, ballistic
/// fall, deepest of the physics' lateral columns along the lower arc.
pub(super) fn articulate_wheels_field(
    hull: Query<&GlobalTransform, With<Hull>>,
    field: Res<TerrainField>,
    time: Res<Time>,
    geom: Res<RigGeom>,
    suspension: Res<RigSuspension>,
    mut wheels: Query<(
        &RigWheel,
        &super::RoadTread,
        &mut Suspension,
        &mut Transform,
    )>,
) {
    let affine = hull
        .single()
        .expect("RigGeom is published with exactly one sandbox hull in the same command flush")
        .affine();
    let oracle = field
        .field
        .as_ref()
        .expect("the sandbox publishes its field before its rig");
    let down = affine
        .transform_vector3(Vec3::NEG_Y)
        .try_normalize()
        .expect(
            "the sandbox hull pose has a finite nonsingular transform, so its down axis is nonzero",
        );
    for (wheel, tread, mut susp, mut transform) in &mut wheels {
        let params = WheelParams {
            reach: tread.0.radius + geom.thickness,
            ease_omega: wheel_lift_rise_omega(),
            max_lift: suspension.0.bump_stop,
            max_droop: geom.side_droop_travel(wheel.side, &suspension.0).effective,
            lateral_spans: &tread.0.spans,
            probe_reach: contact_probe(),
        };
        let params = &params;
        let target = wheel_lift_target(oracle, &affine, down, susp.pivot_local, params);
        let (mut dy, mut dvel) = (susp.dy, susp.dvel);
        wheel_lift_step(&mut dy, &mut dvel, target, time.delta_secs(), params);
        susp.dy = dy;
        susp.dvel = dvel;
        transform.translation.y = susp.pivot_local.y + susp.dy;
    }
}

/// Per-side live filter memory for the kinematic wrap — the shared [`wrap::WrapState`], indexed
/// `[left, right]`. Mutated ONLY by [`conform_belts_field`]. It is pure FILTER state: it holds no
/// sim truth, cannot tear, and self-heals from the current frame's raw targets on any reset.
#[derive(Resource, Default)]
pub(super) struct WrapMemory {
    states: [wrap::WrapState; 2],
}

impl WrapMemory {
    /// Drop the per-side filter memory — the sandbox's discontinuity paths (`R` reseat, live count
    /// rebuild) call this so the wrap re-inits from the fresh pose instead of settling in from
    /// stale memory, exactly as the game's view resets on a presented-pose snap.
    pub(super) fn reset(&mut self) {
        for state in &mut self.states {
            state.reset();
        }
    }
}

/// Per-frame cost of the track view (µs/frame, cumulative average), written by
/// [`conform_belts_field`] and surfaced in the panel's Telemetry section — the same number the
/// every-512-frames log line prints. Sandbox cost is for ONE tank; the game pays it per rendered
/// tank.
///
/// The field is read only by the `dev_ui` panel, so it reads as dead on the default-feature lane.
#[derive(Resource, Default)]

pub(super) struct ViewPerf {
    /// Kinematic wrap ([`conform_belts_field`]).
    pub(super) wrap_us: f32,
}

/// The track view — the **memory-enabled kinematic wrap**, the one and only track view, run here
/// through the SAME [`wrap`](crate::track::wrap) module the game's `track::view` runs. The path is
/// refitted from scratch every frame as a function of the articulated wheels, the terrain field and
/// the belt phase:
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
/// unrepresentable in the wrap CORE: the envelope+conform+sag geometry has no state to capture,
/// buckle, stale, or diverge. Over it sit the two parameter-free feel tiers (a hull-frame conform
/// ease and a slack spring, always on), whose CLIENT-LOCAL filter memory means the drawn belt
/// carries this client's own history rather than being a pure function of replicated state
/// (ADR-0014) — cosmetic view juice, while the pins/wheels/gear still derive from replicated state
/// alone.
///
/// This system is a thin ADAPTER over [`wrap::step`](crate::track::wrap::step): it gathers the
/// articulated circles off the sandbox rig and writes the drawn joints into the sandbox draw
/// resources. It additionally asks the wrap for the taut REFERENCE loop, which the game never does
/// — that is the sandbox's `-` diagnostic layer, not a view option.
#[expect(
    clippy::too_many_arguments,
    reason = "the sandbox's track view: hull, wheels, field, belt state, wrap memory, perf and the rig"
)]
pub(super) fn conform_belts_field(
    hull: Query<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Suspension, &super::RoadTread)>,
    field: Res<TerrainField>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    time: Res<Time>,
    mut memory: ResMut<WrapMemory>,
    mut belts: ResMut<ConformedBelts>,
    mut reference: ResMut<TautReference>,
    mut view_perf: ResMut<ViewPerf>,
    geom: Res<RigGeom>,
    // Perf probe: (busy seconds, frames) — the standing per-tank view budget measurement.
    mut perf: Local<(f64, u64)>,
) {
    let t_perf = std::time::Instant::now();
    let affine = hull
        .single()
        .expect("RigGeom is published with exactly one sandbox hull in the same command flush")
        .affine();
    // Articulated pin-line circles per side, front→rear: the hull-fixed sprocket + idler from the
    // rig's `rest` list, the road wheels at their pin radius lifted by the cosmetic suspension travel.
    let circles: [Vec<(Vec2, f32)>; 2] = Side::ALL.map(|side| {
        let rest = geom.rest.get(side);
        let mut roads: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _, _)| w.side == side)
            .map(|(_, s, tread)| {
                (
                    Vec2::new(s.pivot_local.z, s.pivot_local.y + s.dy),
                    tread.0.radius + geom.model.pin_to_inner,
                )
            })
            .collect();
        roads.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        let mut c = vec![rest[0]];
        c.extend(roads);
        c.push(*rest.last().expect("a side has a sprocket and an idler"));
        c
    });
    let input = wrap::WrapInput {
        dt: time.delta_secs(),
        affine,
        belt_len: pin_belt.length,
        count: pin_belt.count,
        pitch: geom.pitch,
        thickness: geom.thickness,
        probe_reach: contact_probe(),
        // The sandbox draws the `-` reference layer, so it asks the wrap to build it.
        reference: true,
        sides: Side::ALL.map(|side| wrap::WrapSideInput {
            circles: &circles[side.index()],
            plane_x: geom.side_plane_x(side),
            lateral_stations: geom.grip_stations(side),
            phase: phase.get(side),
        }),
    };
    let oracle = field
        .field
        .as_ref()
        .expect("the sandbox publishes its field before its rig");
    let out = wrap::step(&input, oracle, &mut memory.states);

    for side in Side::ALL {
        let track_x = geom.side_plane_x(side);
        let si = side.index();
        let to_world = |p: &Vec2| affine.transform_point3(Vec3::new(track_x, p.y, p.x));
        let ref_world: Vec<Vec3> = out[si].reference.iter().map(&to_world).collect();
        match side {
            Side::Left => reference.left = ref_world,
            Side::Right => reference.right = ref_world,
        }
        if out[si].joints.len() < 3 {
            continue;
        }
        *belts.get_mut(side) = out[si]
            .joints
            .iter()
            .map(|p| BeltSample {
                local: *p,
                world: to_world(p),
            })
            .collect();
    }
    perf.0 += t_perf.elapsed().as_secs_f64();
    perf.1 += 1;
    // Surface the cumulative average to the panel (the same number the 512-frame log prints).
    view_perf.wrap_us = (perf.0 / perf.1 as f64 * 1e6) as f32;
    if perf.1.is_multiple_of(512) {
        info!(
            "kinematic-wrap perf: {:.0} µs/frame avg ({} frames)",
            perf.0 / perf.1 as f64 * 1e6,
            perf.1
        );
    }
}

/// The `9` viz layer: the collocation stations at the **physics** ring — the CONTACT ENVELOPE
/// itself, pins + mids pushed to the measured outer face PLUS the local free travel (the same
/// per-position [`TravelField`] the live law probes from: full droop over the road-wheel span,
/// tapering to zero at the unsprung sprocket/idler). Grey when clear, orange when penetrating
/// the terrain field, at the law's actual field-probe points. The rubble contact dots show the
/// separate footprint contacts, including those between every probe line.
pub(super) fn draw_sample_points(
    mut gizmos: Gizmos,
    viz: Res<VizLayers>,
    hull: Query<&GlobalTransform, With<Hull>>,
    phase: Res<BeltPhase>,
    field: Res<TerrainField>,
    geom: Res<RigGeom>,
    envelope: Res<EnvelopeLaw>,
) {
    if !viz.casts {
        return;
    }
    let affine = hull
        .single()
        .expect("RigGeom is published with exactly one sandbox hull in the same command flush")
        .affine();
    let oracle = field
        .field
        .as_ref()
        .expect("the sandbox publishes its field before its rig");
    // Hull lateral axis: column offsets are hull-x measurements (per-side signed shoe faces).
    let lat_axis = affine.transform_vector3(Vec3::X);
    for side in Side::ALL {
        let resolution = crate::track::forces::CONTACT_RESOLUTION;
        let cols = crate::track::forces::LateralColumns::from(geom.grip_column_offsets(side));
        let track_x = geom.side_plane_x(side);
        let mut loop_pts = belt_loop(geom.rest.get(side));
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / resolution.stations as f32;
        // Resample offset from the canonical decomposition (wrap count unused here).
        let (_, offset) = phase_decompose(phase.get(side), pitch);
        let mut stations = resample(&loop_pts, pitch, offset);
        stations.truncate(resolution.stations);
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
                continue; // Route joins can repeat a point; there is no segment to sample there.
            }
            let tan2 = seg / len;
            let out2 = Vec2::new(tan2.y, -tan2.x);
            let wa = affine.transform_point3(Vec3::new(track_x, a.y, a.x));
            let wb = affine.transform_point3(Vec3::new(track_x, b.y, b.x));
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .try_normalize().expect("a nonzero segment normal remains nonzero under the hull's nonsingular transform");
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
            for &(offset, _) in cols.samples() {
                let shift = lat_axis * offset;
                let (ca, cb) = (wa + shift, wb + shift);
                for s in [ca + face_a, (ca + cb) / 2.0 + face_m, cb + face_b] {
                    // Orange into the terrain field, grey when clear. Body contacts come from
                    // the footprint pass and have their own contact dots.
                    let depth = oracle.depth_along(s, out, contact_probe());
                    let color = if depth > 0.0 {
                        Color::srgb_from_array(scene().traction_force_srgb.0)
                    } else {
                        Color::srgb_from_array(scene().cast_srgb.0)
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
    use super::*;
    use crate::track::gear_phase::{fold, tooth_tip_angle};

    /// The flat-ground conformed loop of a side: rest circles (road wheels un-articulated), taut
    /// envelope + sag budgeted to the material length — the same construction family
    /// [`conform_belts_field`] runs, so its polyline carries the same discretisation strain the fix
    /// corrects. Closed (first == last), ready for [`resample`].
    fn flat_loop(geom: &RigGeom, side: Side) -> Vec<Vec2> {
        crate::track::route::build_route(geom.rest.get(side), geom.belt_len()).pts
    }

    /// The sandbox calibrates and evaluates at the selected force resolution: at the authored
    /// rest pose its elastic load carries the declared weight. Fixture: the shipped tank.
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
            (law.free_travel
                - droop
                    .effective
                    .max(crate::track::envelope::MIN_ENVELOPE_TRAVEL))
            .abs()
                < 1e-6,
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
            probe_reach: contact_probe(),

            grip: None,
        };
        let affine = bevy::math::Affine3A::from_translation(Vec3::new(0.0, geom.hull_rest_y, 0.0));
        let mut total = 0.0;
        let resolution = crate::track::forces::CONTACT_RESOLUTION;
        for side in Side::ALL {
            let mut loop_pts = belt_loop(geom.rest.get(side));
            if let Some(&first) = loop_pts.first() {
                loop_pts.push(first);
            }
            let input = SideInput {
                loop_pts: &loop_pts,
                count: resolution.stations,
                plane_x: geom.side_plane_x(side),
                columns: crate::track::forces::LateralColumns::from(geom.grip_column_offsets(side)),
                travel: crate::track::forces::TravelField {
                    knots: law.knots.get(side),
                },
            };
            let report = contact_side(
                &input,
                SideState::default(),
                affine,
                1.0 / 64.0,
                &params,
                &crate::track::envelope::FlatRest,
                &RubbleBodies::default(),
                |_| Vec3::ZERO,
                &mut crate::track::forces::GripElements::for_stations(
                    resolution.stations,
                    resolution.lateral_points,
                ),
            );
            total += report.contacts.iter().map(|c| c.load_elastic).sum::<f32>();
        }
        assert!(
            (total - weight_n).abs() / weight_n < 1e-3,
            "parked at the authored pose the envelope must carry the weight: {total:.0} N vs \
             {weight_n:.0} N",
        );
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
                let (spacing, offset) =
                    wrap::station_params(travel, geom.pitch, poly, geom.link_count);

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
}
