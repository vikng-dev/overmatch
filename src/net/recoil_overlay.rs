//! The own hull's firing recoil, presented at the local fire tick instead of at the cursor.
//!
//! The owner's body is `RigidBody::Static` (`net::rig`), so the hull
//! impulse `shooting::fire` applies integrates nowhere client-side: the whole visible kick arrives
//! as replicated `Position`/`Rotation` on the interpolation buffer, `RTT/2 + D` after the click,
//! while the muzzle flash and the barrel spring fire on the local tick. This module puts the kick
//! back under the flash as a presentation-only offset on the rendered hull.
//!
//! # OVERLAY WHAT DECAYS TO ZERO; INTERPOLATE WHAT INTEGRATES
//!
//! An impulse response is a transient with no DC component, so a view-layer copy of it is
//! self-retiring and cannot accumulate a position error. Base motion is an integral and stays on
//! the stream. That line bounds this module to ONE member — own fire recoil. Being shot and ramming
//! are not own-initiated knowledge (`net::hit_feel` serves the first at message arrival); nothing
//! here generalizes to them.
//!
//! # RESPONSE REPLAY: THE SIM'S OWN RESPONSE, PLAYED AT THE CLICK, SUBTRACTED AT THE CURSOR
//!
//! At the fire the module computes the hull's full impulse response by MICROSIM — under the sim's
//! ACTUAL laws, not a simplified body. The basis is the newest CONFIRMED server state of the own
//! hull — `ConfirmedHistory::newest_present` for `Position`/`Rotation` (the snapshot buffer backing
//! the interpolated stream), the live-replicated `LinearVelocity`/`AngularVelocity`, and the
//! replicated belt state (`TrackDrive`/`TankTransmission`) — and the excitation is
//! `shooting::recoil_impulse`, the SAME expression the sim applies, resolved about the authored
//! centre of mass with the authored inertia (`I⁻¹ (r × J)`, Avian's own principal-frame tensor).
//! The basis is first walked forward through the confirmed-to-fire gap (the ~RTT of staleness the
//! client can close because it is integrating the same laws the server will), then TWO hulls are
//! integrated [`RESPONSE_TICKS`] fixed ticks — kicked and unkicked — each tick being the sim's own
//! tick: `track::sim::belt_tick` (the IDENTICAL function the live drive step runs — suspension
//! envelope, track grip, transmission, against the same terrain field) at `SimPhase::DrivingForces`,
//! the impulse's velocity change at `SimPhase::WeaponFire`, then Avian's substepped semi-implicit
//! integration with its own gyroscopic solve. The integration is AMORTIZED: the click's frame
//! computes only the basis walk plus a few content ticks, and `advance_recoil_microsim` extends
//! the pair just ahead of the played clock each frame, so the horizon's cost never lands as one
//! spike inside the fire tick. The stored trajectory is the difference,
//! `R(k) = kicked pose − unkicked pose`: the kick is ARRESTED by tracks and suspension exactly as
//! the server will arrest it, so the subtraction cancels down to what the client cannot know —
//! other-caused forces inside the staleness window and the private grip-strain seed
//! (an interpolated owner never receives the element field, ADR-0027; the microsim seeds a rest
//! slab, which both trajectories share, so the seed error largely cancels out of the difference).
//! That floor is what the certification capture measures.
//!
//! The displayed offset is one subtraction:
//!
//! ```text
//! offset = R(local clock − fire) − R(cursor − fire, clamped ≥ 0)
//! ```
//!
//! At the click the second term is zero, so the rendered hull shows the FULL kick, once. As the
//! interpolation cursor crosses the fire tick the stream begins delivering the true kick inside
//! the hull positions, and the second term subtracts the SAME stored trajectory back out —
//! cancellation by the sim's own arithmetic, never by a tuned envelope. Both clocks are held
//! locally: `ShotId::fire_tick` is the client's own `LocalTimeline` tick at the fire, and the
//! cursor is `InterpolationTimeline::tick()` + overstep — the same quantity `trace::record_frame`
//! emits as `itick` — so lightyear's ±5 % clock dilation and an interpolation clamp move both
//! terms and the subtraction tracks them.
//!
//! Content time carries a one-tick lead ([`content_time`]): the impulse lands before the fire
//! tick's own position integration, so the pose stamped `fire_tick` already holds the first
//! response step, and the stream starts blending it in one tick earlier.
//!
//! # THE TAIL HEALS THROUGH THE TRAJECTORY'S OWN END
//!
//! `R(a) − R(b)` at fixed lag does not vanish on its own — an impulse leaves a persistent velocity
//! change — so the trajectory CLAMPS at its horizon: past [`RESPONSE_TICKS`] the played term holds
//! `R(N)` while the delivered term walks the stored tail at the cursor's own rate, and the offset
//! reaches exactly zero the moment the cursor clears the response window. The heal window is
//! therefore the measured handoff span itself — a derived rate, where `track::view`'s
//! `PHASE_HEAL_OMEGA` needs a free one.
//!
//! # RESPONSE_TICKS IS THE ONE FLAGGED CONSTANT
//!
//! Everything else is derived: amplitude and direction from `recoil_impulse` and the authored mass
//! properties, the response shape from the sim's own laws, timing from the two observed clocks,
//! the heal rate from the handoff span. [`RESPONSE_TICKS`] alone is flagged — and even it is
//! derived from the authored suspension's settle law rather than tuned by eye (see its doc);
//! [`MICROSIM_FLOOR_M`] is the companion presentation threshold (the `ZERO_EPS_M` family) that
//! keeps sub-visual rounds from paying for a microsim at all.
//!
//! # SCOPE OF THE WRITE
//!
//! `Transform` only, never `Position`/`Rotation`: an entity-keyed presentation offset, a
//! world-space translation delta plus a body-local rotation delta applied on the right of the
//! simulated rotation, composed in `PostUpdate` onto a pose RE-DERIVED from `Position`/`Rotation`
//! rather than accumulated onto whatever `Transform` holds.
//!
//! Two ordering facts are load-bearing:
//!
//! - `apply_recoil_overlay` runs **after** `camera::OrbitCameraSet`: the recoil must be visible,
//!   so the third-person camera places itself from the un-rocked pose and the hull rocks inside
//!   the frame. There is no camera kick in this module.
//! - `track::view::TrackViewSet` orders after this set, so the belt and wheels are written from the
//!   same presented root pose the hull renders at.
//!
//! # A LOCAL FIRE THE SERVER REFUSED
//!
//! It can happen. `shooting::fire` gates on the client's own `WeaponGate`, which under this branch
//! is presentation state (`net::fire_presentation`), so a crew-paused reload the server applied is
//! invisible for `RTT/2`. The subtraction is clocked, not content-matched: for a refused fire the
//! delivered term still walks the stored trajectory while the stream carries no kick, so the hull
//! rocks and then un-rocks over the same window — a bounded phantom that ends at exactly zero, so
//! no confirmation gate is needed. (One is available if it ever is: the server's confirmation
//! arrives at `RTT/2` and the crossing at `RTT/2 + D`, so it provably lands first.)
//!
//! Rapid refire composes additively: each shot contributes its own response over its own window,
//! and the offsets sum.
//!
//! Design note: `.agents/scratch/impulse-prediction-mixed-timeline-2026-08-14.md`.

use avian3d::dynamics::integrator::solve_gyroscopic_torque;
use avian3d::prelude::{
    AngularInertia, AngularVelocity, CenterOfMass, ComputedAngularInertia, Gravity, LinearVelocity,
    Mass, PhysicsSystems, Position, Rotation, SubstepCount,
};
use bevy::math::Affine3A;
use bevy::prelude::*;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::{Interpolated, LocalTimeline, NetworkTimeline};

use super::protocol::NetTank;
use crate::ballistics::{FireShell, FireShellOrigin};
use crate::command::TankCommand;
use crate::shooting::recoil_impulse;
use crate::tank::Controlled;
use crate::track::drive::DriveAxes;
use crate::track::oracle::TerrainOracle;
use crate::track::sim::{
    BeltRig, TankTransmission, TrackDrive, TrackGear, TrackGrip, TrackGripElements, belt_tick,
};
use crate::track::terrain::TrackField;
use crate::track::transmission::{TransmissionMode, TransmissionState};

/// Below this the composed offset is treated as spent and zeroed, so it never lingers as denormal
/// dust and a root with nothing in flight keeps a `Transform` bit-identical to Avian's.
const ZERO_EPS_M: f32 = 1e-6;
const ZERO_EPS_RAD: f32 = 1e-6;

