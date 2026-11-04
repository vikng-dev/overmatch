//! Belt support, per-element shear and ground reaction for one track side.
//!
//! `contact_side` takes the pose, point velocities, terrain oracle and tick-start piece
//! snapshot; it returns support then total tangent forces and a longitudinal belt reaction.
//! The caller applies those forces, advances the drivetrain, then advects material phase.
//! All loads are N, distances m, velocities m/s; strain belongs to material links and columns.
//!
//! Support uses the struck surface normal. Shear lies in its tangent plane and reads the
//! surface's constitutive facts. Membership and shear capacity use elastic load, so transient
//! support damping cannot erase stationary strain. Each material element advances once even
//! when its footprint crosses several surfaces; slip is load-weighted and compliance is shared.
//!
//! `SideInput::travel` is the droop profile: zero at unsprung ends, full at road wheels.
//! An empty profile is a rest-anchored calibration pad. Construction and simulation share
//! this implementation; the law, alternatives and measured bounds live in
//! `.agents/docs/design/track-model/architecture.md`, section 5a.

use avian3d::prelude::Friction;
use bevy::ecs::entity::Entity;
use bevy::math::{Affine3A, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::load::SupportFact;
#[cfg(test)]
use super::oracle::ContactOracle;
#[cfg(test)]
use super::oracle::DepthOracle;
use super::oracle::{TerrainContact, TerrainOracle};
use super::rubble::{RubbleBodies, Slab, SlabOverlap};

use super::route::{polyline_len, resample};
#[cfg(feature = "bitprobe")]
use crate::bitprobe::{ContactInputProbe, ElementOutputProbe};

/// Effective bristle constitutive data, admitted before contact construction.
/// The measurement history and stability bounds live in ADR-0026.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GripLaw {
    rigid_rolling_resistance: crate::provenance::Labelled<f32>,
    rolling_reference_kpa: crate::provenance::Labelled<f32>,
    breakaway_share: crate::provenance::Labelled<f32>,
}

fn grip_law() -> &'static GripLaw {
    static DATA: std::sync::LazyLock<GripLaw> = std::sync::LazyLock::new(|| {
        const PATH: &str = "assets/driving/grip.ron";
        let data: GripLaw = crate::provenance::from_shipped_str(
            PATH,
            include_str!("../../assets/driving/grip.ron"),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            data.breakaway_share.value.is_finite()
                && data.breakaway_share.value > 0.0
                && data.breakaway_share.value < 1.0
                && matches!(
                    data.breakaway_share.label,
                    crate::provenance::Provenance::StandIn(_)
                ),
            "{PATH}: breakaway_share must lie in (0,1) and be labelled StandIn; author the missing contact presliding curve"
        );
        assert!(
            data.rigid_rolling_resistance.value.is_finite()
                && data.rigid_rolling_resistance.value >= 0.0
                && matches!(
                    data.rigid_rolling_resistance.label,
                    crate::provenance::Provenance::Measured(_)
                        | crate::provenance::Provenance::Derived(_)
                        | crate::provenance::Provenance::Unknown(_)
                        | crate::provenance::Provenance::Fixture(_)
                ),
            "{PATH}: rigid_rolling_resistance must be finite, nonnegative and a fact or Unknown; state the undeforming-support motion resistance"
        );
        assert!(
            data.rolling_reference_kpa.value.is_finite()
                && data.rolling_reference_kpa.value > 0.0
                && matches!(
                    data.rolling_reference_kpa.label,
                    crate::provenance::Provenance::Unit(_)
                ),
            "{PATH}: rolling_reference_kpa must be finite, positive and Unit; declare the common pressure scale"
        );
        data
    });
    &DATA
}

/// Shared running-gear resistance on undeforming support; ground totals already include it.
pub(crate) fn rigid_rolling_resistance() -> f32 {
    grip_law().rigid_rolling_resistance.value
}

/// Common pressure scale for the ground rows, independent of vehicle construction.
pub(crate) fn rolling_reference_kpa() -> f32 {
    grip_law().rolling_reference_kpa.value
}

/// Pressure drag proxy: the hard-support share is pressure-independent; the ground's excess
/// scales with nominal construction pressure. Architecture section 5a owns its limitations.
pub(crate) fn ground_rolling_resistance(reference: f32, nominal_pressure_kpa: f32) -> f32 {
    let hard = rigid_rolling_resistance();
    hard + (reference - hard) * nominal_pressure_kpa / rolling_reference_kpa()
}

/// Resolution floor (m) on shear compliance.
/// DERIVED: ADR-0026's coupled mode has omega*dt about 0.27 at 75 mm and 64 Hz;
/// stiffness varies as 1/K, so 5 mm raises omega*dt to about 1.05, where the coupled
/// experiment rang at half the tick rate. This is a conservative measured-mode floor,
/// not a proof of the sharp coupled-system stability boundary.
/// Architecture section 5a owns the shared compliance and its player-visible recoverable travel.
pub(crate) const GRIP_SHEAR_FLOOR_M: f32 = 0.075;

/// Hull contact-point speed (m/s, relative to support) below which rolling fades to zero.
/// Resolution: regularize the sign change of Coulomb drag in the explicit hull step.
/// DERIVED for uniform support: c*dt/M = rolling*g*dt/band, monotone below one and stable
/// below two. No rolling damper acts directly on belt inertia. At 64 Hz on mud at nominal
/// pressure 107 kPa, rolling is about .1715 and the ratio about .105 (DERIVED); this is no
/// certificate for arbitrary pressure or coupled modes. The fade admits a flat crawl at
/// band*min(engine pull, grip)/rolling_load. With locked belts on uniform support and brakes
/// able to hold them, slopes between atan(mu) and atan(mu + rolling) creep at
/// band*(tan(slope) - mu)/rolling instead of accelerating (DERIVED force balance).
pub const ROLLING_FADE_SPEED: f32 = 0.25;

/// One patch's signed rolling resistance (N), opposing hull travel relative to its support.
/// Coefficient times normal load fades over [`ROLLING_FADE_SPEED`] to zero at rest.
pub fn rolling_resistance(rolling_load: f32, speed: f32) -> f32 {
    rolling_load * (speed / ROLLING_FADE_SPEED).clamp(-1.0, 1.0)
}

/// Track friction and nominal load; the oracle supplies each contact's surface law.
/// Absent from [`ForceParams::grip`], the law runs support only.
#[derive(Clone, Copy, Debug)]
pub struct GripParams {
    /// The track's side of solid-recipient pairs ([`crate::spec::TrackSpec::friction`]).
    pub track: Friction,
    /// Weight over nominal two-track footprint, kPa, derived once from spawn construction.
    pub nominal_pressure_kpa: f32,
    /// Nominal weight on one side (N), half the vehicle's: the scale of the element membership
    /// bounds and the vehicle share of the transmission's shift-landing inertia.
    pub side_weight: f32,
}

impl GripParams {
    /// Solid-recipient track friction, nominal pressure and half-weight from spawn data.
    pub(crate) fn of(spec: &crate::spec::TankSpec, nominal_pressure_kpa: f32) -> Self {
        assert!(
            nominal_pressure_kpa.is_finite() && nominal_pressure_kpa > 0.0,
            "admitted spawn construction supplies positive finite nominal ground pressure"
        );
        grip_law();
        Self {
            track: spec.track.friction(),
            nominal_pressure_kpa,
            side_weight: spec.mass * super::derive::G / 2.0,
        }
    }

    /// The coefficient the belt slides at on a surface carrying `surface`: the one pair rule
    /// (`substances::PAIR_RULE`), combined as avian's narrow phase combines a pair.
    pub fn pair(&self, surface: Friction) -> f32 {
        self.track.combine(surface).dynamic_coefficient
    }
}

/// Resolution: LuGre bristle damping at half critical damping, sized from each patch's
/// initial stiffness and supported mass. ADR-0026 records the coupled stability experiment;
/// this ratio is solver damping, never a material loss factor.
const GRIP_ELEMENT_DAMPING_RATIO: f32 = 0.5;

/// Resolution: element membership bounds, as fractions of the side's nominal weight: a pad ENTERS the
/// grip population above the high bound and STAYS down to the low bound (hysteresis).
/// Membership reads the ELASTIC load only, never the damped load's sign — the damped load
/// carries the support damper's separation transients, and keying reset to it erased a
/// parked tank's strain on one noisy tick (netcode review defect 1: unsafe for parking).
/// The bounds are tiny against a real bearing column (~0.1% / 0.025% of side weight vs the
/// several percent a grounded column carries) — they exclude numeric dust, not load.
///
/// LOW-LOAD POLICY (netcode review defect 3): strain INTEGRATES only while the element is a
/// member — accumulation shares exactly the membership hysteresis, one regime, one set of
/// bounds. A barely-touching pad never clears the enter bound, so it accumulates nothing
/// and later real load finds no stored-up strain to fire as a sudden force; a member that
/// fades into the leave..enter band holds (not grows) its strain. Force always scales by
/// the CURRENT elastic load, so engagement fades continuously in every branch.
const GRIP_ELEMENT_ENTER_FRACTION: f32 = 1e-3;
const GRIP_ELEMENT_LEAVE_FRACTION: f32 = 2.5e-4;

/// Resolution: contact-loss dwell (fixed 64 Hz ticks): a pad whose elastic load falls below the leave
/// bound keeps its stored shear this long before departure is definitive and it forgets —
/// its shear was relieved by ground it no longer touches. 125 ms rides out washboard
/// corner-contact flicker and separation ticks; it is far shorter than a material link's
/// return to the contact patch under drive (one belt loop), and a parked belt never
/// advects, so stale world-space strain is never re-presented to different ground. Chosen
/// over a geometric departure test: one u8 per element, deterministic, and departure
/// geometry is already implied by the load evidence the dwell counts down on.
pub(crate) const GRIP_ELEMENT_LOSS_DWELL_TICKS: u8 = 8;

/// The canonical belt-phase decomposition: travel → (integer wrap count, residual station
/// offset in `[0, pitch)`). ONE function feeds BOTH the resample offset and material
/// identity — computing them separately (f32 residual here, f64 floor there) let rounding
/// disagree for one tick at exact pitch multiples, mis-keying element state by a whole link
/// (netcode review defect 2). The residual comes from the exact f64 remainder; the wrap
/// count is derived FROM that residual (never from an independently rounded division), and
/// the carry branch absorbs the one remaining seam: the f32 cast of a residual just under
/// `pitch` can round up to exactly `pitch`, which `resample`'s internal `rem_euclid` would
/// silently treat as offset 0 — the wrap count must advance with it.
///
/// CONTRACT: pitch is finite and positive, phase is finite, and the whole-pitch quotient fits
/// the signed material-link address. These are checked in every build before integer conversion.
pub fn phase_decompose(phase: f64, pitch: f32) -> (i64, f32) {
    assert!(
        pitch.is_finite() && pitch > 0.0 && phase.is_finite(),
        "phase decomposition requires finite travel and finite positive pitch"
    );
    let p = f64::from(pitch);
    let r = phase.rem_euclid(p);
    let whole = ((phase - r) / p).round();
    assert!(
        whole >= i64::MIN as f64 && whole < -(i64::MIN as f64),
        "the material-link wrap count must fit its signed address before conversion"
    );
    let mut wraps = whole as i64;
    let mut offset = r as f32;
    if offset >= pitch {
        wraps = wraps
            .checked_add(1)
            .expect("the rounded station offset must keep a representable material-link address");
        offset = 0.0;
    }
    (wraps, offset)
}

/// The material link at a station after whole-pitch travel. Wide subtraction keeps the full
/// admitted wrap range addressable; the remainder is a bijection over the nonempty link loop.
pub(crate) fn material_link(station: usize, wraps: i64, count: usize) -> usize {
    assert!(
        count > 0,
        "a material-link address belongs to a nonempty admitted loop"
    );
    (station as i128 - i128::from(wraps)).rem_euclid(count as i128) as usize
}

/// The force model's parameters: vehicle data (spec-authored) + the per-metre support law.
/// Nothing here is solver-quality policy — quality lives in the station/column geometry the
/// caller authors (link pitch sets station density).
#[derive(Clone)]
pub struct ForceParams {
    /// Pin line → outer contact face (m), along the belt's outward normal. The measured
    /// `pin_to_outer` — never `thickness / 2`: the pin does not run mid-plate.
    pub face_offset: f32,
    /// Outer face at rest → the contact envelope (m): the suspension's free travel. The
    /// support spring's zero-force datum sits this far PAST the outer face — the drooped
    /// ("green") envelope, maximum suspension reach — so ground intruding into the
    /// envelope→rest band is what carries the hull, and the authored rest pose is the
    /// flat-ground equilibrium when the stiffness is calibrated to it. The LAW reads the
    /// per-position [`SideInput::travel`] profile (whose road-wheel peak is this value);
    /// this scalar is the calibrated vehicle datum it derives from, kept here as telemetry
    /// (the bitprobe startup digest echoes it).
    pub free_travel: f32,
    /// Support spring (N/m per metre of contacting belt) and damping (N·s/m per metre).
    pub support_stiffness_per_m: f32,
    pub support_damping_per_m: f32,
    /// Soft-engagement ramp depth (m): full support only past this penetration.
    pub engage_depth: f32,
    /// BASE terrain probe reach (m): how far behind the REST face every probe ray starts.
    /// The law extends each probe's reach by its own local free travel, so callers pass the
    /// plain base (no `+ free_travel`) — the ray origin must sit at `face − base` for every
    /// station or zero-travel return-run origins start too deep (buried-origin saturation).
    pub probe_reach: f32,

    /// The traction law's inputs. Ground grip or a solid-recipient material pair sets the
    /// element's isotropic capacity; turning resistance
    /// emerges from footprint geometry, not from an authored lateral share. `None` runs support
    /// only, with no traction or belt hold: the envelope calibration measures penetration area
    /// without traction in the loop.
    pub grip: Option<GripParams>,
}

/// Recoverable shear state, one WORLD-space vector (m) per material link × lateral column
/// (flat index `link * 3 + column`). Force reads this vector divided by contact compliance,
/// capped at that contact's capacity. Sliding returns the state within one shear distance;
/// skid travel beyond it is plastic. Architecture section 5a owns the constitutive decision.
/// Material identity advects with belt phase. Elastic-load hysteresis and contact-loss dwell
/// preserve a parked pad's state through support flicker; definitive departure forgets it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GripElements {
    /// Recoverable bristle deflection, world-space m.
    pub strain: Vec<Vec3>,
    /// Ticks of contact-loss grace left per element before its shear is forgotten. A
    /// parallel `Vec<u8>` rather than a widened strain entry: 1 byte/element with no
    /// padding (a `(Vec3, u8)` entry rounds up to 16 bytes) and the strain slab stays
    /// densely packed on the authority.
    pub dwell: Vec<u8>,
}

impl GripElements {
    /// Both slabs pre-sized for `link_count` material links × 3 lateral columns, at rest
    /// (zero strain, zero dwell) — the ONLY legal construction for a caller entering the
    /// element regime. [`contact_side`] refuses wrong-length slabs in every build;
    /// resizing would erase the strain the contact law carries from tick to tick.
    pub fn for_links(link_count: usize) -> Self {
        let n = link_count * 3;
        Self {
            strain: vec![Vec3::ZERO; n],
            dwell: vec![0; n],
        }
    }
}

/// One side's dynamic state — the caller owns it (the game's `TrackDrive` component, the
/// sandbox's `BeltSpeed`/`BeltPhase` resources).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct SideState {
    /// Belt surface speed (m/s).
    pub speed: f32,
    /// Belt travel (m) — advects the force stations; also the view's scroll phase. `f64`: it
    /// grows unbounded and an f32 loses sub-pitch precision within a long match's driving
    /// distance.
    pub phase: f64,
    /// The side's summed element grip force (N): `x` longitudinal, `y` lateral. Derived
    /// telemetry over the element field (the authoritative state is the caller-owned
    /// [`GripElements`]); ignored on input, overwritten every tick.
    pub grip: Vec2,
}

/// A piecewise-linear free-travel profile over the side-plane longitudinal axis (z): `knots`
/// sorted ascending by z, value clamped to the end knots outside the span. Callers put
/// 0-travel knots at the UNSPRUNG sprocket/idler centres and full-droop knots at the road
/// wheels, so the envelope reaches only where the suspension physically can — the approach
/// runs taper linearly, the wrap arcs beyond the end knots stay unsprung. Top-run stations
/// probe skyward and never see ground, so their lookup value is harmless.
#[derive(Clone, Copy)]
pub struct TravelField<'a> {
    pub knots: &'a [(f32, f32)],
}

