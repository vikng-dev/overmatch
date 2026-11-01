//! The belt force model (architecture §1 "SIM forces", phase B): support, traction, and belt
//! dynamics for one track side — the SINGLE implementation, consumed by the game's sim plugin
//! and by the track sandbox (which is where every piece of it was developed and feel-tested;
//! provenance in `track_sandbox` steps 17–26 and HQ.md).
//!
//! No ECS: the caller supplies the pose affine, a velocity field, the terrain oracle and the
//! tick-start piece shapes, then applies the returned forces in report order. The field uses
//! closed-form profiles; pieces use convex footprint checks and the same support law. Snapshot
//! and force-accumulator order are fixed by the server adapter ([`super::sim`]).
//!
//! The model, per station segment × three lateral collocation columns:
//! - **Support**: directional field depth at pin/mid/pin on the contact envelope (the outer
//!   face pushed out by the drooped suspension's maximum reach) → two-piece clipped-linear
//!   pressure profile → spring along the belt's own inward normal (minus normal-velocity
//!   damping, soft engagement ramp), applied at the profile centroid on the terrain surface.
//!   Ground intruding into the envelope→rest band IS the suspension compression. The reach is
//!   the REQUIRED per-position [`SideInput::travel`] profile, not a scalar: 0 at the unsprung
//!   sprocket/idler, full droop at every road wheel (`envelope::wheel_travel_knots`). An empty
//!   knot slice degenerates to a rest-anchored pad with no droop band — a test fixture, not a
//!   production mode. Roll/pitch/weight transfer are lever-arm implicit.
//! - **Traction**: per-element isotropic shear ([`GripElements`]) — every grounded link ×
//!   column carries its own bristle strain, force follows the strain direction, each element
//!   saturates against μ × its own elastic load. Longitudinal force reacts back into belt
//!   dynamics.
//! - **Belt dynamics** are the joint drivetrain's (`track::transmission::step`): this module hands
//!   it each side's summed ground reaction, and the caller advects the phase.