/// Microsim horizon in fixed ticks — the ONE flagged constant in this module (4.0 s at 64 Hz).
///
/// DERIVED from the settle law of the SLOWEST mode the faithful response actually rings in. The
/// vertical ride mode (`suspension: ride_frequency 1.2 Hz, damping_ratio 0.35`) settles in
/// `4 / (ζ ω_n) = 1.52 s`, but the kick's dominant content is the coupled surge–pitch mode on the
/// track-grip bristles, and the sim's own response (measured by this module's fidelity gates on
/// the calibrated fixture rig) rings there at ~0.64 Hz with ζ ≈ 0.25 — 2 % settling
/// `4 / (0.25 · 2π · 0.64 Hz) ≈ 4.0 s` = 256 ticks. Past the horizon the played term clamps at
/// `R(N)`, so the response must be SETTLED there (`Ṙ ≈ 0` — not necessarily zero: a kick may
/// leave a permanently different rest pose, and the tail-heal clamp handles a settled nonzero
/// tail exactly), or the freeze pops by `Ṙ(N) ×` one handoff span. Running much longer buys
/// nothing (R barely moves past its own settle) and stretches the heal tail. The horizon's cost
/// is amortized — [`advance_recoil_microsim`] integrates the pair just ahead of the played clock —
/// so its length prices frames at a constant few microsim ticks, never one spike at the click.
const RESPONSE_TICKS: usize = 256;

/// How far ahead of the played clock the amortized microsim keeps the stored response (content
/// ticks). Pure buffering headroom — covers a frame hitch's fixed-tick burst without the sample
/// clamping at a half-computed tail; not a feel constant.
const ADVANCE_MARGIN_TICKS: usize = 8;

/// Basis forward-integration bound (ticks). The confirmed basis normally trails the local fire
/// tick by RTT + interpolation delay (~10 ticks on the certified link); a gap past this bound
/// means the stream has stalled, and walking a dead basis further predicts nothing.
const MAX_BASIS_LEAD: u32 = 32;

/// Free-drift response ceiling (m) below which a shot stores NO kick: `|J|/M · N·dt` bounds every
/// displacement the response could ever show (drift with nothing arresting it — the arrested truth
/// is far smaller), so a shot whose CEILING is sub-millimetre is below the certification
/// instrument's own mm class and invisible at any plausible screen scale. This is what keeps the
/// ~12 Hz MG belt (impulse ~700× the 88's below) from paying a microsim per round; it also makes
/// overlapping in-flight responses second-order rare — of the shipped weapons only the 88 passes
/// this floor, so two live nonlinear responses can superpose only across one reload (3 s) inside
/// one horizon (4 s), where the older response has already rung down to millimetres.
const MICROSIM_FLOOR_M: f32 = 1e-3;

/// Everything one microsim tick needs beyond the body state, read once per shot: the sim's own
/// gear/terrain/integration inputs.
struct MicroCtx<'a, O: TerrainOracle> {
    gear: &'a TrackGear,
    oracle: &'a O,
    mode: TransmissionMode,
    /// Drive target held over the horizon (the own `TankCommand` at the fire). Both bodies share
    /// it, so its error against the player's future inputs cancels out of the difference to first
    /// order.
    target: DriveAxes,
    /// World centre of mass at the basis — [`MicroTank::com`] is an OFFSET from here. The
    /// integration state must live at small coordinates: a map-scale f32 com accumulates ~1 ulp
    /// of rounding per tick (measured ~3 mm over the horizon at z ≈ 290 m — the residual bar
    /// itself), while an offset keeps integration noise sub-micrometre. Terrain and force
    /// geometry still evaluate at true world coordinates (`origin + offset` — one rounding per
    /// read, never accumulated).
    origin: Vec3,
    gravity: Vec3,
    dt: f32,
    substeps: u32,
    mass: f32,
    /// Authored body-local centre of mass.
    local_com: Vec3,
    inertia: ComputedAngularInertia,
}

/// One microsim hull: rigid state about the centre of mass (the frame Avian's solver bodies
/// integrate in — `com` as an offset from [`MicroCtx::origin`]) plus the belt/drivetrain state
/// [`belt_tick`] advances.
#[derive(Clone, Debug)]
struct MicroTank {
    /// Centre-of-mass OFFSET from [`MicroCtx::origin`] (world axes).
    com: Vec3,
    rot: Quat,
    linear: Vec3,
    angular: Vec3,
    drive: TrackDrive,
    grip: TrackGrip,
    elements: TrackGripElements,
    transmission: TransmissionState,
}

impl MicroTank {
    /// One fixed tick in the sim's own order: `SimPhase::DrivingForces` — the shared
    /// [`belt_tick`], the IDENTICAL function the live drive step runs, at the pre-tick pose and
    /// velocity field — then `SimPhase::WeaponFire` (the kick's velocity change lands before any
    /// position integration), then Avian's tick: per-application force→acceleration in report
    /// order (`ForcesItem::apply_force_at_point`'s conversion, world inverse inertia at the
    /// pre-integration rotation) and the substepped semi-implicit Euler loop with Avian's own
    /// [`solve_gyroscopic_torque`]. The substep arithmetic is avian3d 0.7's version-pinned source
    /// expression (`integrate_velocities`/`integrate_positions` are query-bound systems and cannot
    /// run on plain state): increments computed once per tick, applied each substep, one
    /// renormalize per tick at writeback.
    ///
    /// `kick` is the shot's BODY-frame velocity delta pair from [`hull_response`], transferred at
    /// this body's own rotation.
    fn step<O: TerrainOracle>(&mut self, ctx: &MicroCtx<'_, O>, kick: Option<(Vec3, Vec3)>) {
        let world_com = ctx.origin + self.com;
        let position = world_com - self.rot * ctx.local_com;
        let affine = Affine3A::from_rotation_translation(self.rot, position);
        let (com, linear, angular) = (world_com, self.linear, self.angular);
        let (reports, _) = belt_tick(
            ctx.gear,
            ctx.oracle,
            ctx.mode,
            ctx.target,
            affine,
            ctx.dt,
            |p| linear + angular.cross(p - com),
            BeltRig {
                drive: &mut self.drive,
                grip: &mut self.grip,
                elements: &mut self.elements,
                transmission: &mut self.transmission,
            },
        );
        let inverse_inertia = ctx.inertia.inverse();
        let mut linear_acceleration = Vec3::ZERO;
        let mut angular_acceleration = Vec3::ZERO;
        for report in &reports {
            for app in &report.apps {
                linear_acceleration += app.force / ctx.mass;
                angular_acceleration += self.rot
                    * (inverse_inertia
                        * (self.rot.inverse() * (app.point - world_com).cross(app.force)));
            }
        }
        if let Some((surge, twist)) = kick {
            self.linear += self.rot * surge;
            self.angular += self.rot * twist;
        }
        let h = ctx.dt / ctx.substeps as f32;
        let linear_increment = (linear_acceleration + ctx.gravity) * h;
        let angular_increment = angular_acceleration * h;
        for _ in 0..ctx.substeps {
            self.linear += linear_increment;
            self.angular += angular_increment;
            solve_gyroscopic_torque(&mut self.angular, self.rot, &ctx.inertia, h);
            self.com += self.linear * h;
            self.rot = Quat::from_scaled_axis(self.angular * h) * self.rot;
        }
        self.rot = self.rot.normalize();
    }
}

/// The kicked/unkicked pair still integrating a response's tail, plus the per-shot ctx the
/// amortizing system needs to rebuild [`MicroCtx`] each frame.
#[derive(Clone, Debug)]
struct LivePair {
    clean: MicroTank,
    kicked: MicroTank,
    /// The shot's BODY-frame velocity deltas ([`hull_response`]), consumed inside the first
    /// kicked tick — after that tick's force evaluation (the sim's `DrivingForces` ran before
    /// `WeaponFire`), before its integration.
    kick: Option<(Vec3, Vec3)>,
    /// The shot's world basis origin — see [`MicroCtx::origin`].
    origin: Vec3,
    /// The drive target held over the horizon — see [`MicroCtx::target`].
    target: DriveAxes,
    /// Accumulated microsim wall time, reported once at finalization.
    spent: core::time::Duration,
}

impl LivePair {
    /// Extend the stored difference through content tick `upto` (clamped to the horizon):
    /// `R(k) = (world centre-of-mass delta, body-local rotation delta)`. Both trajectories
    /// integrate OFFSETS from the shot's basis origin so the millimetre-scale difference never
    /// fights map-scale f32 ulps; the terrain field is position-dependent, so force geometry
    /// still reads true world coordinates.
    fn advance<O: TerrainOracle>(
        &mut self,
        ctx: &MicroCtx<'_, O>,
        response: &mut Vec<(Vec3, Quat)>,
        upto: usize,
    ) {
        let upto = upto.min(RESPONSE_TICKS);
        while response.len() <= upto {
            self.clean.step(ctx, None);
            self.kicked.step(ctx, self.kick.take());
            response.push((
                self.kicked.com - self.clean.com,
                (self.clean.rot.inverse() * self.kicked.rot).normalize(),
            ));
        }
    }
}