impl TravelField<'_> {
    /// The profile value at longitudinal position `z` (piecewise linear, end-clamped).
    pub fn at(&self, z: f32) -> f32 {
        let Some(&(z0, t0)) = self.knots.first() else {
            return 0.0;
        };
        if z <= z0 {
            return t0;
        }
        let mut prev = (z0, t0);
        for &(zk, tk) in &self.knots[1..] {
            if z <= zk {
                let w = zk - prev.0;
                if w <= 0.0 {
                    return tk;
                }
                return prev.1 + (tk - prev.1) * ((z - prev.0) / w);
            }
            prev = (zk, tk);
        }
        prev.1
    }
}

/// One side's per-tick input.
pub struct SideInput<'a> {
    /// The CLOSED pin-line loop polyline (last point == first), side plane (z, y). Rest
    /// geometry — road-wheel articulation is view-only and carries no force.
    pub loop_pts: &'a [Vec2],
    /// Station count (the material link count).
    pub count: usize,
    /// Signed track-centreline x (hull-local).
    pub plane_x: f32,
    /// Lateral collocation columns: (signed hull-x offset from THIS side's pin plane, load
    /// share), ordered inboard → centre → outboard, applied along the hull's lateral axis
    /// (the pin axis — column placement never tilts with segment orientation). Weights sum
    /// to 1: edge columns at the shoe's TRUE lateral faces with Simpson shares (1/6, 2/3,
    /// 1/6) reproduce a laterally-uniform strip's load AND roll moment exactly — the shares
    /// are a statement about the strip's shape, so they survive the shoe's own outboard
    /// bias. Per-side and SIGNED because the shoe is not centred on its pins (the Tiger's
    /// runs ~17 mm outboard: flush with the wheel pack inboard, overhanging outboard) —
    /// `RigGeom::grip_column_offsets(side)` is the measured source; a symmetric
    /// `±width/2` here is the very approximation it replaced.
    pub columns: [(f32, f32); 3],
    /// Per-position free travel (the contact envelope's local reach), REQUIRED: production
    /// callers pass the rig's measured knot profile (`envelope::wheel_travel_knots`); test
    /// fixtures pass uniform knots (empty knots = a rest-anchored zero-droop pad).
    pub travel: TravelField<'a>,
}

/// One force application, in emission order. The caller applies these verbatim — order is
/// part of bit-reproducibility (float accumulation).
pub struct ForceApp {
    pub force: Vec3,
    pub point: Vec3,
    /// The collider the contact presses on, `None` for the terrain field. What the collider's
    /// body takes back follows the recipient law (`track::rubble::piece_impulses`).
    pub collider: Option<Entity>,
}

/// One contact's telemetry (viz / traces). `load` is the ACTUAL damped load applied this
/// tick — `load_elastic` is the spring-only component (static-sink analysis, and the one
/// the element law's friction budget and membership key on).
pub struct BeltContact {
    pub point: Vec3,
    pub load: f32,
    pub load_elastic: f32,
    /// The profile's support rate per metre of probe depth (N/m): the per-metre spring over
    /// the patch's contacting length, at its column's weight, including the engagement ramp's
    /// slope of the spring and closing damper. For a piece's belt-parallel overlap face this
    /// is also its normal stiffness. The recipient's reaction law
    /// reads it as the stiffness the explicit step integrates (`track::rubble::piece_impulses`).
    pub stiffness: f32,
    /// Normal damping rate of this patch (N·s/m), including engagement and column weight.
    pub damping: f32,
    /// Longitudinal slip (belt speed − ground-point speed, m/s).
    pub slip: f32,
    /// Lateral scrub speed at the contact (m/s).
    pub slip_lat: f32,
    /// Scalar friction components along the contact's longitudinal / lateral axes (N).
    pub f_long: f32,
    pub f_lat: f32,
    pub normal: Vec3,
    pub traction: Vec3,
    /// Ground motion resistance on the hull, separate from grip and belt reaction (N).
    pub rolling_force: Vec3,
    /// The belt surface's world velocity at the contact (m/s): the hull's velocity there less
    /// the pre-tick belt speed along the drive direction (the hull's velocity alone where the
    /// contact has no tangent plane).
    pub belt_velocity: Vec3,
    /// The collider under the contact, `None` for the terrain field.
    pub collider: Option<Entity>,
    /// Capacity/load coefficient: direct terrain grip or the solid-recipient material pair
    /// ([`GripParams::pair`]). The piece's reaction is clamped by the same coefficient
    /// (`track::rubble::piece_impulses`). Zero on a support-only rig, which grips nothing.
    pub mu: f32,
}

impl BeltContact {
    /// Total tangent force delivered to the hull and passed to the support's reaction law.
    pub fn tangential_force(&self) -> Vec3 {
        self.traction + self.rolling_force
    }
}

/// What one side's tick produced.
#[derive(Default)]
pub struct SideReport {
    pub state: SideState,
    pub apps: Vec<ForceApp>,
    pub contacts: Vec<BeltContact>,
    /// Longitudinal grip reaction summed into belt dynamics (N): sum of f_long.
    pub belt_reaction: f32,
    /// Terrain/construction facts before dynamic support damping; Driver observation only.
    pub perceived_support: Vec<super::load::SupportFact>,
    /// Material-station depth probes; surface quadrature casts are not recorded.
    #[cfg(feature = "bitprobe")]
    pub(crate) bitprobe_contacts: Vec<ContactInputProbe>,
    /// Per-station/column post-update load, slip, force, strain, and dwell. Measurement builds only.
    #[cfg(feature = "bitprobe")]
    pub(crate) bitprobe_elements: Vec<ElementOutputProbe>,
}

impl SideReport {
    /// Driver geometry is distinct from the piece overlap law's physical orientation.
    /// Empty space inside a coarse overlap cell has no actual surface to observe.
    fn observe_support(
        &mut self,
        (point, normal): (Vec3, Vec3),
        half_span: Vec3,
        length_weight: f32,
        (mu, rolling_coefficient): (f32, f32),
        (collider, out): (Option<Entity>, Vec3),
        pieces: &RubbleBodies,
    ) {
        let (point, normal) = match collider {
            Some(collider) => match pieces.support_face(collider, point, out) {
                Some(face) => face,
                None => return, // The coarse cell's centroid lies in empty space.
            },
            None => (point, normal),
        };
        self.perceived_support.push(SupportFact {
            point,
            normal,
            half_span,
            length_weight,
            mu,
            rolling_coefficient,
        });
    }
}

/// Integrate `max(0, pen(x))` over one linear piece of a pressure profile: `pen` runs
/// `p0 → p1` across `[x0, x1]`. Returns `(∫pen dx, ∫x·pen dx, contacting length)`, clipping
/// the sub-range where the profile is negative (that part of the plate is clear of the
/// ground). Closed form, so the plate's resultant force and centroid are smooth functions of
/// pose — no sampling noise.
pub fn clipped_linear_piece(x0: f32, x1: f32, p0: f32, p1: f32) -> (f32, f32, f32) {
    let w = x1 - x0;
    if w <= 0.0 || (p0 <= 0.0 && p1 <= 0.0) {
        return (0.0, 0.0, 0.0);
    }
    if p0 >= 0.0 && p1 >= 0.0 {
        // Trapezoid: A = w·(p0+p1)/2; M = ∫x·pen dx with pen linear in x.
        let area = w * (p0 + p1) / 2.0;
        let moment = w * (p0 * (2.0 * x0 + x1) + p1 * (x0 + 2.0 * x1)) / 6.0;
        return (area, moment, w);
    }
    // One end negative: clip at the zero crossing and integrate the positive triangle.
    let xc = x0 + w * (p0 / (p0 - p1));
    if p0 > 0.0 {
        clipped_linear_piece(x0, xc, p0, 0.0)
    } else {
        clipped_linear_piece(xc, x1, 0.0, p1)
    }
}

/// Advance one side's contact by one fixed tick: support + traction forces at the advected
/// stations (probing `oracle` at the presented `affine`, reading the hull's velocity field
/// through `vel_at`), contacts, grip-state update, and the summed longitudinal ground reaction.
/// Force application does not feed back into `vel_at` within a tick (velocities integrate
/// later), so reading everything first and applying afterwards is exact. `elements`: the
/// per-element isotropic shear state (see [`GripElements`]) — the caller owns the state vector
/// and must construct it pre-sized to `count * 3` ([`GripElements::for_links`]); wrong-length
/// slabs violate the spawn invariant and panic in every build.
///
/// Returns the report with `state.speed`/`state.phase` still the input values. The caller commits
/// transmission and belt state after both sides report; invalid construction panics.
#[expect(
    clippy::too_many_arguments,
    reason = "one side's contact pass: input, state, pose, clock, params, oracle, velocity field and grip elements"
)]
pub fn contact_side<O: TerrainOracle>(
    input: &SideInput,
    state: SideState,
    affine: Affine3A,
    dt: f32,
    params: &ForceParams,
    oracle: &O,
    pieces: &RubbleBodies,
    vel_at: impl Fn(Vec3) -> Vec3,
    elements: &mut GripElements,
) -> SideReport {
    let mut report = SideReport {
        state,
        ..Default::default()
    };

    assert!(
        input.count >= 3,
        "admitted running gear has enough material links to enclose a belt"
    );
    let pitch = polyline_len(input.loop_pts) / input.count as f32;
    let (wraps, station_offset) = phase_decompose(state.phase, pitch);
    let mut stations = resample(input.loop_pts, pitch, station_offset);
    stations.truncate(input.count);
    assert!(
        stations.len() >= 3,
        "admitted running gear resamples to a closed contact loop; reject degenerate construction before publication"
    );

    // The caller owns fixed-size slabs constructed at spawn by `GripElements::for_links`.
    // A size mismatch is a construction bug; resizing here would erase valid contact strain.
    let element_len = input.count * 3;
    let sized = elements.strain.len() == element_len && elements.dwell.len() == element_len;
    assert!(
        sized,
        "GripElements slabs must be pre-sized to count*3 = {element_len} at construction \
         (strain {}, dwell {}) — runtime resizing erases contact strain",
        elements.strain.len(),
        elements.dwell.len(),
    );
    let elements_mode = params.grip.is_some();

    let (cols, held) = collect_column_contacts(
        input,
        &stations,
        wraps,
        affine,
        params,
        oracle,
        pieces,
        vel_at,
        state.speed,
        elements_mode,
        &mut report,
    );
    let elem_g = match &params.grip {
        Some(grip) => update_grip_elements(
            &cols,
            &held,
            elements,
            grip.side_weight,
            dt,
            element_len,
            &mut report,
        ),
        None => Vec::new(),
    };
    let (elem_sum, belt_reaction) = emit_contact_forces(&mut report, &cols, &elem_g, elements_mode);

    // The grip slot carries the summed element force (long, lat) — telemetry only (the
    // element state is authoritative and lives with the caller). Belt dynamics (the joint
    // transmission) integrate AFTER this pass — `state.speed`/
    // `state.phase` are returned untouched, the pre-tick truth the phase advection and the
    // transmission both key on. Only grip reacts on the belt; rolling acts on the hull.
    report.state.grip = elem_sum;
    report.belt_reaction = belt_reaction;
    report
}

/// One station segment × one lateral column, after support has resolved but before any force
/// is emitted — the intermediate [`contact_side`] pass 1 produces and the element regime and
/// pass 2 consume.
struct ColumnContact {
    p: Vec3,
    normal: Vec3,
    load: f32,
    load_elastic: f32,
    /// [`BeltContact::stiffness`].
    stiffness: f32,
    damping: f32,
    long_dir: Vec3,
    lat_dir: Vec3,
    has_plane: bool,
    slip_long: f32,
    slip_lat: f32,
    /// Flat material-element index (`link * 3 + column`) — the per-element grip key.
    element: usize,
    /// [`BeltContact::belt_velocity`].
    belt_velocity: Vec3,
    /// The surface carrying this patch, `None` for the terrain field.
    collider: Option<Entity>,
    /// [`BeltContact::mu`].
    mu: f32,
    travel_speed: f32,
    /// Motion resistance coefficient: ground total or undeforming-support running-gear loss.
    rolling: f32,
    #[cfg(feature = "bitprobe")]
    probe_index: usize,
}

/// Sample the three-point piecewise-linear profile (`v` at `0`, `len/2`, `len`) at `x` — the
/// same two-half interpolation the pressure profile itself is built from, so the penetration
/// and the travel datum at the centroid are read off ONE shape.
fn sample_halves(x: f32, len: f32, v: [f32; 3]) -> f32 {
    let half = len / 2.0;
    if x <= half {
        v[0] + (v[1] - v[0]) * (x / half)
    } else {
        v[1] + (v[2] - v[1]) * ((x - half) / half)
    }
}

/// A rectangle of one column's strip carried by one surface. Its signed pressure profile has
/// the same pin/mid/pin form as the field's; all surfaces enter the same support law below.
#[derive(Clone, Copy)]
struct SupportPatch {
    origin: Vec3,
    len: f32,
    weight: f32,
    pen: [f32; 3],
    travel: [f32; 3],
    collider: Option<Entity>,
}

/// Resolution: surface quadrature stops below half the field's rounding radius. It confines
/// a curved lip's normal to its pressure share instead of rotating a whole link's load.
const SURFACE_PATCH_LENGTH_M: f32 = super::oracle::FIELD_ROUNDING / 2.0;
/// Resolution: squared unit-normal difference; below this the support direction error is
/// under about 0.6 degrees (DERIVED sqrt(1e-4) radians), avoiding subdivision of planar ground.
const SURFACE_NORMAL_ERROR_SQ: f32 = 1e-4;

fn terrain_surface_patches<O: TerrainOracle>(
    patches: &[SupportPatch],
    axis: Vec3,
    out: Vec3,
    params: &ForceParams,
    oracle: &O,
) -> Vec<SupportPatch> {
    fn split<O: TerrainOracle>(
        patch: SupportPatch,
        axis: Vec3,
        out: Vec3,
        params: &ForceParams,
        oracle: &O,
        result: &mut Vec<SupportPatch>,
    ) {
        let Some((_, _, centroid)) = profile_resultant(patch.len, patch.pen) else {
            return; // A zero-pressure strip carries no surface resultant.
        };
        let hits = [0.0, patch.len / 2.0, patch.len, centroid].map(|x| {
            let travel = sample_halves(x, patch.len, patch.travel);
            oracle.contact_along(
                patch.origin + axis * x + out * (params.face_offset + travel),
                out,
                params.probe_reach + travel,
            )
        });
        let aligned = match hits {
            [
                TerrainContact::Hit { normal: a, .. },
                TerrainContact::Hit { normal: m, .. },
                TerrainContact::Hit { normal: b, .. },
                TerrainContact::Hit { normal: c, depth },
            ] if depth > 0.0 => {
                (*a - *m)
                    .length_squared()
                    .max((*b - *m).length_squared())
                    .max((*c - *m).length_squared())
                    <= SURFACE_NORMAL_ERROR_SQ
            }
            _ => false,
        };
        if aligned || patch.len <= SURFACE_PATCH_LENGTH_M {
            result.push(patch);
            return;
        }
        let half = patch.len / 2.0;
        for start in [0.0, half] {
            let travel = [start, start + half / 2.0, start + half]
                .map(|x| sample_halves(x, patch.len, patch.travel));
            let origin = patch.origin + axis * start;
            let pen = [0.0, half / 2.0, half].map(|x| {
                let t = sample_halves(x, half, travel);
                oracle.depth_along(
                    origin + axis * x + out * (params.face_offset + t),
                    out,
                    params.probe_reach + t,
                )
            });
            split(
                SupportPatch {
                    origin,
                    len: half,
                    pen,
                    travel,
                    ..patch
                },
                axis,
                out,
                params,
                oracle,
                result,
            );
        }
    }
    let mut result = Vec::new();
    for &patch in patches {
        if patch.collider.is_some() || patch.pen.iter().all(|p| *p <= 0.0) {
            result.push(patch);
        } else {
            split(patch, axis, out, params, oracle, &mut result);
        }
    }
    result
}