use avian3d::prelude::Friction;
use bevy::ecs::entity::Entity;
use bevy::math::{Affine3A, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use super::oracle::TerrainOracle;
use super::rubble::{RubbleBodies, Slab, SlabOverlap};

use super::route::{polyline_len, resample};
#[cfg(feature = "bitprobe")]
use crate::bitprobe::{ContactInputProbe, ElementOutputProbe};

/// The static-grip shear modulus (m): the Janosi–Hanamoto `K` for RUBBER TRACK PADS ON
/// FIRM GROUND (Wong & Chiang's measured parameter set — the terramechanics provenance,
/// static-friction-design.md §3). Full grip develops over this much shear; a 20° park
/// settles back ~28 mm before holding — the vehicle "rocks onto its brakes". When the
/// ground-type mechanic lands, TERRAIN owns this dial (firm soil ~10 mm … loose sand 25 mm).
///
/// Deliberately NOT the design draft's 5 mm park target: at 64 Hz that stiffness drove a
/// full-amplitude coupled roll/yaw limit cycle (measured: 32 Hz side-swap, 7× per-tick load
/// swings — the textbook undamped-bristle oscillation). At 75 mm every coupled mode sits
/// deep inside the semi-implicit stability region (ωΔt ≈ 0.27; per-tick growth damping
/// ratio ≈ 0.07 of the limit).
pub const GRIP_SHEAR_MODULUS_M: f32 = 0.075;

/// What the belt grips with: the friction its track and the terrain field bring to a contact, and
/// the side's weight share. Absent from [`ForceParams::grip`], the law runs support only.
#[derive(Clone, Copy, Debug)]
pub struct GripParams {
    /// The track's side of every pair ([`crate::spec::TrackSpec::friction`]).
    pub track: Friction,
    /// The terrain field's side: the ground type its collider carries
    /// ([`crate::ground_types::GroundTypes::terrain_friction`]).
    pub ground: Friction,
    /// Nominal weight on one side (N), half the vehicle's: the scale of the element membership
    /// bounds and the vehicle share of the transmission's shift-landing inertia.
    pub side_weight: f32,
}

impl GripParams {
    /// A tank's grip from its data: its track's substance on the terrain's base ground, at half
    /// its weight.
    pub(crate) fn of(spec: &crate::spec::TankSpec) -> Self {
        Self {
            track: spec.track.friction(),
            ground: crate::ground_types::GroundTypes::load_shipped().terrain_friction(),
            side_weight: spec.mass * super::derive::G / 2.0,
        }
    }

    /// The coefficient the belt slides at on a surface carrying `surface`: the one pair rule
    /// (`substances::PAIR_RULE`), combined as avian's narrow phase combines a pair.
    pub fn pair(&self, surface: Friction) -> f32 {
        self.track.combine(surface).dynamic_coefficient
    }

    /// The coefficient the belt slides at on the terrain field.
    pub fn on_ground(&self) -> f32 {
        self.pair(self.ground)
    }
}

/// Bristle damping ratio: the elastic-zone bristle is an undamped integrator in the hull's
/// resonance loop — without its damping partner a large disturbance (spawn drop, explosion
/// kick) can capture a saturated oscillation attractor (the literature's textbook pairing —
/// TMeasy `F = c·x + d·ẋ`, Pacejka's Besselink term). LuGre sizes the bristle damper
/// σ1 = 2ζ√(σ0·m) with ζ = 0.5 (Canudas de Wit et al., "A New Model for Control of Systems
/// with Friction", IEEE TAC 1995 — the canonical bristle-mode damping choice, NOT a
/// measured rubber loss factor). Sized against the 64 Hz explicit bound: 143.5 kN·s/m per
/// side is 47% below the ~270 kN·s/m regularizer slope that measurably rang the contact
/// modes (ADR-0026 §3), with an 11.7× Jury margin on the measured 1.6 Hz post-stop yaw mode
/// — the wobble this constant exists to close (one ≤0.03 rad/s rebound instead of a ζ=0.15
/// ring; the Dupont elastic zone is deliberately lossless there, and lowering it was
/// quantitatively dead: ζ_eq ≈ 0.002 at the observed amplitude, 79× short).
const GRIP_ELEMENT_DAMPING_RATIO: f32 = 0.5;

/// Elastic fraction of the grip budget: below `GRIP_BREAKAWAY·C` the state is a PURE spring
/// (zero plastic flow) — the Dupont elasto-plastic branch that kills presliding drift
/// (plain Dahl ratchets downhill under oscillating loads; our at-rest suspension limit
/// cycle + MG recoil are exactly that). α blends smoothly to full Dahl flow at the cap.
const GRIP_BREAKAWAY: f32 = 0.5;

/// Element membership bounds, as fractions of the side's nominal weight: a pad ENTERS the
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

/// Contact-loss dwell (fixed 64 Hz ticks): a pad whose elastic load falls below the leave
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

/// `1 − smoothstep` on [0, 1] — the hold blend. The transmission reads it as the
/// brake-engagement envelope and the drag release (transmission-design.md §3: the hold reframe
/// keeps the blend only as the engagement smoothing, never as a reaction attenuator).
pub fn hold_blend(x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
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
    /// Slip speed (m/s) normalizing the transmission's brake-engagement blend — the scale on
    /// which a belt counts as "moving".
    pub slip_saturation: f32,
    /// Reflected belt + drivetrain inertia (kg).
    pub inertia: f32,
    /// The traction law's inputs. Each contact's Coulomb coefficient is the pair its track makes
    /// with the surface under it ([`GripParams::pair`]): the element law is isotropic, so that
    /// pair is the whole friction story per element (μ × elastic load); turning resistance
    /// emerges from footprint geometry, not from an authored lateral share. `None` runs support
    /// only, with no traction or belt hold: the envelope calibration measures penetration area
    /// without traction in the loop.
    pub grip: Option<GripParams>,
}

/// THE traction state: per-element isotropic shear — the Wong & Chiang resultant-j /
/// Janosi–Hanamoto form proper. One accumulated shear vector per
/// material link × lateral column (flat index `link * 3 + column`), WORLD space, meters.
/// Each grounded element resists relative ground motion in ANY direction (no friction
/// ellipse, no authored lateral share); turning resistance emerges from the footprint geometry —
/// fore and aft elements strain in opposing directions under yaw, which is exactly the
/// rotational stick a single per-side resultant could never represent (a load-weighted
/// mean slip cancels antisymmetric lateral slip — the "pivots on ice" defect).
/// State rides MATERIAL identity (advects with belt phase, same mapping as the witness
/// link). A pad's memory survives as long as the pad is plausibly on the ground: membership
/// is keyed to ELASTIC load with enter/leave hysteresis plus a short loss dwell
/// ([`GRIP_ELEMENT_LOSS_DWELL_TICKS`]), and only definitive departure (dwell expired —
/// cycled to the return run, or lifted off) forgets — memory expires within one
/// contact-patch dwell while driving; a parked tank holds it through contact flicker.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GripElements {
    /// The pads' accumulated shear against the ground (bristle deflection, world-space
    /// meters), one per material link × lateral column, flat index `link * 3 + column`.
    pub strain: Vec<Vec3>,
    /// Ticks of contact-loss grace left per element before its shear is forgotten. A
    /// parallel `Vec<u8>` rather than a widened strain entry: 1 byte/element with no
    /// padding (a `(Vec3, u8)` entry rounds up to 16 bytes) and the strain slab stays
    /// densely packed — this state is rollback-historied and checkpointed, every byte counts.
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
    /// body takes back is the caller's (`track::rubble::piece_impulses`), never `-force`.
    pub collider: Option<Entity>,
}