/// Walk the basis forward through the confirmed-to-fire gap, then integrate the full response in
/// one call — the whole-horizon form the fidelity gates exercise; production amortizes the same
/// [`LivePair::advance`] across frames.
#[cfg(test)]
fn impulse_response<O: TerrainOracle>(
    mut basis: MicroTank,
    ctx: &MicroCtx<'_, O>,
    lead: u32,
    surge: Vec3,
    twist: Vec3,
) -> Vec<(Vec3, Quat)> {
    for _ in 0..lead {
        basis.step(ctx, None);
    }
    let mut pair = LivePair {
        clean: basis.clone(),
        kicked: basis,
        kick: Some((surge, twist)),
        origin: ctx.origin,
        target: ctx.target,
        spent: core::time::Duration::ZERO,
    };
    let mut response = vec![(Vec3::ZERO, Quat::IDENTITY)];
    pair.advance(ctx, &mut response, RESPONSE_TICKS);
    response
}

/// The confirmed-to-fire gap in whole ticks, clamped to `[0, MAX_BASIS_LEAD]` — how far
/// [`impulse_response`] walks the basis before the kick. Both clocks are the shared tick space the
/// subtraction law already compares (`fire_tick` vs the interpolation cursor).
fn basis_lead(fire_tick: u32, confirmed_tick: f64) -> u32 {
    (f64::from(fire_tick) - confirmed_tick).clamp(0.0, f64::from(MAX_BASIS_LEAD)) as u32
}

/// Sample the stored response at a fractional tick, clamped to `[0, RESPONSE_TICKS]`.
///
/// Piecewise-linear between entries — the same structure lightyear's interpolation applies to the
/// arriving keyframes, so the delivered term subtracts what the stream is actually showing
/// mid-tick. The clamp at the horizon is the tail-heal law: see the module doc.
fn sample(response: &[(Vec3, Quat)], at: f64) -> (Vec3, Quat) {
    let at = at.clamp(0.0, (response.len() - 1) as f64);
    let index = at.floor() as usize;
    let frac = (at - at.floor()) as f32;
    if frac == 0.0 || index + 1 >= response.len() {
        return response[index];
    }
    let (near_com, near_rot) = response[index];
    let (far_com, far_rot) = response[index + 1];
    (near_com.lerp(far_com, frac), near_rot.slerp(far_rot, frac))
}

/// A clock reading converted to response content time, in ticks.
///
/// The `+ 1`: the impulse lands before the fire tick's own position integration
/// (`SimPhase::WeaponFire` precedes the solver), so the pose STAMPED `fire_tick` already holds one
/// step of response — content time 1 — and the stream starts blending it in one tick earlier.
fn content_time(fire_tick: u32, clock: f64) -> f64 {
    clock - f64::from(fire_tick) + 1.0
}

/// One shot's stored impulse response, in flight until the cursor clears its horizon.
#[derive(Debug)]
struct RecoilKick {
    /// The client `LocalTimeline` tick the shot was fired on (`ShotId::fire_tick`).
    fire_tick: u32,
    /// `R(k)` for `k = 0..=` the last computed content tick (grows to `RESPONSE_TICKS`).
    response: Vec<(Vec3, Quat)>,
    /// The pair still integrating the tail — `None` once `R` reaches the horizon.
    live: Option<Box<LivePair>>,
}

/// Presentation-only recoil offset composed onto the own interpolated root.
///
/// Translation is a world-space additive delta of the CENTRE OF MASS; rotation is a body-local delta
/// applied on the right of the simulated rotation. [`apply_recoil_overlay`] derives the root
/// origin's displacement from the two.
#[derive(Component, Default, Debug)]
pub struct RecoilOverlay {
    kicks: Vec<RecoilKick>,
    /// Composed world-space surge of the centre of mass this frame.
    pub translation: Vec3,
    /// Composed body-local rock this frame.
    pub rotation: Quat,
}

/// Ordering owner for the recoil overlay: after the camera has read the un-rocked pose, before
/// propagation carries the rocked one out.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecoilOverlayApplied;

/// Install the own-hull recoil overlay.
pub fn plugin(app: &mut App) {
    app.add_systems(Update, arm_recoil_overlay);
    app.add_observer(excite_recoil_overlay);
    app.add_systems(
        PostUpdate,
        // The amortized tail integration serves this frame's sample, so it runs before the apply.
        advance_recoil_microsim.before(RecoilOverlayApplied),
    );
    app.add_systems(
        PostUpdate,
        apply_recoil_overlay
            .in_set(RecoilOverlayApplied)
            .after(PhysicsSystems::Writeback)
            // THE "no camera kick" EDGE: the third-person camera must place itself from the
            // un-rocked pose.
            .after(crate::camera::OrbitCameraSet)
            .before(TransformSystems::Propagate),
    );
    // The belt and wheels are written FROM the presented root pose, so they must read the rocked
    // one. The edge lives here because the net-boundary guard keeps `track::view` from naming the
    // netcode.
    app.configure_sets(
        PostUpdate,
        crate::track::view::TrackViewSet.after(RecoilOverlayApplied),
    );
}

/// Arm the own tank once the server stream — not local physics — owns its hull.
///
/// The mass-property requirements are the excitation's inputs: a root that has not finished
/// construction cannot yet derive a response.
#[expect(clippy::type_complexity, reason = "one arming predicate, spelled out")]
fn arm_recoil_overlay(
    // Fused own fire presents the kick from the stream itself, at the cursor — there is no early
    // presentation to cancel, so the overlay must not arm. Read at arming: with the lever unset the
    // resource is absent and this system is bit-identical to before the lever existed.
    fused: Option<Res<crate::FusedOwnFire>>,
    tanks: Query<
        Entity,
        (
            With<NetTank>,
            With<Controlled>,
            With<Interpolated>,
            With<Mass>,
            With<AngularInertia>,
            With<CenterOfMass>,
            Without<RecoilOverlay>,
            Without<ChildOf>,
        ),
    >,
    mut commands: Commands,
) {
    if fused.is_some() {
        return;
    }
    for entity in &tanks {
        info!("net: {entity} own interpolated hull armed with the fire-recoil overlay");
        commands.entity(entity).insert(RecoilOverlay::default());
    }
}

/// The interpolation cursor in fractional ticks — `trace`'s `itick`, and the delivered term's only
/// clock. `f64` because a session's tick counter outgrows `f32`'s sub-tick resolution.
fn cursor_ticks(timeline: &InterpolationTimeline) -> f64 {
    f64::from(timeline.tick().0) + f64::from(timeline.overstep().to_f32())
}

/// One kick's contribution at the two clock readings: `(surge, twist, spent)`.
///
/// The played term samples at local content time, the delivered term at cursor content time
/// (clamped ≥ 0 — before the crossing nothing has arrived), and the offset is their difference:
/// world translation subtracted directly, body-local rotation composed as `D(b)⁻¹ · D(a)` and
/// returned as a rotation vector so refire composes additively. Spent — identically zero, forever —
/// once the cursor clears the response horizon; a still-integrating response is never spent (its
/// stored end is a compute boundary, not the horizon).
fn kick_offset(kick: &RecoilKick, local_clock: f64, cursor: f64) -> (Vec3, Vec3, bool) {
    let delivered = content_time(kick.fire_tick, cursor);
    if kick.live.is_none() && delivered >= (kick.response.len() - 1) as f64 {
        return (Vec3::ZERO, Vec3::ZERO, true);
    }
    let played = content_time(kick.fire_tick, local_clock);
    let (played_com, played_rot) = sample(&kick.response, played);
    let (delivered_com, delivered_rot) = sample(&kick.response, delivered.max(0.0));
    (
        played_com - delivered_com,
        (delivered_rot.inverse() * played_rot).to_scaled_axis(),
        false,
    )
}

/// The hull's velocity response to one shot: `(surge, twist)` from the sim's own recoil impulse.
///
/// `surge` is world-frame linear velocity; `twist` is BODY-frame angular velocity — the moment arm
/// is body-fixed geometry, read at the pose the muzzle point was computed in. Working the arm in
/// the body frame is what keeps the inertia tensor un-rotated: `I_world = R I R⁻¹`, so
/// `R⁻¹ I_world⁻¹ = I⁻¹ R⁻¹`.
fn hull_response(
    impulse: Vec3,
    muzzle: Vec3,
    position: Vec3,
    rotation: Quat,
    mass: &Mass,
    inertia: &AngularInertia,
    center_of_mass: &CenterOfMass,
) -> (Vec3, Vec3) {
    if mass.0 <= 0.0 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let inverse_rotation = rotation.inverse();
    let center = position + rotation * center_of_mass.0;
    let arm = inverse_rotation * (muzzle - center);
    let local_impulse = inverse_rotation * impulse;
    // Avian's own principal-frame-to-tensor conversion, so the overlay inverts the inertia the
    // solver would have.
    let tensor =
        ComputedAngularInertia::new_with_local_frame(inertia.principal, inertia.local_frame);
    (
        impulse / mass.0,
        tensor.inverse() * arm.cross(local_impulse),
    )
}