/// Partition a strip at footprint edges and at field/piece depth crossings. Within each cell
/// only the deepest surface carries the belt, so stacked pieces cannot multiply its support.
/// Column widths are DERIVED from their quadrature shares, retaining the existing field rule
/// while a narrow piece takes exactly its covered fraction of the true shoe width.
fn footprint_patches(
    slab: &Slab,
    strip: (f32, f32),
    field_offset: f32,
    pen: [f32; 3],
    travel: [f32; 3],
    overlaps: &[SlabOverlap],
    pieces: &RubbleBodies,
) -> Vec<SupportPatch> {
    let mut us = vec![0.0, slab.len / 2.0, slab.len];
    let mut ls = vec![strip.0, strip.1];
    for hit in overlaps {
        us.extend([hit.u.0, hit.u.1]);
        ls.extend([
            hit.l.0.clamp(strip.0, strip.1),
            hit.l.1.clamp(strip.0, strip.1),
        ]);
    }
    us.sort_unstable_by(f32::total_cmp);
    us.dedup();
    ls.sort_unstable_by(f32::total_cmp);
    ls.dedup();
    let mut patches = Vec::new();
    for l in ls.windows(2) {
        let lateral = (l[0] + l[1]) / 2.0;
        for u in us.windows(2) {
            let mid = (u[0] + u[1]) / 2.0;
            let deepest = overlaps
                .iter()
                .filter(|hit| {
                    mid > hit.u.0 && mid < hit.u.1 && lateral > hit.l.0 && lateral < hit.l.1
                })
                .fold(None::<&SlabOverlap>, |best, hit| match best {
                    Some(best) if best.depth >= hit.depth => Some(best),
                    _ => Some(hit),
                });
            let field = |x| sample_halves(x, slab.len, pen);
            let local_travel = |x| sample_halves(x, slab.len, travel);
            let mut cuts = vec![u[0], u[1]];
            if let Some(hit) = deepest {
                let a = hit.depth + local_travel(u[0]) - field(u[0]);
                let b = hit.depth + local_travel(u[1]) - field(u[1]);
                if (a < 0.0 && b > 0.0) || (a > 0.0 && b < 0.0) {
                    cuts.insert(1, u[0] + (u[1] - u[0]) * a / (a - b));
                }
            }
            for range in cuts.windows(2) {
                let xs = [range[0], (range[0] + range[1]) / 2.0, range[1]];
                let winner = deepest.filter(|hit| hit.depth + local_travel(xs[1]) > field(xs[1]));
                patches.push(SupportPatch {
                    // A field cell retains its quadrature column's location. Only the covered
                    // piece area moves to its true lateral centroid, so a vanishing footprint
                    // cannot shift the remaining ground's roll lever arm.
                    origin: slab.origin
                        + slab.axis * range[0]
                        + slab.lat
                            * if winner.is_some() {
                                lateral
                            } else {
                                field_offset
                            },
                    len: range[1] - range[0],
                    weight: (l[1] - l[0]) / (slab.lateral.1 - slab.lateral.0),
                    pen: xs.map(|x| match winner {
                        Some(hit) => hit.depth + local_travel(x),
                        None => field(x),
                    }),
                    travel: xs.map(local_travel),
                    collider: winner.map(|hit| pieces.collider(hit.piece)),
                });
            }
        }
    }
    patches
}

/// Pieces that actually replace positive field support in this column strip. A buried or
/// laterally separate piece must not change the remaining field's quadrature or force bits.
fn raised_footprints(
    overlaps: &[SlabOverlap],
    strip: (f32, f32),
    len: f32,
    pen: [f32; 3],
    travel: [f32; 3],
) -> Vec<SlabOverlap> {
    overlaps
        .iter()
        .copied()
        .filter(|hit| {
            hit.l.1 > strip.0
                && hit.l.0 < strip.1
                && [hit.u.0, hit.u.1, (len / 2.0).clamp(hit.u.0, hit.u.1)]
                    .into_iter()
                    .any(|x| {
                        let depth = hit.depth + sample_halves(x, len, travel);
                        depth > 0.0 && depth > sample_halves(x, len, pen)
                    })
        })
        .collect()
}

/// Integrate the two-piece clipped-linear pressure profile `pen` (at `0`, `len/2`, `len`) over
/// one segment. Returns `(∫pen, contacting length, centroid position along the segment)`, or
/// `None` when the profile carries no positive area (the plate is clear of the ground).
fn profile_resultant(len: f32, pen: [f32; 3]) -> Option<(f32, f32, f32)> {
    let (a1, m1, l1) = clipped_linear_piece(0.0, len / 2.0, pen[0], pen[1]);
    let (a2, m2, l2) = clipped_linear_piece(len / 2.0, len, pen[1], pen[2]);
    let (area, moment, contact_len) = (a1 + a2, m1 + m2, l1 + l2);
    if area <= 0.0 {
        return None;
    }
    Some((area, contact_len, moment / area))
}

/// The contact's tangent frame and the slip it sees: the belt's drive direction projected into
/// the contact plane, the lateral axis completing it, and the longitudinal/lateral slip speeds
/// between belt surface and ground point. A segment whose drive direction is (nearly) parallel
/// to the contact normal has no usable plane — the flag says so and every component is zero, so
/// the caller emits support only.
fn contact_frame(
    affine: Affine3A,
    tan2: Vec2,
    normal: Vec3,
    vel: Vec3,
    belt_speed: f32,
) -> (Vec3, Vec3, bool, f32, f32) {
    let drive = -affine.transform_vector3(Vec3::new(0.0, tan2.y, tan2.x));
    let long_plane = drive - drive.dot(normal) * normal;
    let has_plane = long_plane.length() > 1e-4;
    if !has_plane {
        return (Vec3::ZERO, Vec3::ZERO, false, 0.0, 0.0);
    }
    let long_dir = long_plane.normalize();
    let lat_dir = normal.cross(long_dir).normalize_or_zero();
    (
        long_dir,
        lat_dir,
        true,
        belt_speed - vel.dot(long_dir),
        vel.dot(lat_dir),
    )
}

/// Exact identity of a terrain probe: signed zeros and every direction/reach bit are part of
/// the oracle input. Only an identical query may reuse a neighbouring link's endpoint result.
type ProbeKey = ([u32; 3], [u32; 3], u32);

fn probe_key(station: Vec3, out: Vec3, reach: f32) -> ProbeKey {
    (
        station.to_array().map(f32::to_bits),
        out.to_array().map(f32::to_bits),
        reach.to_bits(),
    )
}

/// Adjacent straight links share a pin probe. The oracle is pure, so exactly equal endpoint,
/// direction and reach inputs keep the preceding result, including all of its float bits.
fn probe_depths<O: TerrainOracle>(
    previous: &mut Option<(ProbeKey, f32)>,
    oracle: &O,
    points: [Vec3; 3],
    out: Vec3,
    reach: [f32; 3],
) -> [f32; 3] {
    let key = probe_key(points[0], out, reach[0]);
    let first = match *previous {
        Some((last_key, depth)) if last_key == key => depth,
        _ => oracle.depth_along(points[0], out, reach[0]),
    };
    let depths = [
        first,
        oracle.depth_along(points[1], out, reach[1]),
        oracle.depth_along(points[2], out, reach[2]),
    ];
    *previous = Some((probe_key(points[2], out, reach[2]), depths[2]));
    depths
}

/// Fold one support patch into its element's probe record: the load-weighted point, the loads
/// and the normal. Measurement builds only.
#[cfg(feature = "bitprobe")]
fn probe_patch(
    output: &mut ElementOutputProbe,
    (p, normal): (Vec3, Vec3),
    (load, load_elastic): (f32, f32),
) {
    if output.load == 0.0 {
        output.point = p;
        output.normal = normal;
    } else {
        let share = load / (output.load + load);
        output.point = output.point.lerp(p, share);
        output.normal = output.normal.lerp(normal, share);
    }
    output.load += load;
    output.load_elastic += load_elastic;
}

/// A patch's support load (N): the spring on its profile `area` (m²) and the damper on normal
/// closing speed (m/s) over its contacting `length`, never pulling, at its column's `weight` of
/// the per-metre coefficients and through the engagement ramp `engage`.
fn support_load(
    params: &ForceParams,
    (weight, engage): (f32, f32),
    (area, length): (f32, f32),
    closing: f32,
) -> f32 {
    weight
        * (params.support_stiffness_per_m * area + params.support_damping_per_m * length * closing)
            .max(0.0)
        * engage
}

/// The profile load's derivative under uniform probe-depth displacement, at `closing` m/s. The
/// profile area's derivative is its contacting length; inside soft engagement the ramp scales
/// the spring's force and the damper's, so both contribute its slope. A parting damper's
/// negative slope is left out: the stability bound stays conservative.
fn support_stiffness(
    params: &ForceParams,
    weight: f32,
    (area, length, depth): (f32, f32, f32),
    closing: f32,
) -> f32 {
    let engage = (depth / params.engage_depth).clamp(0.0, 1.0);
    let slope = if depth < params.engage_depth {
        params.engage_depth.recip()
    } else {
        0.0 // Full engagement has no remaining ramp slope.
    };
    let elastic = params.support_stiffness_per_m * (length * engage + area * slope);
    let damper = params.support_damping_per_m * length * closing.max(0.0) * slope;
    weight * (elastic + damper)
}

/// PASS 1 — geometry, support, and slip per contact column. Forces are NOT emitted yet: the
/// elastic grip resultant needs this tick's total budget and load-weighted slip BEFORE
/// per-contact traction can include it. Pass 2 emits apps in the exact support-then-traction
/// per-contact order the one-pass law used — application order is the bit-reproducibility
/// contract, and it is unchanged.
///
/// Returns the resolved contacts plus the membership evidence pass 1 cannot put in them:
/// columns whose damped support load clipped to zero while elastic penetration remains — the
/// pad is still ON the ground (a separating/noisy tick), and element strain must survive it.
/// Station order — deterministic, fixed bound (`count * 3`).
#[expect(
    clippy::too_many_arguments,
    reason = "one side's contact columns: input, stations, wraps, pose, params, oracle, velocity field, belt speed and mode"
)]
fn collect_column_contacts<O: TerrainOracle>(
    input: &SideInput,
    stations: &[Vec2],
    wraps: i64,
    affine: Affine3A,
    params: &ForceParams,
    oracle: &O,
    pieces: &RubbleBodies,
    vel_at: impl Fn(Vec3) -> Vec3,
    belt_speed: f32,
    elements_mode: bool,
    report: &mut SideReport,
) -> (Vec<ColumnContact>, Vec<(usize, f32)>) {
    let n = stations.len();
    let mut cols: Vec<ColumnContact> = Vec::with_capacity(n);
    let mut held: Vec<(usize, f32)> = Vec::new();

    // The support datum: probes cast from the contact envelope (outer face + local free
    // travel), so penetration measures the ground's intrusion into the suspension band, not
    // into the rest face. The travel is position-dependent (unsprung arcs at 0, road-wheel
    // span at full droop).
    let travel_at = |z: f32| -> f32 { input.travel.at(z) };

    // Material identity under advection: station `i` samples arc `offset + i·pitch`, so the
    // material link there is `i − wraps` (mod count) — when the sampling offset wraps, the
    // identity shift absorbs the jump (the witness-link mapping). `wraps` and the offset come
    // from the SAME [`phase_decompose`] call — the pair is consistent by construction.
    let material = |i: usize| material_link(i, wraps, input.count);

    // The pin axis in world space: column offsets are hull-x measurements (shoe faces
    // relative to the pin plane), so they shift along the hull's lateral axis — never along
    // a per-segment cross product, whose sign would depend on the loop's winding.
    let lat_axis = affine.transform_vector3(Vec3::X);
    let lateral = (
        input.columns[0].0.min(input.columns[2].0),
        input.columns[0].0.max(input.columns[2].0),
    );
    let mut previous_end: [Option<(ProbeKey, f32)>; 3] = [None; 3];

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

        let wa = affine.transform_point3(Vec3::new(input.plane_x, a.y, a.x));
        let wb = affine.transform_point3(Vec3::new(input.plane_x, b.y, b.x));
        let out = affine
            .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
            .normalize_or_zero();
        let axis = (wb - wa) / len;
        // Local envelope datum at the three collocation stations (a / mid / b) — the profile
        // varies along the approach runs, so each probe carries its own offset. Ground-facing
        // segments only (hull-local outward normal pointing down): the suspension band exists
        // under the belly, never over the return run.
        let travel = if out2.y < 0.0 {
            [travel_at(a.x), travel_at((a.x + b.x) / 2.0), travel_at(b.x)]
        } else {
            [0.0; 3]
        };
        let face_a = out * (params.face_offset + travel[0]);
        let face_m = out * (params.face_offset + travel[1]);
        let face_b = out * (params.face_offset + travel[2]);

        let slab = Slab {
            origin: wa,
            axis,
            lat: lat_axis,
            out,
            len,
            lateral,
            face: params.face_offset,
            reach: params.probe_reach,
            travel: travel.into_iter().fold(0.0_f32, f32::max),
        };
        let overlaps = pieces.in_slab(&slab);
        let mut share = 0.0;

        // The shoe is sampled as three lateral COLUMNS (measured faces + centre — see
        // [`SideInput::columns`]): each column runs the full profile machinery on its own
        // three stations with its weight of the per-metre coefficients and applies its
        // resultant at its own point — roll torque from a curb under one track edge,
        // cross-slope contact, and half-off-a-ledge support all emerge from the
        // application points.
        for (ci, &(offset, weight)) in input.columns.iter().enumerate() {
            let shift = lat_axis * offset;
            let ca = wa + shift;
            let cb = wb + shift;

            // The three collocation stations, on the outer face; depth along the link's own
            // outward normal (cast semantics).
            // Per-probe reach = base + LOCAL travel, so the ray ORIGIN sits at `face − base`
            // for every station regardless of its travel — the shipped law's origin geometry.
            // A uniform `base + max_travel` reach put zero-travel stations' origins `travel`
            // too deep; on the RETURN run that is downward, and a settle transient dipped the
            // up-facing origins underground — buried-origin saturation then reported full
            // reach of phantom penetration whose load points DOWN (−out), a self-burying
            // feedback (measured: the headless Tiger tail-sunk at 1.46 m of 4 s full
            // throttle).
            let pen = probe_depths(
                &mut previous_end[ci],
                oracle,
                [ca + face_a, (ca + cb) / 2.0 + face_m, cb + face_b],
                out,
                travel.map(|travel| params.probe_reach + travel),
            );
            #[cfg(feature = "bitprobe")]
            let probe_index = {
                let (query_a, query_m, query_b) =
                    (ca + face_a, (ca + cb) / 2.0 + face_m, cb + face_b);
                let material = material(i);
                report.bitprobe_contacts.push(ContactInputProbe {
                    station: i as u32,
                    material: material as u32,
                    column: ci as u32,
                    query_a,
                    query_m,
                    query_b,
                    out,
                    reach: params.probe_reach,
                    depths: pen,
                });
                report.bitprobe_elements.push(ElementOutputProbe {
                    station: i as u32,
                    material: material as u32,
                    column: ci as u32,
                    ..Default::default()
                });
                report.bitprobe_elements.len() - 1
            };
            let strip = if input.columns[0].0 < input.columns[2].0 {
                (
                    lateral.0 + (lateral.1 - lateral.0) * share,
                    lateral.0 + (lateral.1 - lateral.0) * (share + weight),
                )
            } else {
                (
                    lateral.1 - (lateral.1 - lateral.0) * (share + weight),
                    lateral.1 - (lateral.1 - lateral.0) * share,
                )
            };
            share += weight;
            let affected = raised_footprints(&overlaps, strip, len, pen, travel);
            let field_patch = SupportPatch {
                origin: ca,
                len,
                weight,
                pen,
                travel,
                collider: None,
            };
            let split;
            let patches = if affected.is_empty() {
                std::slice::from_ref(&field_patch)
            } else {
                split = footprint_patches(&slab, strip, offset, pen, travel, &affected, pieces);
                &split
            };
            for patch in terrain_surface_patches(patches, axis, out, params, oracle) {
                let SupportPatch {
                    len,
                    weight,
                    pen,
                    collider,
                    ..
                } = patch;
                let pen_max = pen[0].max(pen[1]).max(pen[2]);
                if pen_max <= 0.0 {
                    continue;
                }

                let Some((area, contact_len, x_c)) = profile_resultant(len, pen) else {
                    continue;
                };
                let Some((p, normal)) = patch_contact(patch, axis, out, x_c, params, oracle) else {
                    // The loaded profile bridges a gap or a buried probe; neither is a surface.
                    continue;
                };
                let hull_vel = vel_at(p);
                let vel = match collider {
                    Some(collider) => hull_vel - pieces.velocity_at(collider, p),
                    None => hull_vel,
                };
                let closing = -normal.dot(vel);
                let engage = (pen_max / params.engage_depth).clamp(0.0, 1.0);
                let load_elastic = weight * params.support_stiffness_per_m * area * engage;
                let stiffness =
                    support_stiffness(params, weight, (area, contact_len, pen_max), closing);
                let load = support_load(params, (weight, engage), (area, contact_len), closing);
                #[cfg(feature = "bitprobe")]
                probe_patch(
                    &mut report.bitprobe_elements[probe_index],
                    (p, normal),
                    (load, load_elastic),
                );
                let surface = surface_under(params.grip, collider, p, oracle, pieces);
                report.observe_support(
                    (p, normal),
                    axis * (contact_len / 2.0),
                    weight * contact_len,
                    (surface.mu, surface.rolling),
                    (collider, out),
                    pieces,
                );
                if load <= 0.0 {
                    if elements_mode && load_elastic > 0.0 {
                        held.push((material(i) * 3 + ci, load_elastic));
                    }
                    continue;
                }

                let (long_dir, lat_dir, has_plane, slip_long, slip_lat) =
                    contact_frame(affine, tan2, normal, vel, belt_speed);
                let belt_velocity = hull_vel - long_dir * belt_speed;
                cols.push(ColumnContact {
                    p,
                    normal,
                    load,
                    load_elastic,
                    stiffness,
                    damping: weight * params.support_damping_per_m * contact_len * engage,
                    long_dir,
                    lat_dir,
                    has_plane,
                    slip_long,
                    slip_lat,
                    element: material(i) * 3 + ci,
                    belt_velocity,
                    collider,
                    mu: surface.mu,
                    travel_speed: vel.dot(long_dir),
                    rolling: surface.rolling,
                    #[cfg(feature = "bitprobe")]
                    probe_index,
                });
            }
        }
    }
    (cols, held)
}