/// One contact's telemetry (viz / traces). `load` is the ACTUAL damped load applied this
/// tick — `load_elastic` is the spring-only component (static-sink analysis, and the one
/// the element law's friction budget and membership key on).
pub struct BeltContact {
    pub point: Vec3,
    pub load: f32,
    pub load_elastic: f32,
    /// Longitudinal slip (belt speed − ground-point speed, m/s).
    pub slip: f32,
    /// Lateral scrub speed at the contact (m/s).
    pub slip_lat: f32,
    /// Scalar friction components along the contact's longitudinal / lateral axes (N).
    pub f_long: f32,
    pub f_lat: f32,
    pub normal: Vec3,
    pub traction: Vec3,
    /// The belt surface's world velocity at the contact (m/s): the hull's velocity there less
    /// the pre-tick belt speed along the drive direction (the hull's velocity alone where the
    /// contact has no tangent plane).
    pub belt_velocity: Vec3,
    /// The collider under the contact, `None` for the terrain field.
    pub collider: Option<Entity>,
    /// The Coulomb coefficient of this contact's pair, the track on the surface under it
    /// ([`GripParams::pair`]). The piece's reaction is clamped by the same coefficient
    /// (`track::rubble::piece_impulses`). Zero on a support-only rig, which grips nothing.
    pub mu: f32,
}

/// What one side's tick produced.
#[derive(Default)]
pub struct SideReport {
    pub state: SideState,
    pub apps: Vec<ForceApp>,
    pub contacts: Vec<BeltContact>,
    /// Ground reaction summed into belt dynamics (Σ f_long, N).
    pub belt_reaction: f32,
    /// Full oracle-call inputs/results, in execution order. Measurement builds only.
    #[cfg(feature = "bitprobe")]
    pub(crate) bitprobe_contacts: Vec<ContactInputProbe>,
    /// Per-station/column post-update load, slip, force, strain, and dwell. Measurement builds only.
    #[cfg(feature = "bitprobe")]
    pub(crate) bitprobe_elements: Vec<ElementOutputProbe>,
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
    // transmission both key on.
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
    // `report` carries only the measurement-build probe slabs.
    #[cfg(not(feature = "bitprobe"))]
    let _ = &report;

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