/// Admit one locally-fired shot: microsim the response from the newest confirmed basis under the
/// sim's own laws, store it.
///
/// Basis inventory, honestly stated: the interpolated own hull's `Position`/`Rotation` come from
/// the confirmed snapshot buffer; `LinearVelocity`/`AngularVelocity`, `TrackDrive`, and
/// `TankTransmission` are plain-replicated (their live values ARE the newest confirmed).
/// `TrackGripElements` — the private per-element strain field — is NOT streamed to an interpolated
/// replica (ADR-0027): the microsim seeds the replicate-once snapshot when one exists, else the
/// rest slab. Both trajectories share that seed, so its error largely cancels out of the
/// difference; what survives (strain near saturation — parked on a steep slope) is part of the
/// measured residual floor.
#[expect(clippy::type_complexity, reason = "one basis read, spelled out")]
fn excite_recoil_overlay(
    fire: On<FireShell>,
    cursors: Query<&InterpolationTimeline>,
    tick: Res<TickDuration>,
    gravity: Res<Gravity>,
    substeps: Res<SubstepCount>,
    gear: Option<Res<TrackGear>>,
    field: Option<Res<TrackField>>,
    mut roots: Query<(
        &Position,
        &Rotation,
        &Mass,
        &AngularInertia,
        &CenterOfMass,
        Option<&ConfirmedHistory<Position>>,
        Option<&ConfirmedHistory<Rotation>>,
        Option<&LinearVelocity>,
        Option<&AngularVelocity>,
        Option<&TrackDrive>,
        Option<&TankTransmission>,
        Option<&TrackGripElements>,
        Option<&TankCommand>,
        &mut RecoilOverlay,
    )>,
) {
    // A reconstructed opponent shot is somebody else's hull; a sandbox free-fly shot has no shooter;
    // a slot too wide for the wire has no `ShotId` and therefore no fire tick to hand off at. The
    // query supplies the last filter: only the armed own root matches.
    if fire.shot_origin != FireShellOrigin::Local {
        return;
    }
    let (Some(source), Some(shot)) = (fire.shooter, fire.shot) else {
        return;
    };
    let Ok((
        position,
        rotation,
        mass,
        inertia,
        center_of_mass,
        confirmed_position,
        confirmed_rotation,
        linear,
        angular,
        drive,
        transmission,
        elements,
        command,
        mut overlay,
    )) = roots.get_mut(source.tank)
    else {
        return;
    };
    let Ok(timeline) = cursors.single() else {
        return;
    };
    // The cursor is already at or past the fire tick: the stream is about to carry the kick itself,
    // and there is no window to fill.
    if f64::from(shot.fire_tick) - cursor_ticks(timeline) <= 0.0 {
        return;
    }
    let impulse = recoil_impulse(fire.direction, fire.mass, fire.speed);
    // Sub-visual shots store nothing: the free-drift ceiling bounds every reachable displacement.
    if impulse.length() / mass.0.max(f32::EPSILON) * RESPONSE_TICKS as f32 * tick.0.as_secs_f32()
        < MICROSIM_FLOOR_M
    {
        return;
    }
    // The microsim needs the sim's own gear and terrain field; without them (a client still
    // loading) there is no faithful response, and an unfaithful one is the defect this module
    // replaced — present no kick.
    let (Some(gear), Some(oracle)) = (
        gear.as_deref(),
        field.as_deref().and_then(|field| field.field.as_ref()),
    ) else {
        bevy::log::warn_once!(
            "net: recoil overlay fired before TrackGear/terrain field — no kick presented"
        );
        return;
    };
    // The moment arm is read at the live pose — the frame `fire.origin` was computed in — and both
    // deltas are carried BODY-frame, so they transfer to the microsim body's own rotation at the
    // kick tick.
    let (surge, twist) = hull_response(
        impulse,
        fire.origin,
        position.0,
        rotation.0,
        mass,
        inertia,
        center_of_mass,
    );
    let surge = rotation.0.inverse() * surge;
    // The microsim basis: newest CONFIRMED pose (the snapshot buffer the stream interpolates), the
    // plain-replicated velocities and belt state (their live values ARE the newest confirmed), the
    // best available element seed. A history still warming up falls back to the live pose.
    let confirmed = confirmed_position.and_then(ConfirmedHistory::newest_present);
    let basis_position = confirmed.map_or(position.0, |(_, confirmed)| confirmed.0);
    let basis_rotation = confirmed_rotation
        .and_then(ConfirmedHistory::newest_present)
        .map_or(rotation.0, |(_, confirmed)| confirmed.0);
    let basis = MicroTank {
        com: Vec3::ZERO,
        rot: basis_rotation,
        linear: linear.map_or(Vec3::ZERO, |velocity| velocity.0),
        angular: angular.map_or(Vec3::ZERO, |velocity| velocity.0),
        drive: drive.copied().unwrap_or_default(),
        grip: TrackGrip::default(),
        elements: elements
            .cloned()
            .unwrap_or_else(|| TrackGripElements::for_links(gear.link_count())),
        transmission: transmission.map_or_else(
            || {
                gear.trans()
                    .map_or_else(TankTransmission::for_governor, TankTransmission::from_spec)
                    .0
            },
            |state| state.0,
        ),
    };
    let ctx = MicroCtx {
        gear,
        oracle,
        mode: gear.mode(),
        origin: basis_position + basis_rotation * center_of_mass.0,
        target: command.map_or(
            DriveAxes {
                throttle: basis.drive.throttle,
                steer: basis.drive.steer,
            },
            |command| DriveAxes {
                throttle: command.throttle,
                steer: command.steer,
            },
        ),
        gravity: gravity.0,
        dt: tick.0.as_secs_f32(),
        substeps: substeps.0,
        mass: mass.0,
        local_com: center_of_mass.0,
        inertia: ComputedAngularInertia::new_with_local_frame(
            inertia.principal,
            inertia.local_frame,
        ),
    };
    let lead = confirmed.map_or(0, |(tick, _)| basis_lead(shot.fire_tick, f64::from(tick.0)));
    let start = std::time::Instant::now();
    let mut basis = basis;
    for _ in 0..lead {
        basis.step(&ctx, None);
    }
    let mut pair = LivePair {
        clean: basis.clone(),
        kicked: basis,
        kick: Some((surge, twist)),
        origin: ctx.origin,
        target: ctx.target,
        spent: core::time::Duration::ZERO,
    };
    // The click's own frame needs only the first response steps; the rest of the horizon is
    // amortized by `advance_recoil_microsim`, so the fire tick never pays a horizon-length spike.
    let mut response = vec![(Vec3::ZERO, Quat::IDENTITY)];
    pair.advance(&ctx, &mut response, ADVANCE_MARGIN_TICKS);
    pair.spent = start.elapsed();
    // Per-shot cost is a certified number: the click-frame slice here, the amortized total at
    // finalization (`advance_recoil_microsim`).
    info!(
        "net: recoil response replay armed — fire tick {}, confirmed basis tick {:?}, lead {} \
         ticks, click-frame microsim {} ticks in {:?}",
        shot.fire_tick,
        confirmed.map(|(tick, _)| tick.0),
        lead,
        lead as usize + 2 * ADVANCE_MARGIN_TICKS,
        pair.spent,
    );
    overlay.kicks.push(RecoilKick {
        fire_tick: shot.fire_tick,
        response,
        live: Some(Box::new(pair)),
    });
}

/// Amortize the microsim across frames: every armed root's still-integrating responses extend to
/// stay [`ADVANCE_MARGIN_TICKS`] ahead of the played clock, finalizing at the horizon. Steady
/// state this is ~2 content ticks (4 microsim ticks) per frame per live kick — the whole-horizon
/// cost spread over the response's own playback instead of one spike inside the fire tick.
fn advance_recoil_microsim(
    tick: Res<TickDuration>,
    gravity: Res<Gravity>,
    substeps: Res<SubstepCount>,
    gear: Option<Res<TrackGear>>,
    field: Option<Res<TrackField>>,
    local: Res<LocalTimeline>,
    fixed: Res<Time<Fixed>>,
    mut roots: Query<(&Mass, &AngularInertia, &CenterOfMass, &mut RecoilOverlay)>,
) {
    let (Some(gear), Some(oracle)) = (
        gear.as_deref(),
        field.as_deref().and_then(|field| field.field.as_ref()),
    ) else {
        return;
    };
    let local_clock = f64::from(local.tick().0) + f64::from(fixed.overstep_fraction());
    for (mass, inertia, center_of_mass, mut overlay) in &mut roots {
        for kick in &mut overlay.kicks {
            let Some(pair) = kick.live.as_deref_mut() else {
                continue;
            };
            let played = content_time(kick.fire_tick, local_clock).max(0.0) as usize;
            let start = std::time::Instant::now();
            let ctx = MicroCtx {
                gear,
                oracle,
                mode: gear.mode(),
                target: pair.target,
                origin: pair.origin,
                gravity: gravity.0,
                dt: tick.0.as_secs_f32(),
                substeps: substeps.0,
                mass: mass.0,
                local_com: center_of_mass.0,
                inertia: ComputedAngularInertia::new_with_local_frame(
                    inertia.principal,
                    inertia.local_frame,
                ),
            };
            pair.advance(&ctx, &mut kick.response, played + ADVANCE_MARGIN_TICKS);
            pair.spent += start.elapsed();
            if kick.response.len() > RESPONSE_TICKS {
                info!(
                    "net: recoil response finalized — fire tick {}, {} content ticks, total \
                     microsim wall time {:?}",
                    kick.fire_tick,
                    kick.response.len() - 1,
                    pair.spent,
                );
                kick.live = None;
            }
        }
    }
}