/// The pressure centroid's own struck surface. A clearance hit cannot carry the load
/// sampled on neighbouring lips; piece overlap orientation remains the documented stand-in.
fn patch_contact<O: TerrainOracle>(
    patch: SupportPatch,
    axis: Vec3,
    out: Vec3,
    centroid: f32,
    params: &ForceParams,
    oracle: &O,
) -> Option<(Vec3, Vec3)> {
    let travel = sample_halves(centroid, patch.len, patch.travel);
    let query = patch.origin + axis * centroid + out * (params.face_offset + travel);
    match patch.collider {
        Some(_) => Some((
            query - out * sample_halves(centroid, patch.len, patch.pen).max(0.0),
            -out,
        )),
        None => match oracle.contact_along(query, out, params.probe_reach + travel) {
            TerrainContact::Hit { depth, normal } if depth > 0.0 => {
                Some((query - out * depth, *normal))
            }
            TerrainContact::Hit { .. } | TerrainContact::Miss | TerrainContact::Buried => None,
        },
    }
}

/// Contact capacity and the Driver read the same coefficients, independent of support elasticity.
struct SurfaceGrip {
    mu: f32,
    rolling: f32,
}

fn surface_under<O: TerrainOracle>(
    grip: Option<GripParams>,
    collider: Option<Entity>,
    p: Vec3,
    oracle: &O,
    pieces: &RubbleBodies,
) -> SurfaceGrip {
    match (grip, collider) {
        (None, _) => SurfaceGrip {
            mu: 0.0,
            rolling: 0.0,
        },
        (Some(grip), Some(collider)) => SurfaceGrip {
            mu: grip.pair(pieces.friction(collider)),
            rolling: rigid_rolling_resistance(),
        },
        (Some(grip), None) => match oracle.ground_at(p) {
            Some(ground) => SurfaceGrip {
                mu: ground.grip,
                rolling: ground.rolling_at(grip.nominal_pressure_kpa),
            },
            None => {
                #[cfg(test)]
                {
                    // Fixture: no-field calibration pads have effective grip 0.9 and no rolling load.
                    SurfaceGrip {
                        mu: 0.9,
                        rolling: 0.0,
                    }
                }
                #[cfg(not(test))]
                {
                    panic!(
                        "a force-bearing terrain contact requires its GroundField; publish ground facts with the terrain before simulation"
                    )
                }
            }
        },
    }
}

/// Dupont elastic zone with Dahl return, shared by every surface. Below breakaway and
/// during unloading the state is elastic; plastic flow bounds recoverable shear to
/// [`GRIP_SHEAR_FLOOR_M`]. Each patch projects this vector into its own plane when reading force.
fn advance_element_strain(j0: Vec3, sdot: Vec3, dt: f32) -> Vec3 {
    let shear_m = GRIP_SHEAR_FLOOR_M;
    let speed = sdot.length();
    let alpha = if j0.dot(sdot) < 0.0 {
        0.0
    } else {
        let m = ((j0.length() / shear_m - grip_law().breakaway_share.value)
            / (1.0 - grip_law().breakaway_share.value))
            .clamp(0.0, 1.0);
        m * m * (3.0 - 2.0 * m)
    };
    let mut j1 = (j0 + sdot * dt) / (1.0 + alpha * (dt / shear_m) * speed);
    let j_len = j1.length();
    if j_len > shear_m {
        j1 *= shear_m / j_len;
    }
    j1
}

/// Tick-local accumulation for one material element, shared by all its surface patches.
#[derive(Clone, Copy, Default)]
struct ElementWork {
    load: f32,
    membership_load: f32,
    slip: Vec3,
    count: usize,
    advanced: bool,
    kept: bool,
}

/// Definitive departure only: the dwell rides out contact flicker; when it expires the pad has
/// truly left the ground (cycled to the return run, or lifted off) and forgets — its shear was
/// relieved by ground it no longer touches. `kept` flags the elements this tick found evidence
/// for; every other element counts down.
fn forget_departed_elements(work: &[ElementWork], elements: &mut GripElements) {
    for ((work, dwell), strain) in work
        .iter()
        .zip(elements.dwell.iter_mut())
        .zip(elements.strain.iter_mut())
    {
        if !work.kept {
            let d = dwell.saturating_sub(1);
            *dwell = d;
            if d == 0 {
                *strain = Vec3::ZERO;
            }
        }
    }
}

/// The per-element isotropic shear regime (architecture section 5a; see [`GripElements`]).
/// Each grounded element advances its bounded, recoverable world-space shear once; every
/// surface patch reads the same return law with its own capacity. Membership and budgets
/// key on the ELASTIC load, never the damped actual load: the damped load carries the support
/// damper's tick-scale transients, and feeding those into an integrating state amplified a
/// marginal mm-scale Nyquist wobble into a full force limit cycle (measured: ±90 kN damped-load
/// alternation over a ±11 kN elastic wobble, hull perfectly smooth — the support damper
/// converts wobble rate into load asymmetry, grip fed it back). Force is the strain direction
/// itself — isotropic, NO ellipse: lateral-vs-longitudinal asymmetry and turning resistance are
/// left to emerge from footprint geometry.
///
/// Advances `elements` in place and returns each contact's capped grip direction as
/// `(longitudinal, lateral)` components of a unit-bounded vector — pass 2 scales it by that
/// contact's μ × its elastic load. Membership bounds scale with `side_weight`
/// ([`GripParams::side_weight`]).
fn update_grip_elements(
    cols: &[ColumnContact],
    held: &[(usize, f32)],
    elements: &mut GripElements,
    side_weight: f32,
    dt: f32,
    element_len: usize,
    report: &mut SideReport,
) -> Vec<Vec2> {
    // `report` carries only the measurement-build probe slabs.
    #[cfg(not(feature = "bitprobe"))]
    let _ = &report;

    let enter = side_weight * GRIP_ELEMENT_ENTER_FRACTION;
    let leave = side_weight * GRIP_ELEMENT_LEAVE_FRACTION;
    let mut work = vec![ElementWork::default(); element_len];
    let mut elem_g = vec![Vec2::ZERO; cols.len()];
    // Split footprints advance their material element once, from its load-weighted slip.
    for c in cols {
        let element = &mut work[c.element];
        element.load += c.load_elastic;
        element.slip += (c.long_dir * c.slip_long - c.lat_dir * c.slip_lat) * c.load_elastic;
        element.count += 1;
    }
    for element in &mut work {
        element.membership_load = element.load;
    }
    for &(element, elastic) in held {
        work[element].membership_load += elastic;
    }
    for (idx, c) in cols.iter().enumerate() {
        // ENTER above the high bound; STAY down to the low bound while already a member
        // (hysteresis + dwell refresh). Never the damped load's sign — see
        // [`GRIP_ELEMENT_ENTER_FRACTION`].
        let element = &mut work[c.element];
        let elastic = element.membership_load;
        let member = elastic >= enter || (elastic >= leave && elements.dwell[c.element] > 0);
        if member {
            elements.dwell[c.element] = GRIP_ELEMENT_LOSS_DWELL_TICKS;
            element.kept = true;
        }
        if !c.has_plane || c.load_elastic <= 0.0 {
            continue;
        }
        // World-space shear rate in FORCE convention: the direction friction pushes the
        // hull (+slip_long along long_dir, −slip_lat along lat_dir — the kinetic law's
        // signs, vectorized).
        let sdot = c.long_dir * c.slip_long - c.lat_dir * c.slip_lat;
        if !element.advanced {
            if element.load >= enter {
                let mean_slip = if element.count == 1 {
                    sdot
                } else {
                    element.slip / element.load
                };
                elements.strain[c.element] =
                    advance_element_strain(elements.strain[c.element], mean_slip, dt);
            }
            // Below the enter bound the existing strain holds; each patch's force still scales
            // with its own current elastic load, including its own local damping slip.
            element.advanced = true;
        }
        let shared = elements.strain[c.element];
        let j1 = shared - c.normal * shared.dot(c.normal);
        if c.mu <= 0.0 {
            // A frictionless pair has no grip budget, and its damping partner is unbounded.
            continue;
        }
        // The damping partner, normalized per unit budget: σ1 = 2ζ√(σ0·m) with the element's
        // stiffness σ0 = μN/K and mass share m = N/g, at the LuGre-canonical ζ and this
        // contact's equivalent capacity/load coefficient μ.
        let d_coef = 2.0 * GRIP_ELEMENT_DAMPING_RATIO
            / (GRIP_SHEAR_FLOOR_M * c.mu * super::derive::G).sqrt();
        let mut g = (j1 / GRIP_SHEAR_FLOOR_M).clamp_length_max(1.0) + sdot * d_coef;
        let g_len = g.length();
        if g_len > 1.0 {
            g /= g_len;
        }
        elem_g[idx] = Vec2::new(g.dot(c.long_dir), g.dot(c.lat_dir));
    }
    // Columns the damped support clipped out this tick but whose elastic penetration
    // remains: still on the ground — membership continues, stored strain holds, no force
    // (support omitted the contact entirely).
    for &(el, _) in held {
        let element = &mut work[el];
        let elastic = element.membership_load;
        if elastic >= enter || (elastic >= leave && elements.dwell[el] > 0) {
            elements.dwell[el] = GRIP_ELEMENT_LOSS_DWELL_TICKS;
            element.kept = true;
        }
    }
    forget_departed_elements(&work, elements);
    #[cfg(feature = "bitprobe")]
    for output in &mut report.bitprobe_elements {
        let element = output.material as usize * 3 + output.column as usize;
        output.strain = elements.strain[element];
        output.dwell = u32::from(elements.dwell[element]);
    }
    elem_g
}