        let lateral = (
            input.columns[0].0.min(input.columns[2].0),
            input.columns[0].0.max(input.columns[2].0),
        );
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
            let probes = probe_depths(
                &mut previous_end[ci],
                oracle,
                [ca + face_a, (ca + cb) / 2.0 + face_m, cb + face_b],
                out,
                travel.map(|travel| params.probe_reach + travel),
            );
            let pen = probes;
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
            for &patch in patches {
                let SupportPatch {
                    origin: ca,
                    len,
                    weight,
                    pen,
                    travel,
                    collider,
                } = patch;
                let pen_max = pen[0].max(pen[1]).max(pen[2]);
                if pen_max <= 0.0 {
                    continue;
                }

                let Some((area, contact_len, x_c)) = profile_resultant(len, pen) else {
                    continue;
                };
                // Resultant at the terrain surface, on this column: the profile's own value at
                // the centroid position. (The normal force is offset-invariant along its own
                // line; the traction lever is not.) The centroid's own datum interpolates the
                // travel exactly like the penetration, so `p` lands on the terrain surface under
                // a varying profile too.
                let pen_c = sample_halves(x_c, len, pen).max(0.0);
                let t_c = sample_halves(x_c, len, travel);
                let p = ca + axis * x_c + out * (params.face_offset + t_c - pen_c);

                // Support: penalty spring along the belt's own inward normal, at the column's
                // share of the per-metre coefficients. Damping and slip read the hull's velocity
                // relative to the surface it presses: a piece's own motion enters the slip, which
                // is stable because the piece takes back only a clamped impulse
                // (`track::rubble::piece_impulses`), never the explicit reaction.
                let normal = -out;
                let hull_vel = vel_at(p);
                let vel = match collider {
                    Some(collider) => hull_vel - pieces.velocity_at(collider, p),
                    None => hull_vel,
                };
                let engage = (pen_max / params.engage_depth).clamp(0.0, 1.0);
                let load_elastic = weight * params.support_stiffness_per_m * area * engage;
                let load = weight
                    * (params.support_stiffness_per_m * area
                        - params.support_damping_per_m * contact_len * vel.dot(normal))
                    .max(0.0)
                    * engage;
                #[cfg(feature = "bitprobe")]
                {
                    let output = &mut report.bitprobe_elements[probe_index];
                    output.point = if output.load == 0.0 {
                        p
                    } else {
                        (output.point * output.load + p * load) / (output.load + load)
                    };
                    output.load += load;
                    output.load_elastic += load_elastic;
                    output.normal = normal;
                }
                if load <= 0.0 {
                    if elements_mode && load_elastic > 0.0 {
                        held.push((material(i) * 3 + ci, load_elastic));
                    }
                    continue;
                }

                let (long_dir, lat_dir, has_plane, slip_long, slip_lat) =
                    contact_frame(affine, tan2, normal, vel, belt_speed);
                let belt_velocity = hull_vel - long_dir * belt_speed;
                // One rule for ground and pieces: the track's friction paired with the surface's.
                // A support-only rig grips nothing.
                let mu = params.grip.map_or(0.0, |grip| match collider {
                    Some(collider) => grip.pair(pieces.friction(collider)),
                    None => grip.on_ground(),
                });

                cols.push(ColumnContact {
                    p,
                    normal,
                    load,
                    load_elastic,
                    long_dir,
                    lat_dir,
                    has_plane,
                    slip_long,
                    slip_lat,
                    element: material(i) * 3 + ci,
                    belt_velocity,
                    collider,
                    mu,
                    #[cfg(feature = "bitprobe")]
                    probe_index,
                });
            }
        }
    }
    (cols, held)
}

/// One element's shear state after this tick: the Dupont elasto-plastic update — pure spring
/// below breakaway or when slip unloads it, smoothstepping to full Dahl flow at one shear
/// modulus of strain. The result is kept in the contact tangent plane (terrain curvature
/// rotates it out) and capped at one shear modulus (the Dahl saturation the rational update
/// converges to; the projection is the α < 1 safety net).
fn advance_element_strain(j0: Vec3, sdot: Vec3, normal: Vec3, dt: f32) -> Vec3 {
    let speed = sdot.length();
    let alpha = if j0.dot(sdot) < 0.0 {
        0.0
    } else {
        let m = ((j0.length() / GRIP_SHEAR_MODULUS_M - GRIP_BREAKAWAY) / (1.0 - GRIP_BREAKAWAY))
            .clamp(0.0, 1.0);
        m * m * (3.0 - 2.0 * m)
    };
    let mut j1 = (j0 + sdot * dt) / (1.0 + alpha * (dt / GRIP_SHEAR_MODULUS_M) * speed);
    j1 -= j1.dot(normal) * normal;
    let j_len = j1.length();
    if j_len > GRIP_SHEAR_MODULUS_M {
        j1 *= GRIP_SHEAR_MODULUS_M / j_len;
    }
    j1
}

/// Definitive departure only: the dwell rides out contact flicker; when it expires the pad has
/// truly left the ground (cycled to the return run, or lifted off) and forgets — its shear was
/// relieved by ground it no longer touches. `kept` flags the elements this tick found evidence
/// for; every other element counts down.
fn forget_departed_elements(kept: &[bool], elements: &mut GripElements) {
    for ((&kept, dwell), strain) in kept
        .iter()
        .zip(elements.dwell.iter_mut())
        .zip(elements.strain.iter_mut())
    {
        if !kept {
            let d = dwell.saturating_sub(1);
            *dwell = d;
            if d == 0 {
                *strain = Vec3::ZERO;
            }
        }
    }
}