/// Compose every in-flight response and present it, re-derived from the sim pose.
fn apply_recoil_overlay(
    cursors: Query<&InterpolationTimeline>,
    local: Res<LocalTimeline>,
    fixed: Res<Time<Fixed>>,
    mut roots: Query<(
        &mut Transform,
        &Position,
        &Rotation,
        &CenterOfMass,
        &mut RecoilOverlay,
    )>,
) {
    let Ok(timeline) = cursors.single() else {
        return;
    };
    let cursor = cursor_ticks(timeline);
    // The played term's clock: the local timeline in fractional ticks — the clock `fire_tick` was
    // stamped on, carried between fixed steps by the fixed accumulator's overstep.
    let local_clock = f64::from(local.tick().0) + f64::from(fixed.overstep_fraction());

    for (mut transform, position, rotation, center_of_mass, mut overlay) in &mut roots {
        let mut surge = Vec3::ZERO;
        let mut twist = Vec3::ZERO;
        // A spent kick contributes exactly zero, so summing and retiring in one pass cannot change
        // the composed offset.
        overlay.kicks.retain(|kick| {
            let (kick_surge, kick_twist, spent) = kick_offset(kick, local_clock, cursor);
            surge += kick_surge;
            twist += kick_twist;
            !spent
        });
        if surge.length() <= ZERO_EPS_M {
            surge = Vec3::ZERO;
        }
        if twist.length() <= ZERO_EPS_RAD {
            twist = Vec3::ZERO;
        }
        overlay.translation = surge;
        overlay.rotation = if twist == Vec3::ZERO {
            Quat::IDENTITY
        } else {
            Quat::from_scaled_axis(twist)
        };

        // Avian's `position_to_transform` root branch plus the offset, written through `set_if_neq`
        // so a spent overlay neither writes nor dirties the root — which would propagate through the
        // tank's ~194 link children for no visual difference. The zero case is spelled out rather
        // than folded into the arithmetic so the written value is BIT-IDENTICAL to Avian's, which is
        // what makes the comparison skip.
        let clean_translation = position.0;
        let clean_rotation = rotation.0;
        let presented = if surge == Vec3::ZERO && twist == Vec3::ZERO {
            Transform {
                translation: clean_translation,
                rotation: clean_rotation,
                scale: transform.scale,
            }
        } else {
            // The rock pivots about the CENTRE OF MASS, not the root origin: holding the centre
            // fixed under the body-local rotation `O` moves the origin by `R (c − O c)`.
            let pivot = center_of_mass.0 - overlay.rotation * center_of_mass.0;
            Transform {
                translation: clean_translation + surge + clean_rotation * pivot,
                rotation: (clean_rotation * overlay.rotation).normalize(),
                scale: transform.scale,
            }
        };
        transform.set_if_neq(presented);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::track::forces::{ForceParams, grip_stiffness};

    /// The 88 mm round and the Tiger hull, from `assets/tiger_1/tiger_1.tank.ron`.
    const SHELL_MASS_KG: f32 = 10.2;
    const MUZZLE_SPEED_MPS: f32 = 773.0;
    const HULL_MASS_KG: f32 = 57_000.0;
    /// A muzzle 1.5 m above and 3 m ahead of the centre of mass — the geometry that turns the
    /// bore-axis impulse into gun climb.
    const MUZZLE_LIFT_M: f32 = 1.5;
    const MUZZLE_REACH_M: f32 = 3.0;
    /// The fixed tick, in seconds (64 Hz), and Avian's default substep count.
    const TICK_SECS: f32 = 1.0 / 64.0;
    const SUBSTEPS: u32 = 6;
    const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);
    /// The fixture rig's authored ride model — the same pair the real spec authors.
    const RIDE_HZ: f32 = 1.2;
    const RIDE_ZETA: f32 = 0.35;
    /// The fixture rig's station count and contacting belt length (both sides, m).
    const FIXTURE_LINKS: usize = 25;
    const FIXTURE_CONTACT_M: f32 = 8.0;

    fn tiger_inertia() -> AngularInertia {
        AngularInertia::from_shape(&Cuboid::new(3.7, 2.0, 6.3), HULL_MASS_KG)
    }

    /// Fire level, down `-Z`, from a muzzle above and ahead of a centre of mass at the origin, with
    /// the whole rig rotated by `rotation`.
    fn level_shot(rotation: Quat) -> (Vec3, Vec3) {
        let impulse = recoil_impulse(Dir3::NEG_Z, SHELL_MASS_KG, MUZZLE_SPEED_MPS);
        let muzzle = Vec3::new(0.0, MUZZLE_LIFT_M, -MUZZLE_REACH_M);
        hull_response(
            rotation * impulse,
            rotation * muzzle,
            Vec3::ZERO,
            rotation,
            &Mass(HULL_MASS_KG),
            &tiger_inertia(),
            &CenterOfMass(Vec3::ZERO),
        )
    }

    /// Empty space — every probe misses. Under this oracle the full microsim must REDUCE to the
    /// free-body impulse laws, which is what makes them testable against closed forms.
    struct NoGround;
    impl TerrainOracle for NoGround {
        fn depth_along(&self, _station: Vec3, _out: Vec3, reach: f32) -> f32 {
            -reach
        }
    }

    /// Flat horizontal ground filling `y < surface_y` — the `track::forces` fixture's shape.
    struct FlatGround {
        surface_y: f32,
    }
    impl TerrainOracle for FlatGround {
        fn depth_along(&self, station: Vec3, _out: Vec3, reach: f32) -> f32 {
            (self.surface_y - station.y).min(reach)
        }
    }

    /// A synthetic rig wearing the real [`TrackGear`] type: a 4 m bottom run 1 m under the hull
    /// origin, 25 stations, with the support law sized to the certified ride model on the
    /// fixture's contacting belt — `k = M ω_n² / L`, `c = 2 ζ √(k_total M) / L` — the exact
    /// relation `envelope::calibrate` derives on the measured rig (ride 1.2 Hz, ζ 0.35).
    fn test_gear() -> TrackGear {
        let omega = core::f32::consts::TAU * RIDE_HZ;
        let stiffness_total = HULL_MASS_KG * omega * omega;
        let damping_total = 2.0 * RIDE_ZETA * (stiffness_total * HULL_MASS_KG).sqrt();
        TrackGear::test_fixture(
            vec![
                Vec2::new(-2.0, -1.0),
                Vec2::new(2.0, -1.0),
                Vec2::new(2.0, -0.4),
                Vec2::new(-2.0, -0.4),
                Vec2::new(-2.0, -1.0),
            ],
            FIXTURE_LINKS,
            1.5,
            [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)],
            Vec::new(),
            ForceParams {
                face_offset: 0.025,
                free_travel: 0.0,
                support_stiffness_per_m: stiffness_total / FIXTURE_CONTACT_M,
                support_damping_per_m: damping_total / FIXTURE_CONTACT_M,
                engage_depth: 0.02,
                probe_reach: 0.5,
                mu: 0.9,
                slip_saturation: 0.4,
                max_speed: 10.0,
                engine_power: 1.0e5,
                engine_force: 1.0e5,
                governor_gain: 1.0e4,
                inertia: 500.0,
                grip_stiffness: grip_stiffness(0.9, HULL_MASS_KG * 9.81),
            },
        )
    }

    /// The rolling-basis tests' map-scale world origin — nothing about the response laws may
    /// depend on the shot happening near the world origin.
    const MAP_ORIGIN: Vec3 = Vec3::new(140.0, 6.6, 290.0);

    fn micro_ctx<'a, O: TerrainOracle>(
        gear: &'a TrackGear,
        oracle: &'a O,
        inertia: &AngularInertia,
        origin: Vec3,
    ) -> MicroCtx<'a, O> {
        MicroCtx {
            gear,
            oracle,
            mode: TransmissionMode::Governor,
            target: DriveAxes::default(),
            origin,
            gravity: GRAVITY,
            dt: TICK_SECS,
            substeps: SUBSTEPS,
            mass: HULL_MASS_KG,
            local_com: Vec3::ZERO,
            inertia: ComputedAngularInertia::new_with_local_frame(
                inertia.principal,
                inertia.local_frame,
            ),
        }
    }

    fn micro_tank(rot: Quat, linear: Vec3, angular: Vec3) -> MicroTank {
        MicroTank {
            com: Vec3::ZERO,
            rot,
            linear,
            angular,
            drive: TrackDrive::default(),
            grip: TrackGrip::default(),
            elements: TrackGripElements::for_links(FIXTURE_LINKS),
            transmission: TankTransmission::for_governor().0,
        }
    }

    /// A moving, spinning basis on a gravity world — nothing about the response laws below may
    /// depend on any of it being zero. Paired with [`MAP_ORIGIN`] in the ctx.
    fn rolling_basis() -> MicroTank {
        micro_tank(
            Quat::from_rotation_y(0.4),
            Vec3::new(1.0, 0.2, -3.0),
            Vec3::new(0.05, -0.3, 0.02),
        )
    }

    /// Settle the fixture hull onto flat ground under the microsim's own laws — the shared basis
    /// for the fidelity gates (384 ticks ≈ 4 settle windows from a near-equilibrium drop at an
    /// origin 0.86 m above the surface).
    fn settled_on_flat<O: TerrainOracle>(ctx: &MicroCtx<'_, O>) -> MicroTank {
        let mut tank = micro_tank(Quat::IDENTITY, Vec3::ZERO, Vec3::ZERO);
        for _ in 0..384 {
            tank.step(ctx, None);
        }
        tank
    }

    /// A free-space kick for the subtraction-law tests: the semantics under test are the clocked
    /// offsets, so any smooth nonzero response serves.
    fn kick(fire_tick: u32, surge: Vec3, spin: Vec3) -> RecoilKick {
        let gear = test_gear();
        let ctx = micro_ctx(&gear, &NoGround, &tiger_inertia(), MAP_ORIGIN);
        RecoilKick {
            fire_tick,
            response: impulse_response(rolling_basis(), &ctx, 0, surge, spin),
            live: None,
        }
    }

    /// AMPLITUDE IS THE SHELL'S MOMENTUM OVER THE HULL'S MASS, and nothing else. A feel constant
    /// anywhere in the chain moves this number off `m·v/M`.
    #[test]
    fn the_surge_is_the_shells_momentum_divided_by_the_hull_mass() {
        let (surge, _) = level_shot(Quat::IDENTITY);
        let expected = SHELL_MASS_KG * MUZZLE_SPEED_MPS / HULL_MASS_KG;
        assert!(
            (surge.length() - expected).abs() < 1e-6,
            "surge {} m/s, expected {expected} m/s",
            surge.length(),
        );
        // Opposite the bore: the gun points down -Z, so the hull goes +Z.
        assert!(
            surge.normalize().dot(Vec3::Z) > 1.0 - 1e-6,
            "the hull must recoil opposite the bore, got {surge:?}",
        );
    }

    /// A ROUND 700x LIGHTER KICKS 700x LESS, with no per-weapon term anywhere: the MGs fall out of
    /// the same expression the 88 does.
    #[test]
    fn a_lighter_round_scales_the_response_by_its_own_momentum() {
        let muzzle = Vec3::new(0.0, MUZZLE_LIFT_M, -MUZZLE_REACH_M);
        let response = |mass: f32, speed: f32| {
            hull_response(
                recoil_impulse(Dir3::NEG_Z, mass, speed),
                muzzle,
                Vec3::ZERO,
                Quat::IDENTITY,
                &Mass(HULL_MASS_KG),
                &tiger_inertia(),
                &CenterOfMass(Vec3::ZERO),
            )
        };
        let (main_surge, main_twist) = response(SHELL_MASS_KG, MUZZLE_SPEED_MPS);
        let (mg_surge, mg_twist) = response(0.0125, 760.0);
        let ratio = (SHELL_MASS_KG * MUZZLE_SPEED_MPS) / (0.0125 * 760.0);
        assert!(
            (main_surge.length() / mg_surge.length() - ratio).abs() < 1e-2,
            "surge ratio {}, expected {ratio}",
            main_surge.length() / mg_surge.length(),
        );
        assert!(
            (main_twist.length() / mg_twist.length() - ratio).abs() < 1e-2,
            "twist ratio {}, expected {ratio}",
            main_twist.length() / mg_twist.length(),
        );
    }

    /// THE ROCK IS NOSE-UP. A muzzle above the centre of mass turns a rearward impulse into gun
    /// climb; the body-local twist is a pitch about +X with the nose rising.
    #[test]
    fn a_muzzle_above_the_centre_of_mass_pitches_the_nose_up() {
        let (_, twist) = level_shot(Quat::IDENTITY);
        assert!(
            twist.x > 0.0,
            "the recoil must pitch the nose up, got {twist:?}",
        );
        assert!(
            twist.yz().length() < twist.x.abs() * 1e-3,
            "a bore-axis impulse on a level gun produces pitch alone, got {twist:?}",
        );
        // The lever is the muzzle's HEIGHT over the centre of mass, not its reach: the impulse is
        // parallel to the reach, so that component contributes no moment.
        let expected =
            SHELL_MASS_KG * MUZZLE_SPEED_MPS * MUZZLE_LIFT_M / tiger_inertia().principal.x;
        assert!(
            (twist.x - expected).abs() < 1e-4,
            "twist {} rad/s, expected {expected} rad/s",
            twist.x,
        );
    }

    /// The response is expressed in the BODY frame, so a yawed hull firing along its own bore
    /// produces the identical twist and a world surge rotated with it.
    #[test]
    fn the_twist_is_body_local_and_the_surge_is_world() {
        let (level_surge, level_twist) = level_shot(Quat::IDENTITY);
        let yaw = Quat::from_rotation_y(core::f32::consts::FRAC_PI_2);
        let (yawed_surge, yawed_twist) = level_shot(yaw);
        assert!(
            (yawed_twist - level_twist).length() < 1e-4,
            "body-frame twist must not depend on the hull's heading: {level_twist:?} vs \
             {yawed_twist:?}",
        );
        assert!(
            (yawed_surge - yaw * level_surge).length() < 1e-4,
            "the world surge must rotate with the hull: {yawed_surge:?}",
        );
    }

    /// THE MICROSIM REDUCES TO THE FREE-BODY IMPULSE LAW IN EMPTY SPACE: with no ground in reach
    /// the stored difference is the kick velocity integrated tick by tick, and gravity —
    /// integrated into both bodies by the same substep loop — cancels out of it. Applying gravity
    /// to one body only, or scaling the surge anywhere in the chain, moves every entry off
    /// `surge · k · dt`. Tolerance is set by f32 at map-scale world coordinates (the microsim
    /// cannot recentre — terrain is position-dependent), a fraction of a millimetre against a
    /// 0.2 m response.
    #[test]
    fn the_linear_response_is_the_kick_velocity_integrated_and_gravity_cancels() {
        let gear = test_gear();
        let ctx = micro_ctx(&gear, &NoGround, &tiger_inertia(), MAP_ORIGIN);
        let surge = Vec3::new(0.02, 0.01, 0.138);
        let basis = rolling_basis();
        let world_surge = basis.rot * surge;
        let response = impulse_response(basis.clone(), &ctx, 0, surge, Vec3::ZERO);
        for (k, (com, rot)) in response.iter().enumerate() {
            let expected = world_surge * k as f32 * TICK_SECS;
            assert!(
                (*com - expected).length() < 1e-3,
                "tick {k}: {com:?}, expected {expected:?}",
            );
            assert!(
                rot.to_scaled_axis().length() < 1e-6,
                "tick {k}: a pure surge must not rock the hull, got {rot:?}",
            );
        }
        let heavy_ctx = MicroCtx {
            gravity: GRAVITY * 4.0,
            ..micro_ctx(&gear, &NoGround, &tiger_inertia(), MAP_ORIGIN)
        };
        let heavy = impulse_response(basis, &heavy_ctx, 0, surge, Vec3::ZERO);
        for (k, (light, heavy)) in response.iter().zip(&heavy).enumerate() {
            assert!(
                (light.0 - heavy.0).length() < 1e-3,
                "tick {k}: gravity must cancel out of the difference",
            );
        }
    }

    /// THE MICROSIM'S ANGULAR LAW, in empty space: on a still basis the stored rotation delta is
    /// the BODY-frame twist exponentiated tick by tick. On a spinning basis the delta rides the
    /// basis rotation: the deviation from the still-basis axis is the proof the base motion is
    /// integrated, not ignored. Zeroing the basis angular velocity inside the microsim erases the
    /// deviation and reds the second half. Isotropic inertia, so the gyroscopic solve is exactly
    /// inert (`ω × Iω = 0`) and the closed form holds; the anisotropic gyroscopic path is Avian's
    /// own `solve_gyroscopic_torque`, not this module's code.
    #[test]
    fn the_angular_response_rides_the_basis_rotation() {
        let gear = test_gear();
        let sphere = AngularInertia::from_shape(&Sphere::new(1.5), HULL_MASS_KG);
        let ctx = micro_ctx(&gear, &NoGround, &sphere, MAP_ORIGIN);
        let twist = Vec3::new(0.057, 0.0, 0.0);
        let still = MicroTank {
            angular: Vec3::ZERO,
            ..rolling_basis()
        };
        let response = impulse_response(still.clone(), &ctx, 0, Vec3::ZERO, twist);
        for (k, (com, rot)) in response.iter().enumerate() {
            let expected = twist * k as f32 * TICK_SECS;
            assert!(
                (rot.to_scaled_axis() - expected).length() < 1e-4,
                "tick {k}: {rot:?}, expected {expected:?} as a rotation vector",
            );
            assert!(
                com.length() < 1e-6,
                "tick {k}: a pure spin must not translate the centre of mass",
            );
        }

        let yawing = MicroTank {
            angular: Vec3::new(0.0, 0.5, 0.0),
            ..rolling_basis()
        };
        let ridden = impulse_response(yawing, &ctx, 0, Vec3::ZERO, twist);
        // Sampled mid-response: the first-order magnitude band below tightens as `k·dt` grows
        // (the basis-composition correction is second order in time).
        const K: usize = 20;
        let sampled = ridden[K].1;
        let angle = sampled.to_scaled_axis().length();
        let expected_angle = twist.length() * K as f32 * TICK_SECS;
        assert!(
            (angle - expected_angle).abs() < expected_angle * 0.1,
            "the delta's magnitude stays first-order: {angle} vs {expected_angle}",
        );
        let still_axis = response[K].1.to_scaled_axis().normalize();
        assert!(
            sampled
                .to_scaled_axis()
                .normalize()
                .angle_between(still_axis)
                > 0.02,
            "a yawing basis must precess the body-frame delta axis off the still-basis one: \
             {sampled:?}",
        );
    }

    /// THE KICK IS ARRESTED BY THE GROUND. On a settled hull the 88's rearward surge is caught by
    /// track grip and suspension within the response window: the peak displacement stays far
    /// under the free-body drift (`|surge| · N · dt` — what the retired free-body model showed and
    /// the stream then had to walk back as residual), and by the horizon the response has SETTLED
    /// (its last per-tick step is a sliver of its peak step). Skipping the belt forces inside
    /// [`MicroTank::step`] turns the hull ballistic and reds both halves.
    #[test]
    fn the_kicked_hull_is_arrested_by_the_ground_not_ballistic() {
        let gear = test_gear();
        let ground = FlatGround { surface_y: 0.0 };
        let ctx = micro_ctx(&gear, &ground, &tiger_inertia(), Vec3::new(0.0, 0.86, 0.0));
        let basis = settled_on_flat(&ctx);
        let (surge, twist) = level_shot(Quat::IDENTITY);
        let response = impulse_response(basis, &ctx, 0, surge, twist);
        for (k, (com, rot)) in response.iter().enumerate() {
            assert!(
                com.is_finite() && rot.is_finite(),
                "tick {k}: the microsim must stay finite",
            );
        }
        let free_drift = surge.length() * RESPONSE_TICKS as f32 * TICK_SECS;
        let peak = response
            .iter()
            .map(|(com, _)| com.length())
            .fold(0.0_f32, f32::max);
        assert!(
            peak > 1e-4,
            "the kick must actually present ({peak} m peak)",
        );
        assert!(
            peak < 0.2 * free_drift,
            "the ground must arrest the kick: peak {peak} m vs free drift {free_drift} m",
        );
        let step = |k: usize| (response[k].0 - response[k - 1].0).length();
        let peak_step = (1..=RESPONSE_TICKS).map(step).fold(0.0_f32, f32::max);
        assert!(
            step(RESPONSE_TICKS) < 0.05 * peak_step,
            "the response must be settled at the horizon: last step {} vs peak step {peak_step}",
            step(RESPONSE_TICKS),
        );
    }

    /// THE FLAT-GROUND ROCK RINGS DOWN: the pitch transient decays at the suspension's own rate,
    /// so by the derived horizon the rotation delta is a small fraction of its peak — the settled
    /// tail may hold a permanent rest-pose change (the tail-heal clamp serves it), but it may not
    /// still be ringing.
    #[test]
    fn the_flat_ground_rock_settles_within_the_derived_horizon() {
        let gear = test_gear();
        let ground = FlatGround { surface_y: 0.0 };
        let ctx = micro_ctx(&gear, &ground, &tiger_inertia(), Vec3::new(0.0, 0.86, 0.0));
        let basis = settled_on_flat(&ctx);
        let (surge, twist) = level_shot(Quat::IDENTITY);
        let response = impulse_response(basis, &ctx, 0, surge, twist);
        let angle = |k: usize| response[k].1.to_scaled_axis().length();
        let peak = (0..=RESPONSE_TICKS).map(angle).fold(0.0_f32, f32::max);
        assert!(
            peak > 1e-4,
            "the rock must actually present ({peak} rad peak)",
        );
        assert!(
            angle(RESPONSE_TICKS) < 0.15 * peak,
            "the rock must ring down by the horizon: {} rad vs peak {peak} rad",
            angle(RESPONSE_TICKS),
        );
    }

    /// THE LEAD IS REAL INTEGRATION: a basis still bouncing (dropped 5 cm above its settled pose)
    /// walked forward under the sim's laws lands the kick at a different point in the bounce, so
    /// the response measurably differs from the unwalked one. Emptying the lead loop in
    /// [`impulse_response`] makes them identical and reds this.
    #[test]
    fn the_basis_walks_forward_through_the_lead_before_the_kick() {
        let gear = test_gear();
        let ground = FlatGround { surface_y: 0.0 };
        let ctx = micro_ctx(&gear, &ground, &tiger_inertia(), Vec3::new(0.0, 0.86, 0.0));
        let mut basis = settled_on_flat(&ctx);
        basis.com.y += 0.05;
        let (surge, twist) = level_shot(Quat::IDENTITY);
        let unwalked = impulse_response(basis.clone(), &ctx, 0, surge, twist);
        let walked = impulse_response(basis, &ctx, 8, surge, twist);
        let divergence = (0..=RESPONSE_TICKS)
            .map(|k| (unwalked[k].0 - walked[k].0).length())
            .fold(0.0_f32, f32::max);
        assert!(
            divergence > 1e-4,
            "an 8-tick lead through a live bounce must change the response ({divergence} m)",
        );
    }

    /// A STILL-INTEGRATING RESPONSE IS NEVER SPENT: while the amortized pair is live, the stored
    /// end is a compute boundary, not the horizon — a cursor racing past it (frame-hitch burst)
    /// must clamp both terms to the same last entry (offset exactly zero) and RETAIN the kick,
    /// never retire it. Dropping the liveness guard in [`kick_offset`] retires it and reds this.
    #[test]
    fn a_still_integrating_response_is_never_spent() {
        const FIRE_TICK: u32 = 100;
        let mut kick = kick(
            FIRE_TICK,
            Vec3::new(0.0, 0.01, 0.138),
            Vec3::new(0.057, 0.0, 0.0),
        );
        kick.response.truncate(9);
        let paused = micro_tank(Quat::IDENTITY, Vec3::ZERO, Vec3::ZERO);
        kick.live = Some(Box::new(LivePair {
            clean: paused.clone(),
            kicked: paused,
            kick: None,
            origin: Vec3::ZERO,
            target: DriveAxes::default(),
            spent: core::time::Duration::ZERO,
        }));
        // Delivered content 21 — far past the stored end (8), well short of the horizon.
        let (offset, twist, spent) = kick_offset(&kick, 130.0, 120.0);
        assert!(!spent, "a live response must be retained");
        assert_eq!(
            (offset, twist),
            (Vec3::ZERO, Vec3::ZERO),
            "both terms clamp to the same last computed entry",
        );
    }

    /// The confirmed-to-fire gap clamps to `[0, MAX_BASIS_LEAD]`: a future-stamped basis walks
    /// nowhere, a stalled stream's gap is bounded.
    #[test]
    fn the_basis_lead_is_the_confirmed_gap_clamped() {
        assert_eq!(basis_lead(1_000, 993.0), 7);
        assert_eq!(basis_lead(1_000, 1_005.0), 0);
        assert_eq!(basis_lead(1_000, 100.0), MAX_BASIS_LEAD);
    }

    /// THE FULL KICK PRESENTS AT THE CLICK. On the first rendered frame after the fire tick the
    /// delivered term is still zero, and the offset is the response itself — content time 1 at the
    /// tick boundary, because the fire tick's own integration step is inside the pose stamped with
    /// it. Dropping `content_time`'s one-tick lead presents `R(0) = 0` here and reds this.
    #[test]
    fn the_full_response_presents_at_the_click() {
        const FIRE_TICK: u32 = 4_000;
        let surge = Vec3::new(0.0, 0.01, 0.138);
        let kick = kick(FIRE_TICK, surge, Vec3::new(0.057, 0.0, 0.0));
        // Cursor 8 ticks behind the fire: delivered content clamps to zero. The kick's surge is
        // body-frame; the presented offset is world, rotated by the basis heading.
        let (offset, twist, spent) = kick_offset(&kick, f64::from(FIRE_TICK), 3_992.0);
        let expected = rolling_basis().rot * surge * TICK_SECS;
        assert!(!spent);
        assert!(
            (offset - expected).length() < 1e-5,
            "at the click the offset is the first response step, got {offset:?}",
        );
        assert!(twist.length() > 0.0, "and the rock is already presented");

        // Mid-window, still before the crossing: the offset IS the stored trajectory, unscaled.
        let (offset, _, spent) = kick_offset(&kick, f64::from(FIRE_TICK) + 4.5, 3_996.5);
        let (expected, _) = sample(&kick.response, 5.5);
        assert!(!spent);
        assert!(
            (offset - expected).length() < 1e-6,
            "pre-crossing the played term presents whole: {offset:?} vs {expected:?}",
        );
    }

    /// THE STREAM IS SUBTRACTED THROUGH THE IDENTICAL TRAJECTORY: what the delivered term removes
    /// is exactly what a click at that content time would have presented, so played = presented +
    /// delivered with no envelope anywhere. Scaling the delivered sample — any tuned cancellation —
    /// breaks the identity.
    #[test]
    fn the_delivered_stream_cancels_through_the_identical_response() {
        const FIRE_TICK: u32 = 4_000;
        let kick = kick(
            FIRE_TICK,
            Vec3::new(0.0, 0.01, 0.138),
            Vec3::new(0.057, 0.0, 0.01),
        );
        let clock = f64::from(FIRE_TICK) + 10.0; // played content 11
        let crossed = f64::from(FIRE_TICK) + 4.0; // delivered content 5
        let uncrossed = f64::from(FIRE_TICK) - 1.0; // delivered content 0

        let (full, full_twist, _) = kick_offset(&kick, clock, uncrossed);
        let (residual, residual_twist, _) = kick_offset(&kick, clock, crossed);
        let (delivered, delivered_twist, _) =
            kick_offset(&kick, f64::from(FIRE_TICK) + 4.0, uncrossed);

        assert!(
            residual.length() < full.length(),
            "a delivering stream must shrink the presented offset",
        );
        assert!(
            (residual + delivered - full).length() < 1e-6,
            "presented + delivered must equal the full response: {residual:?} + {delivered:?} vs \
             {full:?}",
        );
        let recomposed =
            Quat::from_scaled_axis(delivered_twist) * Quat::from_scaled_axis(residual_twist);
        assert!(
            recomposed.angle_between(Quat::from_scaled_axis(full_twist)) < 1e-5,
            "the rock decomposes through the same trajectory",
        );
    }

    /// THE TAIL RETURNS TO EXACTLY ZERO. Past the horizon the played term holds `R(N)` while the
    /// delivered term walks the stored tail; the moment the cursor clears the window the offset is
    /// identically zero and the kick is spent — forever. Extrapolating the response past its last
    /// entry leaves the persistent-velocity residual in place and reds every assertion here.
    #[test]
    fn the_offset_is_exactly_zero_once_the_stream_has_delivered_the_response() {
        const FIRE_TICK: u32 = 4_000;
        let horizon = f64::from(FIRE_TICK) - 1.0 + RESPONSE_TICKS as f64;
        let kick = kick(
            FIRE_TICK,
            Vec3::new(0.0, 0.01, 0.138),
            Vec3::new(0.057, 0.0, 0.0),
        );
        // Mid-heal: the played term is clamped at the horizon, the delivered term is still short of
        // it — the residual is live and easing down the stored tail.
        let (offset, twist, spent) = kick_offset(&kick, horizon + 3.0, horizon - 3.0);
        assert!(
            offset.length() > 0.0 && twist.length() > 0.0 && !spent,
            "the heal tail is presented while the stream still owes response",
        );

        // The cursor clears the window: exactly zero, spent, and it stays that way.
        assert_eq!(
            kick_offset(&kick, horizon + 6.0, horizon),
            (Vec3::ZERO, Vec3::ZERO, true),
        );
        assert_eq!(
            kick_offset(&kick, horizon + 400.0, horizon + 300.0),
            (Vec3::ZERO, Vec3::ZERO, true),
        );
    }

    /// A CURSOR THAT STALLS HOLDS THE OVERLAY rather than skipping it: the delivered term is driven
    /// by the observed cursor, and once the played term is past the horizon a frozen cursor freezes
    /// the whole offset, not just half of it.
    #[test]
    fn a_stalled_cursor_holds_the_residual_where_it_was() {
        const FIRE_TICK: u32 = 512;
        let kick = kick(
            FIRE_TICK,
            Vec3::new(0.0, 0.01, 0.138),
            Vec3::new(0.057, 0.0, 0.0),
        );
        let stalled = f64::from(FIRE_TICK) + 6.0;
        let past_horizon = f64::from(FIRE_TICK) + RESPONSE_TICKS as f64;
        let first = kick_offset(&kick, past_horizon + 10.0, stalled);
        assert_eq!(first, kick_offset(&kick, past_horizon + 40.0, stalled));
        assert!(!first.2, "a stalled cursor has not cleared the window");
        assert!(first.0.length() > 0.0, "and the offset is still presented");
    }

    /// RAPID REFIRE COMPOSES ADDITIVELY: a second shot before the first response is spent adds its
    /// own, and neither is reset.
    #[test]
    fn a_second_shot_adds_its_response_to_the_one_still_in_flight() {
        const FIRST: u32 = 2_000;
        let surge = Vec3::new(0.0, 0.01, 0.138);
        let spin = Vec3::new(0.057, 0.0, 0.0);
        let first = kick(FIRST, surge, spin);
        let second = kick(FIRST + 4, surge, spin);
        let clock = f64::from(FIRST) + 8.0;
        let cursor = f64::from(FIRST) - 2.0;
        let (first_surge, first_twist, _) = kick_offset(&first, clock, cursor);
        let (second_surge, second_twist, _) = kick_offset(&second, clock, cursor);
        assert!(first_surge.length() > 0.0 && second_surge.length() > 0.0);
        assert!(
            (first_surge + second_surge).length() > first_surge.length()
                && (first_twist + second_twist).length() > first_twist.length(),
            "the second shot must add to the first, not replace it",
        );
    }

    /// The overlay arms only on the own interpolated hull — never an opponent's.
    #[test]
    fn only_the_own_interpolated_hull_arms() {
        let mut app = App::new();
        let mass_properties = || {
            (
                Mass(HULL_MASS_KG),
                tiger_inertia(),
                CenterOfMass(Vec3::ZERO),
            )
        };
        let own = app
            .world_mut()
            .spawn((NetTank, Controlled, Interpolated, mass_properties()))
            .id();
        // An opponent: interpolated, but not the player's.
        let opponent = app
            .world_mut()
            .spawn((NetTank, Interpolated, mass_properties()))
            .id();

        app.world_mut()
            .run_system_once(arm_recoil_overlay)
            .expect("arming runs");

        assert!(app.world().get::<RecoilOverlay>(own).is_some());
        assert!(app.world().get::<RecoilOverlay>(opponent).is_none());
    }

    /// FUSED OWN FIRE NEVER ARMS THE OVERLAY: the kick presents from the stream itself at the
    /// cursor, so there is no early presentation to cancel. With the lever unset the same root
    /// arms — the default path, pinned.
    #[test]
    fn the_fused_lever_keeps_the_overlay_unarmed() {
        let mut app = App::new();
        app.insert_resource(crate::FusedOwnFire);
        let own = app
            .world_mut()
            .spawn((
                NetTank,
                Controlled,
                Interpolated,
                Mass(HULL_MASS_KG),
                tiger_inertia(),
                CenterOfMass(Vec3::ZERO),
            ))
            .id();

        app.world_mut()
            .run_system_once(arm_recoil_overlay)
            .expect("arming runs");
        assert!(
            app.world().get::<RecoilOverlay>(own).is_none(),
            "fused: the overlay must not arm",
        );

        app.world_mut().remove_resource::<crate::FusedOwnFire>();
        app.world_mut()
            .run_system_once(arm_recoil_overlay)
            .expect("arming runs");
        assert!(
            app.world().get::<RecoilOverlay>(own).is_some(),
            "lever unset: the own interpolated hull arms exactly as before",
        );
    }
}