/// Emit support then total tangent force per contact. Returns summed grip and longitudinal
/// belt reaction, each in N.
fn emit_contact_forces(
    report: &mut SideReport,
    cols: &[ColumnContact],
    elem_g: &[Vec2],
    elements_mode: bool,
) -> (Vec2, f32) {
    let mut elem_sum = Vec2::ZERO;
    let mut belt_reaction = 0.0;
    for (idx, c) in cols.iter().enumerate() {
        report.apps.push(ForceApp {
            force: c.normal * c.load,
            point: c.p,
            collider: c.collider,
        });

        let mut f_long = 0.0;
        let mut f_lat = 0.0;
        let mut traction = Vec3::ZERO;
        if c.has_plane && elements_mode {
            // Each contact spends its own isotropic capacity on recoverable shear and damping.
            // No velocity-regularized kinetic term is added: its former near-zero slope was
            // MEASURED about 270 kN s/m per side, marginal at the fixed step (ADR-0026).
            let grip_el = c.mu * c.load_elastic;
            let g = elem_g[idx];
            f_long = grip_el * g.x;
            f_lat = grip_el * g.y;
            elem_sum += Vec2::new(f_long, f_lat);
            traction = c.long_dir * f_long + c.lat_dir * f_lat;
            belt_reaction += f_long;
        }
        let rolling = if c.has_plane {
            rolling_resistance(c.rolling * c.load, c.travel_speed)
        } else {
            0.0 // No longitudinal tangent exists on this contact.
        };
        let rolling_force = -c.long_dir * rolling;
        if c.has_plane && elements_mode {
            report.apps.push(ForceApp {
                force: traction + rolling_force,
                point: c.p,
                collider: c.collider,
            });
        }

        report.contacts.push(BeltContact {
            point: c.p,
            load: c.load,
            load_elastic: c.load_elastic,
            stiffness: c.stiffness,
            damping: c.damping,
            slip: c.slip_long,
            slip_lat: c.slip_lat,
            f_long,
            f_lat,
            normal: c.normal,
            traction,
            rolling_force,
            belt_velocity: c.belt_velocity,
            collider: c.collider,
            mu: c.mu,
        });
        #[cfg(feature = "bitprobe")]
        {
            let output = &mut report.bitprobe_elements[c.probe_index];
            output.active = true;
            output.slip_long = c.slip_long;
            output.slip_lat = c.slip_lat;
            output.f_long += f_long;
            output.f_lat += f_lat;
        }
    }
    (elem_sum, belt_reaction)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Force traces reconstruct the support resultant from total load and mean normal.
    /// FIXTURE: two perpendicular surface patches carrying 2 and 3 N.
    #[cfg(feature = "bitprobe")]
    #[test]
    fn a_probe_preserves_the_sum_of_its_patch_support_vectors() {
        let mut output = ElementOutputProbe::default();
        probe_patch(&mut output, (Vec3::ZERO, Vec3::Y), (2.0, 2.0));
        probe_patch(&mut output, (Vec3::ZERO, Vec3::X), (3.0, 3.0));
        assert!((output.normal * output.load - (Vec3::Y * 2.0 + Vec3::X * 3.0)).length() < 1e-6);
    }

    /// Force and view material identities must remain a permutation across the entire admitted
    /// wrap range. Signed subtraction at either extreme would panic or wrap in different builds.
    #[test]
    fn material_addresses_cover_the_loop_at_extreme_wrap_counts() {
        for wraps in [i64::MIN, -1, 0, i64::MAX] {
            let mut links: Vec<_> = (0..40)
                .map(|station| material_link(station, wraps, 40))
                .collect();
            links.sort_unstable();
            assert_eq!(links, (0..40).collect::<Vec<_>>());
        }
    }

    /// Material identity must never be computed from a non-finite pitch/travel or a saturated
    /// integer cast. Grip history and the shoe table rely on the same checked decomposition.
    #[test]
    fn phase_decomposition_refuses_invalid_or_unaddressable_inputs() {
        for (phase, pitch) in [
            (0.0, 0.0),
            (0.0, -1.0),
            (0.0, f32::NAN),
            (0.0, f32::INFINITY),
            (f64::NAN, 1.0),
            (f64::INFINITY, 1.0),
            (f64::MAX, 1.0),
            (1.0, f32::from_bits(1)),
        ] {
            assert!(std::panic::catch_unwind(|| phase_decompose(phase, pitch)).is_err());
        }
    }

    /// A contact pass cannot silently succeed without an enclosing material loop. Both drive
    /// adapters rely on this construction invariant when committing transmission and belt state.
    #[test]
    #[should_panic(expected = "admitted running gear has enough material links")]
    fn a_contact_pass_refuses_a_non_enclosing_link_count() {
        let input = SideInput {
            loop_pts: &rect_loop(0.02),
            count: 2,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        contact_side(
            &input,
            SideState::default(),
            Affine3A::IDENTITY,
            1.0 / 64.0,
            &rig_params(),
            &FlatGround { surface_y: 0.0 },
            &RubbleBodies::default(),
            |_| Vec3::ZERO,
            &mut GripElements::for_links(2),
        );
    }

    /// Flat horizontal ground: terrain fills `y < surface_y`. Direction-agnostic on purpose
    /// — only the loop's bottom run ever probes into it in the rig below.
    struct FlatGround {
        surface_y: f32,
    }
    impl DepthOracle for FlatGround {
        fn depth_along(&self, station: Vec3, _out: Vec3, reach: f32) -> f32 {
            (self.surface_y - station.y).min(reach)
        }
    }

    impl ContactOracle for FlatGround {
        fn contact_along(&self, station: Vec3, out: Vec3, reach: f32) -> TerrainContact {
            if out.y >= 0.0 {
                return TerrainContact::Miss;
            }
            TerrainContact::Hit {
                depth: self.depth_along(station, out, reach),
                normal: bevy::math::Dir3::Y,
            }
        }
    }

    /// A closed rectangular pin-line loop in the side plane (z, y), bottom run traversed +z
    /// so its outward normal points DOWN (`out2 = (tan.y, −tan.x)`). `bottom_y` positions the
    /// pin line; the contact face sits `face_offset` further out.
    fn rect_loop(bottom_y: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(-1.0, bottom_y),
            Vec2::new(1.0, bottom_y),
            Vec2::new(1.0, bottom_y + 0.5),
            Vec2::new(-1.0, bottom_y + 0.5),
            Vec2::new(-1.0, bottom_y),
        ]
    }

    /// The rig's collocation columns: symmetric about the pin plane (a fixture, not the
    /// measured-shoe form the game feeds — flat ground makes the tests offset-invariant).
    const TEST_COLUMNS: [(f32, f32); 3] = [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)];

    /// A small parked test vehicle for the element regime: perimeter 5 m, 25 links (pitch
    /// 0.2 m), 5 mm of bottom-face penetration at `bottom_y = 0.02`. The support damping is
    /// sized so a 2 m/s separation velocity clips every damped load to zero while elastic
    /// penetration remains.
    fn rig_params() -> ForceParams {
        ForceParams {
            face_offset: 0.025,
            free_travel: 0.0,
            support_stiffness_per_m: 2.0e6,
            support_damping_per_m: 1.0e5,
            engage_depth: 0.002,
            probe_reach: 0.5,

            grip: Some(GripParams {
                track: crate::substances::contact_friction(0.9),
                nominal_pressure_kpa: rolling_reference_kpa(),
                side_weight: 30_000.0 * 9.81 / 2.0,
            }),
        }
    }

    fn rig_tick(
        loop_pts: &[Vec2],
        state: &mut SideState,
        elems: &mut GripElements,
        surface_y: f32,
        vel: Vec3,
    ) -> SideReport {
        let input = SideInput {
            loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            // Empty knots: a rest-anchored zero-droop pad (the fixture's free_travel is 0).
            travel: TravelField { knots: &[] },
        };
        let report = contact_side(
            &input,
            *state,
            Affine3A::IDENTITY,
            1.0 / 64.0,
            &rig_params(),
            &FlatGround { surface_y },
            &RubbleBodies::default(),
            |_| vel,
            elems,
        );
        *state = report.state;
        report
    }

    /// Every collocation depth keeps the uncached oracle's exact bits, while neighbouring
    /// straight links cast their shared pin once. Driving relies on unchanged support arithmetic;
    /// direction or travel changes must prevent reuse. The closed pad is fixture geometry.
    #[cfg(feature = "bitprobe")]
    #[test]
    fn shared_pin_probes_are_cast_once_without_changing_any_depth_bits() {
        struct CountedGround {
            calls: std::cell::Cell<usize>,
        }
        impl DepthOracle for CountedGround {
            fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
                self.calls.set(self.calls.get() + 1);
                FlatGround { surface_y: 0.0 }.depth_along(station, out, reach)
            }
        }
        impl ContactOracle for CountedGround {
            fn contact_along(&self, station: Vec3, out: Vec3, reach: f32) -> TerrainContact {
                FlatGround { surface_y: 0.0 }.contact_along(station, out, reach)
            }
        }
        let loop_pts = rect_loop(0.02);
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        for phase in [0.0, 0.13] {
            let oracle = CountedGround {
                calls: std::cell::Cell::new(0),
            };
            let report = contact_side(
                &input,
                SideState {
                    phase,
                    ..SideState::default()
                },
                Affine3A::IDENTITY,
                1.0 / 64.0,
                &rig_params(),
                &oracle,
                &RubbleBodies::default(),
                |_| Vec3::ZERO,
                &mut GripElements::for_links(input.count),
            );
            // CountedGround counts scalar profile casts; the contact query is separate.
            let raw = report.bitprobe_contacts.len() * 3;
            assert!(
                oracle.calls.get() < raw,
                "straight neighbours must share their pin casts"
            );
            for contact in &report.bitprobe_contacts {
                for (point, depth) in [contact.query_a, contact.query_m, contact.query_b]
                    .into_iter()
                    .zip(contact.depths)
                {
                    let expected = FlatGround { surface_y: 0.0 }.depth_along(
                        point,
                        contact.out,
                        contact.reach,
                    );
                    assert_eq!(depth.to_bits(), expected.to_bits());
                }
            }
            println!(
                "MEASURED fixture terrain casts at phase {phase}: before={raw} after={}",
                oracle.calls.get()
            );
        }
    }

    /// The Driver must still see geometrically supported ground when separating velocity
    /// unloads the normal damper. A sensor collected only from emitted forces loses that ground.
    #[test]
    fn perceived_support_survives_dynamic_load_dropout() {
        // Fixture: a shallow pad, identical geometry, rest versus a rapid separating velocity.
        let loop_pts = rect_loop(0.02);
        let measure = |velocity| {
            let mut state = SideState::default();
            let mut elements = GripElements::for_links(25);
            rig_tick(&loop_pts, &mut state, &mut elements, 0.0, velocity)
        };
        let resting = measure(Vec3::ZERO);
        let separating = measure(Vec3::Y * 20.0);
        assert!(!resting.perceived_support.is_empty());
        assert!(
            separating.contacts.is_empty(),
            "fixture must unload the damped support"
        );
        assert_eq!(resting.perceived_support, separating.perceived_support);
    }

    fn strain_sum(elems: &GripElements) -> f32 {
        elems.strain.iter().map(|j| j.length()).sum()
    }

    /// Terrain fills `y > under_y` — an overhang/bridge deck for the return-run gate test.
    /// Directional: only upward probes see it.
    struct Ceiling {
        under_y: f32,
    }
    impl DepthOracle for Ceiling {
        fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
            if out.y <= 0.0 {
                return -reach;
            }
            (station.y - self.under_y).min(reach)
        }
    }

    impl ContactOracle for Ceiling {
        fn contact_along(&self, station: Vec3, out: Vec3, reach: f32) -> TerrainContact {
            if out.y <= 0.0 {
                return TerrainContact::Miss;
            }
            TerrainContact::Hit {
                depth: self.depth_along(station, out, reach),
                normal: bevy::math::Dir3::NEG_Y,
            }
        }
    }

    /// The travel band exists UNDER the belly only: a return-run station shares a belly
    /// station's z, and an ungated profile mirrored the suspension band onto the top of the
    /// loop — meeting overhead geometry `free_travel` too early. Rig: the top face passes
    /// 25 mm under a ceiling; the raised (ungated) datum would penetrate it 75 mm.
    #[test]
    fn the_envelope_band_never_mirrors_onto_the_return_run() {
        let params = ForceParams {
            free_travel: 0.1,
            ..rig_params()
        };
        let loop_pts = rect_loop(0.0);
        // Uniform 0.1 m of travel across the span (end-clamped): the belly gate in the law is
        // what must keep it off the return run, not the profile shape.
        let knots = [(-1.0, 0.1), (1.0, 0.1)];
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &knots },
        };
        let report = contact_side(
            &input,
            SideState::default(),
            Affine3A::IDENTITY,
            1.0 / 64.0,
            &params,
            &Ceiling { under_y: 0.55 },
            &RubbleBodies::default(),
            |_| Vec3::ZERO,
            &mut GripElements::for_links(25),
        );
        assert!(
            report.contacts.is_empty(),
            "the return run must meet the ceiling at its FACE, not at a phantom suspension \
             band ({} contacts)",
            report.contacts.len(),
        );
        // Receipt: the ceiling IS inside the ungated band's reach — drop it to the face line
        // and the top run must contact (the gate must not have killed top-run contact
        // outright).
        let report = contact_side(
            &input,
            SideState::default(),
            Affine3A::IDENTITY,
            1.0 / 64.0,
            &params,
            &Ceiling { under_y: 0.51 },
            &RubbleBodies::default(),
            |_| Vec3::ZERO,
            &mut GripElements::for_links(25),
        );
        assert!(
            !report.contacts.is_empty(),
            "a ceiling below the top face must still contact through the gated law"
        );
    }

    /// Recipient stability needs the derivative of the support force, including soft engagement
    /// and, while the belt closes, the damper's share of the ramp. FIXTURE: uniform 1 cm
    /// penetration in a 2 cm ramp, spring 2 MN/m per m, 2 m bottom run, at rest and closing at
    /// 1 m/s. DERIVED: F = k L p²/e + c L v p/e, so dF/dp = 2 k L p/e + c L v/e: 4 MN/m at rest,
    /// 14 MN/m closing at the rig's 100 kN s/m per m.
    #[test]
    fn engagement_reports_the_support_force_derivative() {
        let mut params = rig_params();
        params.engage_depth = 0.02;
        let loop_pts = rect_loop(params.face_offset);
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        for closing in [0.0, 1.0] {
            let read = |depth: f32| {
                let report = contact_side(
                    &input,
                    SideState::default(),
                    Affine3A::IDENTITY,
                    1.0 / 64.0,
                    &params,
                    &FlatGround { surface_y: depth },
                    &RubbleBodies::default(),
                    |_| Vec3::NEG_Y * closing,
                    &mut GripElements::for_links(25),
                );
                (
                    report.contacts.iter().map(|c| c.load).sum::<f32>(),
                    report.contacts.iter().map(|c| c.stiffness).sum::<f32>(),
                )
            };
            let delta = 1e-5; // FIXTURE finite-difference spacing within the ramp.
            let derivative = (read(0.01 + delta).0 - read(0.01 - delta).0) / (2.0 * delta);
            let reported = read(0.01).1;
            println!(
                "MEASURED closing {closing} m/s: ramp stiffness {reported} N/m, force derivative \
                 {derivative} N/m"
            );
            assert!(
                (reported / derivative - 1.0).abs() < 0.002,
                "closing {closing}: reported {reported} against {derivative}"
            );
        }
    }

    /// The envelope reframing: with the rest face exactly kissing flat ground, a zero-travel
    /// profile carries nothing — while a travel band turns the same kiss into a fully-drooped
    /// spring at exactly that much intrusion, so the total elastic load is
    /// `k · travel · bottom_run` to closed form. Same stations, same probes, same profile
    /// machinery — only the zero-force surface moved.
    #[test]
    fn the_free_travel_moves_the_support_datum_to_the_droop_envelope() {
        // Pin belly at +face_offset → outer face at y = 0 = the ground surface.
        let params = rig_params();
        let loop_pts = rect_loop(params.face_offset);
        let tick = |knots: &[(f32, f32)]| {
            let input = SideInput {
                loop_pts: &loop_pts,
                count: 25,
                plane_x: 0.0,
                columns: TEST_COLUMNS,
                travel: TravelField { knots },
            };
            contact_side(
                &input,
                SideState::default(),
                Affine3A::IDENTITY,
                1.0 / 64.0,
                &params,
                &FlatGround { surface_y: 0.0 },
                &RubbleBodies::default(),
                |_| Vec3::ZERO,
                &mut GripElements::for_links(25),
            )
        };

        // Rest datum: a kissing face has zero penetration — no contacts at all.
        assert!(
            tick(&[]).contacts.is_empty(),
            "a kissing face must carry nothing under the rest-anchored law"
        );

        // Envelope datum: the same pose sits `travel` deep into the suspension band. The
        // bottom run is 2 m; every column weight sums to 1 and the engage ramp saturates far
        // below the travel, so Σ elastic = k · travel · 2 m exactly.
        let travel = 0.08;
        let report = tick(&[(-1.0, travel), (1.0, travel)]);
        let total: f32 = report.contacts.iter().map(|c| c.load_elastic).sum();
        let expected = params.support_stiffness_per_m * travel * 2.0;
        assert!(
            (total - expected).abs() / expected < 1e-3,
            "envelope load {total:.1} N vs closed-form {expected:.1} N"
        );
        // And the reported contact points sit on the terrain surface, not on the envelope.
        for c in &report.contacts {
            assert!(
                c.point.y.abs() < 1e-4,
                "contact must land on the ground plane, got y = {}",
                c.point.y
            );
        }
    }

    /// Fixture: deep contact creeping along a stationary belt builds presliding strain.
    fn strained_pad() -> (Vec<Vec2>, SideState, GripElements, SideReport) {
        let loop_pts = rect_loop(0.02);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        let mut report = rig_tick(&loop_pts, &mut state, &mut elems, 0.0, Vec3::ZERO);
        for _ in 0..20 {
            report = rig_tick(
                &loop_pts,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
        }
        (loop_pts, state, elems, report)
    }

    /// Parking relies on elastic contact retaining strain when separation clips damped load
    /// to zero. Resetting elements absent from damped-load contacts would erase that strain.
    #[test]
    fn element_strain_survives_damped_load_dropout() {
        let (loop_pts, mut state, mut elems, report) = strained_pad();
        let enter = rig_params().grip.unwrap().side_weight * GRIP_ELEMENT_ENTER_FRACTION;
        assert!(
            report.contacts.iter().any(|c| c.load_elastic >= enter),
            "rig premise: grounded columns must clear the enter bound"
        );
        let before = elems.strain.clone();
        assert!(strain_sum(&elems) > 0.01, "rig premise: strain accumulated");
        // The dropout tick: strong separation velocity, every damped load clips to zero.
        let report = rig_tick(
            &loop_pts,
            &mut state,
            &mut elems,
            0.0,
            Vec3::new(0.0, 2.0, 0.0),
        );
        assert!(
            report.contacts.is_empty(),
            "rig premise: the separation tick must clip all damped loads"
        );
        assert_eq!(elems.strain, before, "one noisy tick erased parked strain");
        // Elastic membership holds through ANY number of such ticks — reset needs departure.
        for _ in 0..(4 * GRIP_ELEMENT_LOSS_DWELL_TICKS as usize) {
            rig_tick(
                &loop_pts,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 2.0, 0.0),
            );
        }
        assert_eq!(
            elems.strain, before,
            "sustained damped-load noise erased strain"
        );
    }

    /// The counterpart: definitive departure (no elastic contact at all) forgets, exactly one
    /// loss dwell after the last contact evidence.
    #[test]
    fn element_strain_forgets_on_definitive_departure() {
        let loop_pts = rect_loop(0.02);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        for _ in 0..20 {
            rig_tick(
                &loop_pts,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
        }
        assert!(strain_sum(&elems) > 0.01, "rig premise: strain accumulated");
        // Ground removed: the pad has left contact for good.
        for _ in 0..(GRIP_ELEMENT_LOSS_DWELL_TICKS as usize - 1) {
            rig_tick(&loop_pts, &mut state, &mut elems, -10.0, Vec3::ZERO);
        }
        assert!(
            strain_sum(&elems) > 0.01,
            "strain forgotten before the loss dwell expired"
        );
        rig_tick(&loop_pts, &mut state, &mut elems, -10.0, Vec3::ZERO);
        assert_eq!(strain_sum(&elems), 0.0, "departed strain must be forgotten");
        assert!(elems.dwell.iter().all(|&d| d == 0));
    }

    /// Parking strain must hold while an existing member fades into [leave, enter), even with
    /// continuing slip. Integration requires `load_elastic >= enter`; membership alone would
    /// keep growing strain in that band while departure tests stayed green.
    #[test]
    fn element_member_fading_into_band_holds_strain() {
        let params = rig_params();
        let side_weight = params.grip.unwrap().side_weight;
        let enter = side_weight * GRIP_ELEMENT_ENTER_FRACTION;
        let leave = side_weight * GRIP_ELEMENT_LEAVE_FRACTION;

        let (_, mut state, mut elems, report) = strained_pad();
        assert!(
            report.contacts.iter().any(|c| c.load_elastic >= enter),
            "rig premise: phase-1 contacts clear the enter bound"
        );
        assert!(strain_sum(&elems) > 0.01, "rig premise: strain accumulated");

        // Phase 2 — shallow contact (loads ≈ 100 N, mid-band [leave, enter)), same 25-link count
        // so element identity + strain slab persist. The pad is fading but still a live member
        // (dwell refreshed each tick); slip continues, so an `if member` integration branch WOULD
        // move the strain. It must hold instead.
        let band = rect_loop(0.024);
        let held = elems.strain.clone();
        for _ in 0..(2 * GRIP_ELEMENT_LOSS_DWELL_TICKS as usize) {
            let report = rig_tick(
                &band,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
            let loaded: Vec<f32> = report
                .contacts
                .iter()
                .map(|c| c.load_elastic)
                .filter(|&l| l > 0.0)
                .collect();
            assert!(
                !loaded.is_empty() && loaded.iter().all(|&l| (leave..enter).contains(&l)),
                "rig premise: fading contacts must sit in [leave, enter); got {loaded:?}"
            );
        }
        assert!(
            elems.dwell.iter().any(|&d| d > 0),
            "rig premise: the fading pad stays a live member (dwell refreshed)"
        );
        assert_eq!(
            elems.strain, held,
            "strain integrated in the hysteresis band — the branch must gate on load_elastic >= enter, not membership"
        );
    }

    /// An element below the enter bound must not integrate strain: a barely touching pad would
    /// otherwise store a sudden force for the moment real load arrives.
    #[test]
    fn element_below_enter_bound_never_integrates() {
        // 0.05 mm of face penetration: real contact, but orders below the enter bound.
        let loop_pts = rect_loop(0.024_95);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        let leave = rig_params().grip.unwrap().side_weight * GRIP_ELEMENT_LEAVE_FRACTION;
        for _ in 0..50 {
            let report = rig_tick(
                &loop_pts,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
            assert!(
                !report.contacts.is_empty()
                    && report.contacts.iter().all(|c| c.load_elastic < leave),
                "rig premise: contacts exist and sit below the leave bound"
            );
        }
        assert!(elems.strain.iter().all(|&j| j == Vec3::ZERO));
        assert!(elems.dwell.iter().all(|&d| d == 0));
    }

    /// The decomposition contract: offset strictly in `[0, pitch)` — never `pitch` itself,
    /// which `resample`'s internal `rem_euclid` would alias to 0 while material identity
    /// stayed a link behind — and the pair reconstructs the phase, so the two consumers
    /// cannot disagree.
    fn assert_canonical(phase: f64, pitch: f32) {
        let (wraps, offset) = phase_decompose(phase, pitch);
        assert!(
            (0.0..pitch).contains(&offset),
            "offset {offset} outside [0, {pitch}) at phase {phase}"
        );
        let rebuilt = wraps as f64 * f64::from(pitch) + f64::from(offset);
        let tol = f64::from(pitch) * 1e-6 + phase.abs() * 1e-9;
        assert!(
            (rebuilt - phase).abs() <= tol,
            "wraps {wraps} + offset {offset} rebuilds {rebuilt}, phase {phase}"
        );
    }

    #[test]
    fn phase_decompose_pitch_boundaries() {
        // Next-representable f64 on both sides of positive AND negative pitch multiples —
        // the exact ticks where an f32 remainder and an f64 floor could disagree — out to
        // ≈10 km of accumulated travel, the precision regime the f64 phase exists for. The
        // `next_down` legs are the carry seam: an f64 residual just under one pitch whose f32
        // cast rounds UP to exactly `pitch`. Without the carry, resample would alias the
        // offset to 0 (a full link ahead) while `wraps` lagged — the one-tick material mis-key.
        for pitch in [0.152_87_f32, 0.125] {
            let p = f64::from(pitch);
            let far = (10_000.0 / p).floor() as i64;
            for k in [-far, -40, -3, -1, 0, 1, 2, 7, 40, far] {
                let m = k as f64 * p;
                for phase in [m, m.next_down(), m.next_up()] {
                    assert_canonical(phase, pitch);
                }
            }
        }
    }

    /// A HULL ROLLING ALONG FLAT RIGID GROUND WITH NO ROLLING RESISTANCE NEEDS NO DRIVE FORCE,
    /// however its belt's approach runs are tilted: rolling at its belt's speed, the belt's ground
    /// reaction and the hull's net horizontal force are nil, and the support carries the load
    /// straight up. The top speed at the governor, the coast-down distances (design doc
    /// `track-model/architecture.md` §5a) and `c_rr` read as the whole of a ground's motion
    /// resistance rely on it. A support pushing along the tilted belt's normal (an unbalanced
    /// horizontal push from asymmetric runs), a damper reading the hull's velocity along that
    /// normal (a drag growing with speed), or traction along the tilted belt (a slip the flat run
    /// does not see) fails here.
    ///
    /// FIXTURE DATA: an asymmetric loop, its 2 m bottom run between approach runs rising 26.6°
    /// behind (+z) and 20.6° ahead (-z), 0.1 m of travel tapering to 0 at the run ends, its rest face 5 cm
    /// above directional flat ground; the rig's spring, damper and grip; 2 and 10 m/s over 64 ticks.
    #[test]
    fn a_hull_rolling_on_flat_ground_under_tilted_runs_needs_no_drive_force() {
        let loop_pts = [
            Vec2::new(-1.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.6, 0.3),
            Vec2::new(1.6, 0.5),
            Vec2::new(-1.4, 0.5),
            Vec2::new(-1.4, 0.15),
            Vec2::new(-1.0, 0.0),
        ];
        let knots = [(-1.4, 0.0), (-1.0, 0.1), (1.0, 0.1), (1.6, 0.0)];
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 40,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &knots },
        };
        let params = ForceParams {
            free_travel: 0.1,
            ..rig_params()
        };
        let affine = Affine3A::from_translation(Vec3::new(0.0, 0.075, 0.0));
        for speed in [2.0_f32, 10.0] {
            let mut state = SideState {
                speed,
                ..Default::default()
            };
            let mut elems = GripElements::for_links(input.count);
            let (mut reaction, mut push, mut load, mut tilted) = (0.0, Vec3::ZERO, 0.0, 0);
            for _ in 0..64 {
                let report = contact_side(
                    &input,
                    state,
                    affine,
                    1.0 / 64.0,
                    &params,
                    &crate::track::envelope::FlatRest,
                    &RubbleBodies::default(),
                    |_| Vec3::new(0.0, 0.0, -speed),
                    &mut elems,
                );
                let parked = contact_side(
                    &input,
                    SideState {
                        speed: 0.0,
                        ..state
                    },
                    affine,
                    1.0 / 64.0,
                    &params,
                    &crate::track::envelope::FlatRest,
                    &RubbleBodies::default(),
                    |_| Vec3::ZERO,
                    &mut GripElements::for_links(input.count),
                );
                assert_eq!(
                    report.contacts.len(),
                    parked.contacts.len(),
                    "rolling tangentially must not change which springs carry load"
                );
                for (moving, rest) in report.contacts.iter().zip(&parked.contacts) {
                    assert!(
                        (moving.load - rest.load).abs() < 1e-5 * rest.load,
                        "flat-ground damping changed load {} -> {} at {}",
                        rest.load,
                        moving.load,
                        moving.point
                    );
                }
                reaction += report.belt_reaction.abs();
                push += report.apps.iter().map(|app| app.force).sum::<Vec3>();
                load += report.contacts.iter().map(|c| c.load).sum::<f32>();
                tilted += report
                    .contacts
                    .iter()
                    .filter(|c| c.point.z.abs() > 1.0)
                    .count();
                state.phase += f64::from(speed / 64.0);
            }
            println!(
                "{speed} m/s: belt reaction {reaction:.3} N, horizontal push {:.3} N, load {load:.0} N, \
                 {tilted} approach-run contacts",
                push.z
            );
            assert!(tilted > 0, "the approach runs must carry load");
            assert!(
                reaction < 1e-5 * load && push.z.abs() < 1e-5 * load && push.x.abs() < 1e-5 * load,
                "{speed} m/s: belt reaction {reaction} N, push {push} N against {load} N of load"
            );
            assert!(
                (push.y - load).abs() < 1e-5 * load,
                "the support carries the load upright"
            );
        }
    }

    /// Surface support at the map edge must stay upright; the driver relies on no lateral
    /// shove from flat ground. Fixture: a downward probe 5 mm inside a flat grid edge.
    #[test]
    fn flat_heightfield_edge_does_not_turn_support_sideways() {
        use crate::terrain_grid::{HeightGrid, TerrainExtent};
        let grid = HeightGrid::new(
            vec![0.0; 4].into(),
            2,
            TerrainExtent {
                world_size_m: 4.0,
                height_offset_m: 0.0,
                height_span_m: 1.0,
            },
        );
        let field = crate::track::oracle::BlockField::new(vec![]).with_height(Some(grid));
        let point = Vec3::new(1.995, -0.05, 0.0);
        let TerrainContact::Hit { normal, .. } = field.contact_along(point, Vec3::NEG_Y, 0.5)
        else {
            panic!("the probe inside the edge must hit the flat facet");
        };
        let normal = *normal;
        println!("MEASURED edge normal {normal}");
        assert!(
            (normal - Vec3::Y).length() < 1e-5,
            "flat edge support {normal}"
        );
        let loop_pts = rect_loop(0.02);
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 1.995,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        let report = contact_side(
            &input,
            SideState::default(),
            Affine3A::IDENTITY,
            1.0 / 64.0,
            &rig_params(),
            &field,
            &RubbleBodies::default(),
            |_| Vec3::ZERO,
            &mut GripElements::for_links(input.count),
        );
        assert!(
            !report.contacts.is_empty(),
            "inside columns must carry the pad"
        );
        for contact in report.contacts {
            assert!(
                (contact.normal - Vec3::Y).length() < 1e-5,
                "edge column must load the flat facet, got {}",
                contact.normal
            );
        }
    }

    /// No first-hit surface means no support: drivers must not feel a phantom belt-normal
    /// spring over a gap or from saturated buried origins. Fixture: a pad in empty space, then
    /// fully inside a block; scalar depth remains useful to geometry consumers in both cases.
    #[test]
    fn missing_and_buried_probes_carry_no_support() {
        use crate::track::oracle::{BlockField, TerrainBlock};
        use bevy::math::Quat;
        let empty = BlockField::new(vec![]);
        let solid = BlockField::new(vec![TerrainBlock::new(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::splat(10.0),
        )]);
        assert!(matches!(
            empty.contact_along(Vec3::ZERO, Vec3::NEG_Y, 0.5),
            TerrainContact::Miss
        ));
        assert!(matches!(
            solid.contact_along(Vec3::ZERO, Vec3::NEG_Y, 0.5),
            TerrainContact::Buried
        ));
        let loop_pts = rect_loop(0.02);
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        for field in [empty, solid] {
            let report = contact_side(
                &input,
                SideState::default(),
                Affine3A::IDENTITY,
                1.0 / 64.0,
                &rig_params(),
                &field,
                &RubbleBodies::default(),
                |_| Vec3::Z,
                &mut GripElements::for_links(input.count),
            );
            assert!(
                report.apps.is_empty(),
                "no struck surface may invent support"
            );
        }
    }

    /// A vertical construction corner crossed laterally must not manufacture damping from a ray
    /// miss beside it. Drivers rely on this bound while a wrap arc scrubs along a corner.
    /// Fixture: a rounded wall facing +z and ending at x=0, a vertical pad 5 mm inside that
    /// lateral silhouette, moving toward -x at 2 m/s. No rubble or per-construction rule is used.
    #[test]
    fn vertical_construction_corner_damps_only_the_contact_point_normal_speed() {
        use crate::track::oracle::{BlockField, TerrainBlock};
        use bevy::math::Quat;
        let rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
            * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let field = BlockField::new(vec![TerrainBlock::new(
            rotation * Vec3::new(0.0, -0.4, -1.0),
            rotation,
            Vec3::new(4.0, 1.2, 2.0),
        )]);
        let loop_pts = rect_loop(0.02);
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        let affine = Affine3A::from_rotation_translation(
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            Vec3::new(-0.005, 0.0, 0.15),
        );
        let velocity = Vec3::NEG_X * 2.0;
        let report = contact_side(
            &input,
            SideState::default(),
            affine,
            1.0 / 64.0,
            &rig_params(),
            &field,
            &RubbleBodies::default(),
            |_| velocity,
            &mut GripElements::for_links(input.count),
        );
        assert!(
            !report.contacts.is_empty(),
            "the inside columns must strike the wall"
        );
        let mut max_closing = 0.0_f32;
        for contact in &report.contacts {
            let closing = (contact.load - contact.load_elastic) / contact.damping;
            max_closing = max_closing.max(closing);
        }
        println!("MEASURED vertical corner effective closing {max_closing:.6} m/s for 2 m/s hull");
        assert!(
            max_closing <= velocity.length() + 1e-4,
            "corner damping must not amplify lateral speed: {max_closing}"
        );
    }

    /// A rounded lip carries only its own pressure share. Coarse physical links and a finer
    /// quadrature of the same pad must agree in force; a whole-column centroid normal gives
    /// a different force as that centroid crosses the rounding band. Drivers rely on this
    /// resolution independence at curbs. FIXTURE: the step pad and a stationary force pass.
    #[test]
    fn rounded_lip_force_converges_under_station_refinement() {
        use crate::track::oracle::{BlockField, TerrainBlock};
        use bevy::math::Quat;
        let field = BlockField::new(vec![TerrainBlock::new(
            Vec3::new(0.0, -0.4, -1.0),
            Quat::IDENTITY,
            Vec3::new(4.0, 1.2, 2.0),
        )]);
        let loop_pts = rect_loop(0.02);
        let force = |count, z| {
            let input = SideInput {
                loop_pts: &loop_pts,
                count,
                plane_x: 0.0,
                columns: TEST_COLUMNS,
                travel: TravelField { knots: &[] },
            };
            let report = contact_side(
                &input,
                SideState::default(),
                Affine3A::from_translation(Vec3::new(0.0, 0.15, z)),
                1.0 / 64.0,
                &ForceParams {
                    grip: None,
                    ..rig_params()
                },
                &field,
                &RubbleBodies::default(),
                |_| Vec3::ZERO,
                &mut GripElements::for_links(count),
            );
            report
                .contacts
                .iter()
                .map(|c| c.normal * c.load)
                .sum::<Vec3>()
        };
        for z in [1.02, 1.0, 0.99, 0.98, 0.95, 0.9, 0.8] {
            let coarse = force(25, z);
            let fine = force(400, z);
            println!("MEASURED lip z={z}: coarse {coarse}, fine {fine}");
            // FIXTURE quadrature allowance: 15% against a 16-times finer station mesh.
            assert!(
                (coarse - fine).length() <= 0.15 * fine.length() + 1.0,
                "z={z}: coarse {coarse}, fine {fine}"
            );
        }
    }

    /// A pressure centroid can strike a curved feature between the three profile probes.
    /// Drivers need that feature's normal to carry only its local pressure, even when the
    /// endpoint and midpoint normals all agree. A three-probe-only refinement mutant rotates
    /// the whole metre-long fixture segment and fails this full-force comparison.
    #[test]
    fn an_off_centre_curved_hit_does_not_rotate_the_whole_pressure_column() {
        struct Bump {
            amplitude: f32,
            center: f32,
        }
        impl Bump {
            fn height_and_slope(&self, z: f32) -> (f32, f32) {
                // FIXTURE: 0.2 grade plus a Gaussian feature of 20 mm characteristic width.
                // DERIVED linear-profile centroid: 0.658730 m; its three probes miss the feature.
                let x = z - self.center;
                let bump = self.amplitude * (-(x / 0.02).powi(2)).exp();
                (0.2 * z + bump, 0.2 - 2.0 * x / 0.02_f32.powi(2) * bump)
            }
        }
        impl DepthOracle for Bump {
            fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
                if out.y >= 0.0 {
                    return -reach;
                }
                ((self.height_and_slope(station.z).0 - station.y) / -out.y).clamp(-reach, reach)
            }
        }
        impl ContactOracle for Bump {
            fn contact_along(&self, station: Vec3, out: Vec3, reach: f32) -> TerrainContact {
                if out.y >= 0.0 {
                    return TerrainContact::Miss;
                }
                let depth = self.depth_along(station, out, reach);
                TerrainContact::Hit { depth, normal: bevy::math::Dir3::new(Vec3::new(0.0, 1.0, -self.height_and_slope(station.z).1))
                    .expect("the analytic height graph has a finite gradient and a nonzero up component") }
            }
        }
        let loop_pts = rect_loop(0.02);
        let force = |count, curve: &Bump| {
            let input = SideInput {
                loop_pts: &loop_pts,
                count,
                plane_x: 0.0,
                columns: TEST_COLUMNS,
                travel: TravelField { knots: &[] },
            };
            let report = contact_side(
                &input,
                SideState::default(),
                Affine3A::IDENTITY,
                1.0 / 64.0,
                &ForceParams {
                    grip: None,
                    ..rig_params()
                },
                curve,
                &RubbleBodies::default(),
                |_| Vec3::ZERO,
                &mut GripElements::for_links(count),
            );
            report
                .contacts
                .iter()
                .map(|c| c.normal * c.load)
                .sum::<Vec3>()
        };
        // FIXTURE: an off-centre curved hit, then a lower surface whose centroid has
        // clearance but the same normal as the loaded endpoints. Both retain local support.
        for (curve, tolerance) in [
            (
                Bump {
                    amplitude: 0.025,
                    center: 0.658730 - 0.01,
                },
                0.03,
            ),
            (
                Bump {
                    amplitude: -0.15,
                    center: 0.658730,
                },
                0.15,
            ),
        ] {
            let (coarse, fine) = (force(5, &curve), force(400, &curve));
            println!("MEASURED off-centre surface coarse {coarse}, fine {fine}");
            assert!(
                (coarse - fine).length() < tolerance * fine.length(),
                "coarse {coarse}, fine {fine}"
            );
        }
    }

    /// A driven hull crossing a lip must not receive a rearward damping spike. Fixture: one
    /// 2 m pad translated forward (-z) across a 20 cm step, held at y=0.15 m, at 2 m/s.
    /// This force trace isolates the contact law from the hull integrator and gearbox.
    #[test]
    fn forward_step_lip_force_trace() {
        use crate::track::oracle::{BlockField, TerrainBlock};
        use bevy::math::Quat;
        let field = BlockField::new(vec![TerrainBlock::new(
            Vec3::new(0.0, -0.4, -1.0),
            Quat::IDENTITY,
            Vec3::new(4.0, 1.2, 2.0),
        )]);
        let loop_pts = rect_loop(0.02);
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        let mut peak = 0.0_f32;
        for z in [
            1.10, 1.02, 1.00, 0.99, 0.98, 0.97, 0.95, 0.90, 0.80, 0.60, 0.40, 0.20, 0.0, -0.2,
            -0.4, -0.6, -0.8, -1.0,
        ] {
            let report = contact_side(
                &input,
                SideState {
                    speed: 2.0,
                    ..Default::default()
                },
                Affine3A::from_translation(Vec3::new(0.0, 0.15, z)),
                1.0 / 64.0,
                &rig_params(),
                &field,
                &RubbleBodies::default(),
                |_| Vec3::NEG_Z * 2.0,
                &mut GripElements::for_links(input.count),
            );
            let force = report.apps.iter().map(|a| a.force.z).sum::<f32>();
            peak = peak.max(force);
            println!("MEASURED lip force hull_z={z:.3} F_z={force:.3} N");
        }
        println!("MEASURED lip peak rearward {peak:.3} N");
        // Fixture ceiling: 2 m of pad, c=100 kN s/m per metre, speed=2 m/s plus elastic
        // load at 5.5 cm penetration; a unit-normal damper cannot exceed that total load.
        assert!(
            peak <= 2.0 * (1e5 * 2.0 + 2e6 * 0.055),
            "rearward spike {peak}"
        );
    }

    /// Flat horizontal ground that states its ground: [`FlatGround`] whose every point is
    /// `ground`.
    struct GroundedFlat {
        surface_y: f32,
        ground: crate::ground_field::ContactGround,
    }
    impl DepthOracle for GroundedFlat {
        fn depth_along(&self, station: Vec3, _out: Vec3, reach: f32) -> f32 {
            (self.surface_y - station.y).min(reach)
        }
    }

    impl ContactOracle for GroundedFlat {
        fn ground_at(&self, _point: Vec3) -> Option<crate::ground_field::ContactGround> {
            Some(self.ground)
        }
        fn contact_along(&self, station: Vec3, out: Vec3, reach: f32) -> TerrainContact {
            FlatGround {
                surface_y: self.surface_y,
            }
            .contact_along(station, out, reach)
        }
    }

    /// One contact pass of the rig's pad, held 5 mm into `oracle`'s flat ground, its hull moving
    /// along −z at `hull_speed`, the way a positive belt speed drives it (the bottom run
    /// runs +z, so its drive direction is −z, `contact_frame`).
    fn grounded_tick(
        oracle: &GroundedFlat,
        state: &mut SideState,
        elems: &mut GripElements,
        hull_speed: f32,
    ) -> SideReport {
        let loop_pts = rect_loop(0.02);
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        let report = contact_side(
            &input,
            *state,
            Affine3A::IDENTITY,
            1.0 / 64.0,
            &rig_params(),
            oracle,
            &RubbleBodies::default(),
            |_| Vec3::new(0.0, 0.0, -hull_speed),
            elems,
        );
        *state = report.state;
        report
    }

    /// THE SAME VEHICLE ON TWO GROUNDS COASTS TO REST OVER THE DISTANCES ITS TWO ROLLING
    /// RESISTANCES DERIVE, and takes none at rest. A declutched side (no sprocket force) rolling
    /// with its hull decelerates as one body at `a = c_rr·N/(M + I)` down to the fade band and
    /// decays inside it, so it coasts `d = (v₀² + V²)/(2a)`. The driving feel of every ground and
    /// the coast-down distances the design doc derives rely on it. A law that ignored the
    /// contact's ground, read one ground everywhere, or applied hull drag with the wrong sign
    /// or load fails here. Grip transmits the declutched belt's inertia to the slowing hull.
    ///
    /// FIXTURE DATA: the rig's 2 m pad 5 mm into flat ground, its 500 kg belt, a hull whose weight
    /// is the pad's support load, from 3 m/s, on two grounds of rolling resistance 0.05 and 0.15
    /// (grip 0.9). 2 % covers the grip's slip transient.
    #[test]
    fn the_same_vehicle_coasts_to_rest_over_the_distances_its_ground_derives() {
        let coast = |c_rr: f32| {
            let oracle = GroundedFlat {
                surface_y: 0.0,
                ground: crate::ground_field::ContactGround {
                    grip: 0.9,
                    rolling_resistance: c_rr,
                },
            };
            let dt = 1.0 / 64.0;
            let inertia = 500.0; // Fixture reflected belt inertia, kg.
            let v0 = 3.0;
            let mut state = SideState {
                speed: v0,
                ..Default::default()
            };
            let mut elems = GripElements::for_links(25);
            let mut hull = v0;
            let mut distance = 0.0;
            let mut mass = 0.0;
            // The pad's support load varies from tick to tick as its stations advect (MEASURED:
            // the coast's mean is 8.7 % below the first tick's on this pad), so the derivation
            // reads the mean the rolling resistance acted on.
            let (mut load_sum, mut ticks) = (0.0, 0.0);
            for tick in 0..20_000 {
                let report = grounded_tick(&oracle, &mut state, &mut elems, hull);
                let load: f32 = report.contacts.iter().map(|c| c.load).sum();
                if tick == 0 {
                    mass = load / crate::track::derive::G;
                }
                load_sum += load;
                ticks += 1.0;
                let push: f32 = report
                    .contacts
                    .iter()
                    .map(|c| -c.tangential_force().z)
                    .sum();
                hull += push / mass * dt;
                state.speed -= report.belt_reaction / inertia * dt;
                state.phase += f64::from(state.speed * dt);
                distance += hull * dt;
                if hull.abs() < 1e-4 && state.speed.abs() < 1e-4 {
                    state.speed = 0.0;
                    let at_rest = grounded_tick(&oracle, &mut state, &mut elems, 0.0);
                    assert!(
                        at_rest
                            .contacts
                            .iter()
                            .all(|c| c.rolling_force == Vec3::ZERO),
                        "a vehicle at rest takes no rolling resistance"
                    );
                    let a = c_rr * (load_sum / ticks) / (mass + inertia);
                    let derived = (v0 * v0 + ROLLING_FADE_SPEED * ROLLING_FADE_SPEED) / (2.0 * a);
                    return (distance, derived);
                }
            }
            panic!("the vehicle on rolling resistance {c_rr} never came to rest");
        };
        let (firm, firm_derived) = coast(0.05);
        let (soft, soft_derived) = coast(0.15);
        println!(
            "coast-down: firm {firm:.3} m (derived {firm_derived:.3}), soft {soft:.3} m \
             (derived {soft_derived:.3})"
        );
        for (measured, derived) in [(firm, firm_derived), (soft, soft_derived)] {
            assert!(
                (measured - derived).abs() < 0.02 * derived,
                "coasted {measured:.3} m, derived {derived:.3} m"
            );
        }
    }

    /// A skid may yield the ground, but cannot store its entire travel as a spring.
    /// Drivers stopping or being pushed rely on recoverable travel being bounded by K.
    /// FIXTURE: locked 2 m pad on shipped mud, 4 m/s entry; dirt shoved 1.2 m sideways.
    #[test]
    fn ground_reversal_returns_at_most_one_shear_distance() {
        for sideways in [false, true] {
            let ground = crate::ground_types::GroundTypes::load_shipped()
                .get(if sideways { "dirt" } else { "mud" })
                .contact();
            let oracle = GroundedFlat {
                surface_y: 0.0,
                ground,
            };
            let mut state = SideState::default();
            let mut elements = GripElements::for_links(25);
            let mut velocity = if sideways { 0.6 } else { 4.0 };
            let mut distance = 0.0_f32;
            let mut peak = 0.0_f32;
            let mut reverse = 0.0_f32;
            let mut mass = 0.0;
            for tick in 0..64 * 20 {
                let report = if sideways {
                    let points = rect_loop(0.02);
                    contact_side(
                        &SideInput {
                            loop_pts: &points,
                            count: 25,
                            plane_x: 0.0,
                            columns: TEST_COLUMNS,
                            travel: TravelField { knots: &[] },
                        },
                        state,
                        Affine3A::IDENTITY,
                        1.0 / 64.0,
                        &rig_params(),
                        &oracle,
                        &RubbleBodies::default(),
                        |_| Vec3::X * velocity,
                        &mut elements,
                    )
                } else {
                    grounded_tick(&oracle, &mut state, &mut elements, velocity)
                };
                if tick == 0 {
                    mass = report.contacts.iter().map(|c| c.load_elastic).sum::<f32>()
                        / super::super::derive::G;
                }
                let force: f32 = report
                    .contacts
                    .iter()
                    .map(|c| {
                        if sideways {
                            c.tangential_force().x
                        } else {
                            -c.tangential_force().z
                        }
                    })
                    .sum();
                if !sideways || tick >= 128 {
                    velocity += force / mass / 64.0;
                }
                distance += velocity / 64.0;
                peak = peak.max(distance);
                reverse = reverse.min(velocity);
            }
            // FIXTURE rest observation: below 1 mm/s before comparing recovered travel.
            assert!(
                velocity.abs() < 0.001,
                "the pad must stop before its rebound is read"
            );
            let rollback = peak - distance;
            println!(
                "MEASURED fixed pad sideways={sideways}: peak={peak:.6} m rollback={rollback:.6} m reverse={reverse:.6} m/s"
            );
            let k = GRIP_SHEAR_FLOOR_M;
            // FIXTURE observation precision: f32 position accumulation permits 0.01% of K.
            assert!(
                rollback <= k * 1.0001,
                "yielded ground returned a skid as a spring: {rollback} m"
            );
        }
    }

    /// Closed, sub-breakaway dither must not drain a parked element's holding force.
    /// Drivers rely on zero plastic drift under the support solver's half-tick-rate wobble.
    #[test]
    fn ground_holding_force_survives_32_hz_dither() {
        let k = GRIP_SHEAR_FLOOR_M;
        let initial = Vec3::X * k * grip_law().breakaway_share.value / 2.0;
        let mut state = initial;
        // FIXTURE: 1 mm/s alternating slip, 64 Hz, 60 simulated seconds.
        for tick in 0..64 * 60 {
            let slip = Vec3::X * if tick % 2 == 0 { 0.001 } else { -0.001 };
            state = advance_element_strain(state, slip, 1.0 / 64.0);
        }
        println!(
            "MEASURED closed-dither holding share {} -> {}",
            initial.x / k,
            state.x / k
        );
        assert!(
            (state - initial).length() < k * 0.001,
            "closed small slip must retain static support, not ratchet: {state:?}"
        );
    }

    /// Drivers load an elastic pad up to breakaway and slide at finite capacity thereafter.
    /// The monotonic knee may change with its owned breakaway share; linear force readout stays.
    #[test]
    fn ground_loading_is_elastic_before_breakaway_and_bounded_after() {
        let k = GRIP_SHEAR_FLOOR_M;
        let mut state = Vec3::ZERO;
        for quarter in 1..=8 {
            for _ in 0..1024 {
                state = advance_element_strain(state, Vec3::X * k, 1.0 / 4096.0);
            }
            let travel = quarter as f32 / 4.0;
            assert!(state.x > 0.0 && state.x <= k);
            if travel <= grip_law().breakaway_share.value {
                assert!((state.x / k - travel).abs() < 0.0001);
            }
            println!(
                "MEASURED loading at {} K: capacity share {}",
                quarter as f32 / 4.0,
                state.x / k
            );
        }
    }

    /// Advected contact must resist yaw for steering. The Driver's saturated-sliding estimate
    /// is a comparison observation, not a bound. FIXTURE: 4.27 m pad, 1.4 m half tread.
    #[test]
    fn ground_turning_moment_with_advected_elements() {
        let ground = crate::ground_types::GroundTypes::load_shipped()
            .get("gravel")
            .contact();
        let oracle = GroundedFlat {
            surface_y: 0.0,
            ground,
        };
        let half = 4.27 / 2.0;
        let points = vec![
            Vec2::new(-half, 0.02),
            Vec2::new(half, 0.02),
            Vec2::new(half, 0.52),
            Vec2::new(-half, 0.52),
            Vec2::new(-half, 0.02),
        ];
        for yaw in [0.2, 0.6] {
            let mut state = SideState {
                speed: -yaw * 1.4,
                ..Default::default()
            };
            let mut elements = GripElements::for_links(50);
            let mut moment = 0.0;
            let mut price = 0.0;
            let mut samples = 0;
            for tick in 0..64 * 20 {
                let report = contact_side(
                    &SideInput {
                        loop_pts: &points,
                        count: 50,
                        plane_x: -1.4,
                        columns: TEST_COLUMNS,
                        travel: TravelField { knots: &[] },
                    },
                    state,
                    Affine3A::IDENTITY,
                    1.0 / 64.0,
                    &rig_params(),
                    &oracle,
                    &RubbleBodies::default(),
                    |p| Vec3::Y.cross(p) * yaw,
                    &mut elements,
                );
                state.phase += f64::from(state.speed / 64.0);
                if tick >= 64 * 15 {
                    samples += 1;
                    moment -= report
                        .contacts
                        .iter()
                        .map(|c| c.point.cross(c.traction).y)
                        .sum::<f32>();
                    let weight = report.contacts.iter().map(|c| c.load_elastic).sum();
                    price += super::super::load::observe(
                        report.perceived_support.iter(),
                        Vec3::NEG_Z,
                        weight,
                        None,
                    )
                    .turn_moment_nm;
                }
            }
            println!(
                "MEASURED turning yaw={yaw}: moment={:.6} Nm price={:.6} Nm share={:.6}",
                moment / samples as f32,
                price / samples as f32,
                moment / price
            );
            assert!(price > 0.0, "the fixture has a loaded sliding footprint");
            assert!(moment > 0.0, "advected contact must oppose the fixture yaw");
        }
    }

    /// Every ground supplies mu*N at either fixture load. Drivers must get no area or
    /// track-material multiplier on terrain. Fixture: a fixed sliding pad, full/half pressure.
    #[test]
    fn every_ground_delivers_its_grip_times_load_at_both_pressures() {
        for (name, ground) in crate::ground_types::GroundTypes::load_shipped().types {
            for scale in [1.0, 0.5] {
                let mut params = rig_params();
                params.support_stiffness_per_m *= scale;
                params.grip.as_mut().unwrap().nominal_pressure_kpa *= scale;
                // Deliberately unrelated track coefficient: it belongs only to solid recipients.
                params.grip.as_mut().unwrap().track = crate::substances::contact_friction(0.01);
                let oracle = GroundedFlat {
                    surface_y: 0.0,
                    ground: ground.contact(),
                };
                let loop_pts = rect_loop(0.02);
                let input = SideInput {
                    loop_pts: &loop_pts,
                    count: 25,
                    plane_x: 0.0,
                    columns: TEST_COLUMNS,
                    travel: TravelField { knots: &[] },
                };
                let mut elements = GripElements::for_links(input.count);
                let mut report = SideReport::default();
                for _ in 0..256 {
                    report = contact_side(
                        &input,
                        SideState::default(),
                        Affine3A::IDENTITY,
                        1.0 / 64.0,
                        &params,
                        &oracle,
                        &RubbleBodies::default(),
                        |_| Vec3::X * 2.0,
                        &mut elements,
                    );
                }
                assert!(!report.contacts.is_empty());
                let load: f32 = report.contacts.iter().map(|c| c.load_elastic).sum();
                let pull: f32 = report.contacts.iter().map(|c| c.f_lat.abs()).sum();
                assert!(
                    (pull - ground.grip.value * load).abs() < load * 1e-5,
                    "{name} scale={scale}"
                );
                assert!(report.contacts.iter().all(|c| c.mu == ground.grip.value));
                println!(
                    "MEASURED fixed-pad {name} pressure_scale={scale} load_n={load:.3} pull_n={pull:.3}"
                );
            }
        }
    }

    /// Below atan(mu), a braked fixture stops and stays; above it gravity exceeds every
    /// contact's capacity and sliding accelerates. Fixture mass follows the normal support
    /// at full/half pressure; the pad is constrained to a plane and belts are locked.
    #[test]
    fn every_ground_holds_below_its_slope_limit_and_slides_above_at_both_pressures() {
        let dt = 1.0 / 64.0; // Fixture fixed step, matching the shipped contact integration.
        for (name, ground) in crate::ground_types::GroundTypes::load_shipped().types {
            for scale in [1.0, 0.5] {
                for sign in [-1.0, 1.0] {
                    let limit = ground.grip.value.atan();
                    if limit == 0.0 && sign < 0.0 {
                        continue; // Zero grip has no positive holding-slope interval.
                    }
                    let margin = if sign < 0.0 {
                        limit / 2.0
                    } else {
                        (std::f32::consts::FRAC_PI_2 - limit) / 2.0
                    };
                    let angle = limit + sign * 1.0_f32.to_radians().min(margin);
                    let mut params = rig_params();
                    params.support_stiffness_per_m *= scale * angle.cos();
                    let oracle = GroundedFlat {
                        surface_y: 0.0,
                        ground: ground.contact(),
                    };
                    let loop_pts = rect_loop(0.02);
                    let input = SideInput {
                        loop_pts: &loop_pts,
                        count: 25,
                        plane_x: 0.0,
                        columns: TEST_COLUMNS,
                        travel: TravelField { knots: &[] },
                    };
                    let pieces = RubbleBodies::default();
                    let mut elements = GripElements::for_links(input.count);
                    let first = contact_side(
                        &input,
                        SideState::default(),
                        Affine3A::IDENTITY,
                        dt,
                        &params,
                        &oracle,
                        &pieces,
                        |_| Vec3::ZERO,
                        &mut elements,
                    );
                    let normal: f32 = first.contacts.iter().map(|c| c.load_elastic).sum();
                    let mass = normal / (super::super::derive::G * angle.cos());
                    let width = TEST_COLUMNS[2].0 - TEST_COLUMNS[0].0;
                    let length = 2.0; // Fixture bottom run.
                    let grip = params.grip.as_mut().unwrap();
                    grip.side_weight = mass * super::super::derive::G;
                    grip.nominal_pressure_kpa =
                        2.0 * grip.side_weight / (2.0 * width * length) / 1000.0;
                    let pull = mass * super::super::derive::G * angle.sin();
                    let capacity = ground.grip.value * normal;
                    assert_eq!(pull > capacity, sign > 0.0);
                    // Fixture: nonzero entry energy mu*m*g*K/32 exercises arrest before parking.
                    let (mut velocity, mut distance, mut late_distance) = (
                        0.25 * (ground.grip.value * super::super::derive::G * GRIP_SHEAR_FLOOR_M)
                            .sqrt(),
                        0.0_f32,
                        0.0_f32,
                    );
                    let mut late_speed = 0.0_f32;
                    for tick in 0..64 * 60 {
                        let report = contact_side(
                            &input,
                            SideState::default(),
                            Affine3A::IDENTITY,
                            dt,
                            &params,
                            &oracle,
                            &pieces,
                            |_| Vec3::X * velocity,
                            &mut elements,
                        );
                        let resistance: f32 = report.apps.iter().map(|a| a.force.x).sum();
                        velocity += (pull + resistance) / mass * dt;
                        distance += velocity * dt;
                        if tick >= 64 * 59 {
                            late_distance += velocity * dt;
                            late_speed = late_speed.max(velocity.abs());
                        }
                    }
                    // Resolution: f32 force summation leaves at most this relative residual;
                    // one minute of that residual bounds velocity and final-second travel.
                    let roundoff_speed = capacity / mass * 1e-5 * 60.0;
                    if sign < 0.0 {
                        assert!(
                            late_speed < roundoff_speed,
                            "{name} scale={scale} speed={late_speed}"
                        );
                        assert!(late_distance.abs() < roundoff_speed, "{name} keeps moving");
                    } else {
                        let minimum_speed = (pull - capacity) / mass * 60.0;
                        assert!(
                            velocity >= minimum_speed * (1.0 - 1e-4),
                            "{name} must slide"
                        );
                    }
                    println!(
                        "MEASURED fixed-pad slope {name} pressure_scale={scale} above={} angle_deg={:.4} settle_m={distance:.7} late_travel_m={late_distance:.7} speed_m_s={velocity:.7}",
                        sign > 0.0,
                        angle.to_degrees()
                    );
                }
            }
        }
    }

    /// Rolling follows hull travel, not belt spin; grip alone reacts on the belt. A spinning
    /// stationary rig and a coasting hull therefore read different rolling loads. Fixture pad.
    #[test]
    fn rolling_reads_hull_motion_independently_of_belt_spin() {
        let ground = crate::ground_field::ContactGround {
            grip: 0.9,
            rolling_resistance: 0.1,
        };
        let oracle = GroundedFlat {
            surface_y: 0.0,
            ground,
        };
        for belt_speed in [-2.0, 0.0, 2.0] {
            for hull_speed in [-ROLLING_FADE_SPEED, 0.0, ROLLING_FADE_SPEED] {
                let mut state = SideState {
                    speed: belt_speed,
                    ..Default::default()
                };
                let report = grounded_tick(
                    &oracle,
                    &mut state,
                    &mut GripElements::for_links(25),
                    hull_speed,
                );
                let load: f32 = report.contacts.iter().map(|c| c.load).sum();
                let expected = ground.rolling_resistance * load * (hull_speed / ROLLING_FADE_SPEED);
                let rolling: f32 = report.contacts.iter().map(|c| c.rolling_force.z).sum();
                assert!((rolling - expected).abs() < load * 1e-6);
                let reaction: f32 = report.contacts.iter().map(|c| c.f_long).sum();
                assert_eq!(report.belt_reaction, reaction);
                let hull_force: f32 = report.apps.iter().map(|a| -a.force.z).sum();
                assert!((hull_force - (reaction - expected)).abs() < load * 1e-6);
            }
        }
    }

    /// Pressure-dependent drag is read by the contact pass and by the Driver from the same
    /// spawn pressure; changing patch stiffness cannot change that pressure. Fixture pad.
    #[test]
    fn pressure_drag_reaches_both_contacts_and_driver() {
        let hard = rigid_rolling_resistance();
        let reference = rolling_reference_kpa();
        for (name, ground) in crate::ground_types::GroundTypes::load_shipped().types {
            let mut previous = hard;
            for pressure in [reference / 2.0, reference, reference * 2.0] {
                let expected =
                    hard + (ground.rolling_resistance.value - hard) * pressure / reference;
                let mut params = rig_params();
                params.grip.as_mut().unwrap().nominal_pressure_kpa = pressure;
                let oracle = GroundedFlat {
                    surface_y: 0.0,
                    ground: ground.contact(),
                };
                let surface = surface_under(
                    params.grip,
                    None,
                    Vec3::ZERO,
                    &oracle,
                    &RubbleBodies::default(),
                );
                assert_eq!(surface.rolling, expected, "{name}");
                assert!(surface.rolling >= previous);
                let loop_pts = rect_loop(0.02);
                let input = SideInput {
                    loop_pts: &loop_pts,
                    count: 25,
                    plane_x: 0.0,
                    columns: TEST_COLUMNS,
                    travel: TravelField { knots: &[] },
                };
                let report = contact_side(
                    &input,
                    SideState {
                        speed: ROLLING_FADE_SPEED,
                        ..Default::default()
                    },
                    Affine3A::IDENTITY,
                    1.0 / 64.0,
                    &params,
                    &oracle,
                    &RubbleBodies::default(),
                    |_| Vec3::NEG_Z * ROLLING_FADE_SPEED,
                    &mut GripElements::for_links(input.count),
                );
                let normal: f32 = report.contacts.iter().map(|c| c.load).sum();
                assert!(normal > 0.0);
                let rolling: f32 = report.contacts.iter().map(|c| c.rolling_force.z).sum();
                assert!((rolling - expected * normal).abs() < normal * 1e-6);
                let grip_reaction: f32 = report.contacts.iter().map(|c| c.f_long).sum();
                assert_eq!(
                    report.belt_reaction, grip_reaction,
                    "rolling must not directly load the belt"
                );
                let emitted: Vec3 = report.apps.iter().map(|a| a.force).sum();
                let reconstructed: Vec3 = report
                    .contacts
                    .iter()
                    .map(|c| c.normal * c.load + c.tangential_force())
                    .sum();
                assert!((emitted - reconstructed).length() < normal * 1e-6);
                let weight = params.grip.unwrap().side_weight * 2.0;
                let forecast = super::super::load::observe(
                    report.perceived_support.iter(),
                    Vec3::NEG_Z,
                    weight,
                    None,
                );
                assert!((forecast.rolling_n - expected * weight).abs() < weight * 1e-6);
                previous = surface.rolling;
            }
        }
        for pressure in [0.0, reference / 2.0, reference * 10.0] {
            assert_eq!(ground_rolling_resistance(hard, pressure), hard);
        }
        // Fixture heavy ground: r=hard+.2 at reference, mu=hard+.3. DERIVED threshold from r(p)=mu;
        // at twice the threshold drag exceeds grip, so no engine can leave the rolling fade.
        let (rolling, mu) = (hard + 0.2, hard + 0.3);
        let threshold = reference * (mu - hard) / (rolling - hard);
        assert!(ground_rolling_resistance(rolling, threshold / 2.0) < mu);
        assert!(ground_rolling_resistance(rolling, threshold * 2.0) > mu);
    }

    /// The retained shear law reads its common compliance linearly below breakaway.
    /// Fixture displacements stay inside the elastic zone, so the readout cannot saturate.
    #[test]
    fn recoverable_shear_reads_effective_compliance_in_the_force_pass() {
        for fraction in [0.25, 0.5] {
            let share = grip_law().breakaway_share.value * fraction;
            let oracle = GroundedFlat {
                surface_y: 0.0,
                ground: crate::ground_field::ContactGround {
                    grip: 0.5,
                    rolling_resistance: rigid_rolling_resistance(),
                },
            };
            let loop_pts = rect_loop(0.02);
            let input = SideInput {
                loop_pts: &loop_pts,
                count: 25,
                plane_x: 0.0,
                columns: TEST_COLUMNS,
                travel: TravelField { knots: &[] },
            };
            let mut elements = GripElements::for_links(input.count);
            elements.strain.fill(Vec3::X * (GRIP_SHEAR_FLOOR_M * share));
            let report = contact_side(
                &input,
                SideState::default(),
                Affine3A::IDENTITY,
                1.0 / 64.0,
                &rig_params(),
                &oracle,
                &RubbleBodies::default(),
                |_| Vec3::ZERO,
                &mut elements,
            );
            assert!(!report.contacts.is_empty());
            for c in report.contacts {
                assert!(
                    (c.f_lat.abs() / (c.load_elastic * oracle.ground.grip) - share).abs() < 1e-6
                );
            }
        }
    }

    /// Driver grip is nominal-pressure strength. Spring stiffness is not a ground fact;
    /// changing it at fixed footprint must not change a paid gear decision's load.
    #[test]
    fn driver_observation_does_not_read_spring_load() {
        let oracle = GroundedFlat {
            surface_y: 0.0,
            ground: crate::ground_types::GroundTypes::load_shipped()
                .get("dirt")
                .contact(),
        };
        let loop_pts = rect_loop(0.02);
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: TEST_COLUMNS,
            travel: TravelField { knots: &[] },
        };
        let mut params = rig_params();
        let mut observed = Vec::new();
        for factor in [1.0, 0.5] {
            params.support_stiffness_per_m = rig_params().support_stiffness_per_m * factor;
            let report = contact_side(
                &input,
                SideState::default(),
                Affine3A::IDENTITY,
                1.0 / 64.0,
                &params,
                &oracle,
                &RubbleBodies::default(),
                |_| Vec3::ZERO,
                &mut GripElements::for_links(25),
            );
            observed.push(super::super::load::observe(
                report.perceived_support.iter(),
                Vec3::NEG_Z,
                params.grip.unwrap().side_weight * 2.0,
                Some(10.0),
            ));
        }
        println!(
            "MEASURED observer grip: {} -> {} N",
            observed[0].grip_n, observed[1].grip_n
        );
        let weight = params.grip.unwrap().side_weight * 2.0;
        let expected_grip = oracle.ground.grip * weight;
        let expected_rolling = oracle
            .ground
            .rolling_at(params.grip.unwrap().nominal_pressure_kpa)
            * weight;
        for observed in &observed {
            assert!((observed.grip_n - expected_grip).abs() < weight * 1e-6);
            assert!((observed.rolling_n - expected_rolling).abs() < weight * 1e-6);
        }
        assert_eq!(observed[0].grip_n, observed[1].grip_n);
        assert_eq!(observed[0].turn_n, observed[1].turn_n);
    }

    /// A curb patch must not erase the loaded flat patch's longitudinal shear. Contact
    /// enumeration is geometry bookkeeping, not a choice of constitutive plane.
    #[test]
    fn shared_strain_is_independent_of_patch_order() {
        let patch = |normal, load| ColumnContact {
            p: Vec3::ZERO,
            normal,
            load,
            load_elastic: load,
            stiffness: 1.0,
            damping: 0.0,
            long_dir: Vec3::X,
            lat_dir: normal.cross(Vec3::X),
            has_plane: true,
            slip_long: 0.0,
            slip_lat: 0.0,
            element: 0,
            belt_velocity: Vec3::ZERO,
            collider: None,
            mu: 0.5,
            travel_speed: 0.0,
            rolling: 0.0,
            #[cfg(feature = "bitprobe")]
            probe_index: 0,
        };
        let mut results = Vec::new();
        for normals in [
            [(Vec3::Z, 1.0), (Vec3::Y, 1000.0)],
            [(Vec3::Y, 1000.0), (Vec3::Z, 1.0)],
        ] {
            let cols = normals.map(|(normal, load)| patch(normal, load));
            let mut elements = GripElements::for_links(1);
            elements.strain[0] = Vec3::Z * 0.02; // FIXTURE elastic displacement.
            let g = update_grip_elements(
                &cols,
                &[],
                &mut elements,
                1000.0,
                1.0 / 64.0,
                3,
                &mut SideReport::default(),
            );
            let flat = normals.iter().position(|(n, _)| *n == Vec3::Y).unwrap();
            results.push(g[flat]);
        }
        assert_eq!(results[0], results[1]);
        assert!(results[0].length() > 0.0);
    }
}