/// The per-element isotropic shear regime (static-friction-design.md §3; see [`GripElements`]).
/// Each grounded element integrates its own world-space shear vector; membership and budgets
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
    let mut kept = vec![false; element_len];
    let mut elem_g = vec![Vec2::ZERO; cols.len()];
    // A footprint can split a material element into several surface patches. Membership reads
    // their summed elastic load, and its one strain advances once from their load-weighted slip.
    // The single-contact case keeps the field-only arithmetic unchanged.
    let mut loads = vec![0.0; element_len];
    let mut slips = vec![Vec3::ZERO; element_len];
    let mut counts = vec![0_usize; element_len];
    let mut advanced = vec![false; element_len];
    for c in cols {
        loads[c.element] += c.load_elastic;
        slips[c.element] += (c.long_dir * c.slip_long - c.lat_dir * c.slip_lat) * c.load_elastic;
        counts[c.element] += 1;
    }
    let mut membership_loads = loads.clone();
    for &(element, elastic) in held {
        membership_loads[element] += elastic;
    }
    for (idx, c) in cols.iter().enumerate() {
        // ENTER above the high bound; STAY down to the low bound while already a member
        // (hysteresis + dwell refresh). Never the damped load's sign — see
        // [`GRIP_ELEMENT_ENTER_FRACTION`].
        let elastic = membership_loads[c.element];
        let member = elastic >= enter || (elastic >= leave && elements.dwell[c.element] > 0);
        if member {
            elements.dwell[c.element] = GRIP_ELEMENT_LOSS_DWELL_TICKS;
            kept[c.element] = true;
        }
        if !c.has_plane || c.load_elastic <= 0.0 {
            continue;
        }
        // World-space shear rate in FORCE convention: the direction friction pushes the
        // hull (+slip_long along long_dir, −slip_lat along lat_dir — the kinetic law's
        // signs, vectorized).
        let sdot = c.long_dir * c.slip_long - c.lat_dir * c.slip_lat;
        if !advanced[c.element] {
            if loads[c.element] >= enter {
                let mean_slip = if counts[c.element] == 1 {
                    sdot
                } else {
                    slips[c.element] / loads[c.element]
                };
                elements.strain[c.element] =
                    advance_element_strain(elements.strain[c.element], mean_slip, c.normal, dt);
            }
            // Below the enter bound the existing strain holds; each patch's force still scales
            // with its own current elastic load, including its own local damping slip.
            advanced[c.element] = true;
        }
        let j1 = elements.strain[c.element];
        if c.mu <= 0.0 {
            // A frictionless pair has no grip budget, and its damping partner is unbounded.
            continue;
        }
        // The damping partner, normalized per unit budget: σ1 = 2ζ√(σ0·m) with the element's
        // stiffness σ0 = μN/K and mass share m = N/g, at the LuGre-canonical ζ and this
        // contact's own pair μ.
        let d_coef = 2.0 * GRIP_ELEMENT_DAMPING_RATIO / (GRIP_SHEAR_MODULUS_M * c.mu * 9.81).sqrt();
        let mut g = j1 / GRIP_SHEAR_MODULUS_M + sdot * d_coef;
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
        let elastic = membership_loads[el];
        if elastic >= enter || (elastic >= leave && elements.dwell[el] > 0) {
            elements.dwell[el] = GRIP_ELEMENT_LOSS_DWELL_TICKS;
            kept[el] = true;
        }
    }
    forget_departed_elements(&kept, elements);
    #[cfg(feature = "bitprobe")]
    for output in &mut report.bitprobe_elements {
        let element = output.material as usize * 3 + output.column as usize;
        output.strain = elements.strain[element];
        output.dwell = u32::from(elements.dwell[element]);
    }
    elem_g
}

/// PASS 2 — emit forces in the original per-contact order: support, then traction. Fills the
/// report's application list and contact telemetry, and returns `(summed element force, summed
/// longitudinal ground reaction)`.
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
            // Each element's own capped strain+damping direction, scaled by ITS elastic load
            // — isotropic, full μ in every direction. The kinetic regularizer is deliberately
            // ABSENT: near zero slip its slope is μN/slip_saturation — a ~270 kN·s/m explicit
            // damper per side that sits at the 64 Hz stability margin and rang the contact
            // modes. Under sustained slide the Dahl state saturates to the strain cap — the
            // saturated friction ellipse — so steady sliding converges; what changes is a
            // physical relaxation lag (~C/K of slip distance) in force DIRECTION during fast
            // slides, and fuller sub-saturation traction.
            let grip_el = c.mu * c.load_elastic;
            let g = elem_g[idx];
            f_long = grip_el * g.x;
            f_lat = grip_el * g.y;
            elem_sum += Vec2::new(f_long, f_lat);
            traction = c.long_dir * f_long + c.lat_dir * f_lat;
            report.apps.push(ForceApp {
                force: traction,
                point: c.p,
                collider: c.collider,
            });
            belt_reaction += f_long;
        }

        report.contacts.push(BeltContact {
            point: c.p,
            load: c.load,
            load_elastic: c.load_elastic,
            slip: c.slip_long,
            slip_lat: c.slip_lat,
            f_long,
            f_lat,
            normal: c.normal,
            traction,
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
    impl TerrainOracle for FlatGround {
        fn depth_along(&self, station: Vec3, _out: Vec3, reach: f32) -> f32 {
            (self.surface_y - station.y).min(reach)
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
    /// penetration remains — the exact one-tick dropout of netcode review defect 1.
    fn rig_params() -> ForceParams {
        ForceParams {
            face_offset: 0.025,
            free_travel: 0.0,
            support_stiffness_per_m: 2.0e6,
            support_damping_per_m: 1.0e5,
            engage_depth: 0.002,
            probe_reach: 0.5,
            slip_saturation: 0.5,
            inertia: 500.0,
            grip: Some(GripParams {
                track: crate::substances::contact_friction(0.9),
                ground: crate::substances::contact_friction(0.9),
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
        impl TerrainOracle for CountedGround {
            fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
                self.calls.set(self.calls.get() + 1);
                FlatGround { surface_y: 0.0 }.depth_along(station, out, reach)
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

    fn strain_sum(elems: &GripElements) -> f32 {
        elems.strain.iter().map(|j| j.length()).sum()
    }

    /// Terrain fills `y > under_y` — an overhang/bridge deck for the return-run gate test.
    /// Directional: only upward probes see it.
    struct Ceiling {
        under_y: f32,
    }
    impl TerrainOracle for Ceiling {
        fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
            if out.y <= 0.0 {
                return -reach;
            }
            (station.y - self.under_y).min(reach)
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

    /// Netcode review defect 1: one separating tick (damped load clipped to zero, elastic
    /// penetration intact) must NOT erase parked strain — the pre-fix law reset every
    /// element its damped-load contacts missed, which is unsafe for parking.
    #[test]
    fn element_strain_survives_damped_load_dropout() {
        let loop_pts = rect_loop(0.02);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        // Build presliding strain: hull creeping +z against a stationary belt.
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

    /// Netcode review defect 3 (hysteresis band): an EXISTING member whose elastic load fades
    /// into [leave, enter) — dwell still live, slip continuing — must HOLD its accumulated
    /// strain, not keep integrating it. Integration is gated on `load_elastic >= enter`, NOT on
    /// membership: writing the branch `if member` would let strain GROW in the band (the pad is
    /// a member there) while the departure tests stayed green. This pins that the branch
    /// is load-keyed.
    #[test]
    fn element_member_fading_into_band_holds_strain() {
        let params = rig_params();
        let side_weight = params.grip.unwrap().side_weight;
        let enter = side_weight * GRIP_ELEMENT_ENTER_FRACTION;
        let leave = side_weight * GRIP_ELEMENT_LEAVE_FRACTION;

        // Phase 1 — deep contact (loads ≫ enter), slipping: accumulate real presliding strain.
        let deep = rect_loop(0.02);
        let mut state = SideState::default();
        let mut elems = GripElements::for_links(25);
        let mut report = rig_tick(&deep, &mut state, &mut elems, 0.0, Vec3::ZERO);
        for _ in 0..20 {
            report = rig_tick(
                &deep,
                &mut state,
                &mut elems,
                0.0,
                Vec3::new(0.0, 0.0, 0.05),
            );
        }
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

    /// Netcode review defect 3: an element that never clears the enter bound must not
    /// integrate strain — a barely-touching pad accumulating full strain would fire a sudden
    /// force the moment real load arrives.
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
}
