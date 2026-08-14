//! The own hull's firing recoil, presented at the local fire tick instead of at the cursor.
//!
//! Under unpredicted drive the owner's body is `RigidBody::Static` (`net::rig`), so the hull
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
//! At the fire the module computes the hull's full impulse response by MICROSIM. The basis is the
//! newest CONFIRMED server state of the own hull — `ConfirmedHistory::newest_present` for
//! `Position`/`Rotation` (the snapshot buffer backing the interpolated stream) plus the
//! live-replicated `LinearVelocity`/`AngularVelocity` — and the excitation is
//! `shooting::recoil_impulse`, the SAME expression the sim applies, resolved about the authored
//! centre of mass with the authored inertia (`I⁻¹ (r × J)`, Avian's own principal-frame tensor).
//! One free rigid body is integrated [`RESPONSE_TICKS`] fixed ticks in the sim's own semi-implicit
//! order — gravity into velocity, velocity into the centre of mass, the world angular velocity
//! exponentiated onto the rotation — twice, kicked and unkicked, and the stored trajectory is the
//! difference, `R(k) = kicked pose − unkicked pose`. Ground contact, suspension and the gyroscopic
//! term are omitted; that fidelity gap plus the basis staleness (~RTT/2) IS the residual the
//! certification capture measures.
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
//! # RESPONSE_TICKS IS THE ONE TUNED CONSTANT
//!
//! Everything else is derived: amplitude and direction from `recoil_impulse` and the authored mass
//! properties, timing from the two observed clocks, the heal rate from the handoff span.
//! [`RESPONSE_TICKS`] alone is TUNED — see its doc for the bounds that pin it.
//!
//! # SCOPE OF THE WRITE
//!
//! `Transform` only, never `Position`/`Rotation` — the identical contract `net::render_error`
//! states, and this module reuses that module's offset shape: an entity-keyed presentation offset,
//! a world-space translation delta plus a body-local rotation delta applied on the right of the
//! simulated rotation, composed in `PostUpdate` onto a pose RE-DERIVED from `Position`/`Rotation`
//! rather than accumulated onto whatever `Transform` holds.
//!
//! Two ordering facts are load-bearing, and one is opposite to that module's:
//!
//! - `apply_recoil_overlay` runs **after** `camera::OrbitCameraSet`. `render_error` runs before it
//!   so the camera orbits the offset pose and the correction is invisible; the recoil overlay must
//!   be visible, so the third-person camera places itself from the un-rocked pose and the hull rocks
//!   inside the frame. There is no camera kick in this module.
//! - `track::view::TrackViewSet` orders after this set, so the belt and wheels are written from the
//!   same presented root pose the hull renders at.
//!
//! Arming is disjoint from `render_error`'s by construction — that module requires `Predicted`, this
//! one requires `Interpolated` and `Without<RenderErrorOffset>` — so exactly one presentation layer
//! ever re-derives a given root's `Transform`. In predicted mode this module arms nothing and the
//! local hull moves from the real impulse, as it always did.
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

use avian3d::prelude::{
    AngularInertia, AngularVelocity, CenterOfMass, ComputedAngularInertia, Gravity, LinearVelocity,
    Mass, PhysicsSystems, Position, Rotation,
};
use bevy::prelude::*;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::{Interpolated, LocalTimeline, NetworkTimeline, Predicted};

use super::protocol::NetTank;
use super::render_error::RenderErrorOffset;
use crate::ballistics::{FireShell, FireShellOrigin};
use crate::shooting::recoil_impulse;
use crate::tank::Controlled;

/// Below this the composed offset is treated as spent and zeroed, so it never lingers as denormal
/// dust and a root with nothing in flight keeps a `Transform` bit-identical to Avian's.
const ZERO_EPS_M: f32 = 1e-6;
const ZERO_EPS_RAD: f32 = 1e-6;

/// Microsim horizon in fixed ticks — the ONE TUNED CONSTANT in this module (312 ms at 64 Hz).
///
/// Bounded from BELOW by legibility: the free-body response must run at least as long as the real
/// hull's contact-arrested transient reads on screen (the suspension settles a kick in roughly a
/// third of a second), or the presented kick dies before the eye has it.
///
/// Bounded from ABOVE by the model's own fidelity: with no ground contact the kicked body keeps
/// its velocity forever, so every extra tick carries the trajectory further from the arrested
/// truth the stream will deliver — and lands directly in the post-cancellation residual the
/// certification capture measures. It also stretches the heal tail, which lasts one handoff span
/// past the horizon.
const RESPONSE_TICKS: usize = 20;

/// One free rigid body of the microsim: centre-of-mass position, rotation, world-frame velocities.
#[derive(Debug, Clone, Copy)]
struct FreeBody {
    com: Vec3,
    rot: Quat,
    linear: Vec3,
    angular: Vec3,
}

impl FreeBody {
    /// One fixed tick in the sim's own semi-implicit order: gravity into velocity, velocity into
    /// the centre of mass, the world angular velocity exponentiated onto the rotation (Avian's
    /// `integrate_velocities`/`integrate_positions` for a torque-free body).
    fn step(&mut self, gravity: Vec3, dt: f32) {
        self.linear += gravity * dt;
        self.com += self.linear * dt;
        self.rot = (Quat::from_scaled_axis(self.angular * dt) * self.rot).normalize();
    }
}

/// Integrate the kicked and unkicked bodies side by side and store the difference:
/// `R(k) = (world centre-of-mass delta, body-local rotation delta)` for `k = 0..=RESPONSE_TICKS`.
///
/// `surge` is the world-frame velocity change `J / mass`; `spin` is the world-frame angular
/// velocity change. Gravity is applied to BOTH bodies — it cancels in the difference exactly, and
/// keeping it in the loop keeps the step honest to the sim's integration rather than to a
/// simplified copy of it.
fn impulse_response(
    basis: FreeBody,
    surge: Vec3,
    spin: Vec3,
    gravity: Vec3,
    dt: f32,
) -> Vec<(Vec3, Quat)> {
    // A free body's dynamics are position-independent, so both trajectories are integrated about
    // the basis origin: the metre-scale world coordinate would otherwise cancel catastrophically
    // out of the millimetre-scale difference in f32.
    let mut clean = FreeBody {
        com: Vec3::ZERO,
        ..basis
    };
    let mut kicked = FreeBody {
        com: Vec3::ZERO,
        linear: basis.linear + surge,
        angular: basis.angular + spin,
        ..basis
    };
    let mut response = Vec::with_capacity(RESPONSE_TICKS + 1);
    response.push((Vec3::ZERO, Quat::IDENTITY));
    for _ in 0..RESPONSE_TICKS {
        clean.step(gravity, dt);
        kicked.step(gravity, dt);
        response.push((
            kicked.com - clean.com,
            (clean.rot.inverse() * kicked.rot).normalize(),
        ));
    }
    response
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
#[derive(Debug, Clone, PartialEq)]
struct RecoilKick {
    /// The client `LocalTimeline` tick the shot was fired on (`ShotId::fire_tick`).
    fire_tick: u32,
    /// `R(k)` for `k = 0..=RESPONSE_TICKS`.
    response: Vec<(Vec3, Quat)>,
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
        apply_recoil_overlay
            .in_set(RecoilOverlayApplied)
            .after(PhysicsSystems::Writeback)
            // THE "no camera kick" EDGE, inverted relative to `net::render_error` deliberately: the
            // third-person camera must place itself from the un-rocked pose.
            .after(crate::camera::OrbitCameraSet)
            .before(TransformSystems::Propagate),
    );
    // The belt and wheels are written FROM the presented root pose, so they must read the rocked
    // one. The edge lives here for the same reason `net::render_error` owns its copy: the
    // net-boundary guard keeps `track::view` from naming the netcode.
    app.configure_sets(
        PostUpdate,
        crate::track::view::TrackViewSet.after(RecoilOverlayApplied),
    );
}

/// Arm the own tank once the server stream — not local physics — owns its hull.
///
/// `Without<Predicted>` and `Without<RenderErrorOffset>` together make this module inert in
/// predicted mode and keep exactly one presentation layer re-deriving a root's `Transform`. The
/// mass-property requirements are the excitation's inputs: a root that has not finished
/// construction cannot yet derive a response.
#[expect(clippy::type_complexity, reason = "one arming predicate, spelled out")]
fn arm_recoil_overlay(
    tanks: Query<
        Entity,
        (
            With<NetTank>,
            With<Controlled>,
            With<Interpolated>,
            With<Mass>,
            With<AngularInertia>,
            With<CenterOfMass>,
            Without<Predicted>,
            Without<RenderErrorOffset>,
            Without<RecoilOverlay>,
            Without<ChildOf>,
        ),
    >,
    mut commands: Commands,
) {
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
/// once the cursor clears the response horizon.
fn kick_offset(kick: &RecoilKick, local_clock: f64, cursor: f64) -> (Vec3, Vec3, bool) {
    let delivered = content_time(kick.fire_tick, cursor);
    if delivered >= (kick.response.len() - 1) as f64 {
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

/// Admit one locally-fired shot: microsim the response from the newest confirmed basis, store it.
#[expect(clippy::type_complexity, reason = "one basis read, spelled out")]
fn excite_recoil_overlay(
    fire: On<FireShell>,
    cursors: Query<&InterpolationTimeline>,
    tick: Res<TickDuration>,
    gravity: Res<Gravity>,
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
    // The moment arm is read at the live pose — the frame `fire.origin` was computed in — and the
    // twist it yields is body-frame, so it transfers to the basis rotation below.
    let (surge, twist) = hull_response(
        impulse,
        fire.origin,
        position.0,
        rotation.0,
        mass,
        inertia,
        center_of_mass,
    );
    // The microsim basis: newest CONFIRMED pose (the snapshot buffer the stream interpolates), and
    // the live-replicated velocities (plain replication writes them on arrival — they ARE the
    // newest confirmed values). A history still warming up falls back to the live pose.
    let confirmed = confirmed_position.and_then(ConfirmedHistory::newest_present);
    let basis_position = confirmed.map_or(position.0, |(_, confirmed)| confirmed.0);
    let basis_rotation = confirmed_rotation
        .and_then(ConfirmedHistory::newest_present)
        .map_or(rotation.0, |(_, confirmed)| confirmed.0);
    let basis = FreeBody {
        com: basis_position + basis_rotation * center_of_mass.0,
        rot: basis_rotation,
        linear: linear.map_or(Vec3::ZERO, |velocity| velocity.0),
        angular: angular.map_or(Vec3::ZERO, |velocity| velocity.0),
    };
    debug!(
        "net: recoil response replay armed — fire tick {}, confirmed basis tick {:?}, basis |v| \
         {:.3} m/s |w| {:.3} rad/s",
        shot.fire_tick,
        confirmed.map(|(tick, _)| tick.0),
        basis.linear.length(),
        basis.angular.length(),
    );
    overlay.kicks.push(RecoilKick {
        fire_tick: shot.fire_tick,
        response: impulse_response(
            basis,
            surge,
            basis_rotation * twist,
            gravity.0,
            tick.0.as_secs_f32(),
        ),
    });
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

    /// The 88 mm round and the Tiger hull, from `assets/tiger_1/tiger_1.tank.ron`.
    const SHELL_MASS_KG: f32 = 10.2;
    const MUZZLE_SPEED_MPS: f32 = 773.0;
    const HULL_MASS_KG: f32 = 57_000.0;
    /// A muzzle 1.5 m above and 3 m ahead of the centre of mass — the geometry that turns the
    /// bore-axis impulse into gun climb.
    const MUZZLE_LIFT_M: f32 = 1.5;
    const MUZZLE_REACH_M: f32 = 3.0;
    /// The fixed tick, in seconds (64 Hz).
    const TICK_SECS: f32 = 1.0 / 64.0;
    const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

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

    /// A moving, spinning basis on a gravity world — nothing about the response laws below may
    /// depend on any of it being zero.
    fn rolling_basis() -> FreeBody {
        FreeBody {
            com: Vec3::new(140.0, 6.6, 290.0),
            rot: Quat::from_rotation_y(0.4),
            linear: Vec3::new(1.0, 0.2, -3.0),
            angular: Vec3::new(0.05, -0.3, 0.02),
        }
    }

    fn kick(fire_tick: u32, surge: Vec3, spin: Vec3) -> RecoilKick {
        RecoilKick {
            fire_tick,
            response: impulse_response(rolling_basis(), surge, spin, GRAVITY, TICK_SECS),
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

    /// THE MICROSIM'S LINEAR LAW: the stored difference is the kick velocity integrated tick by
    /// tick, and gravity — applied to both bodies — cancels out of it exactly. Applying gravity to
    /// one body only, or scaling the surge anywhere in the loop, moves every entry off
    /// `surge · k · dt`.
    #[test]
    fn the_linear_response_is_the_kick_velocity_integrated_and_gravity_cancels() {
        let surge = Vec3::new(0.02, 0.01, 0.138);
        let response = impulse_response(rolling_basis(), surge, Vec3::ZERO, GRAVITY, TICK_SECS);
        for (k, (com, rot)) in response.iter().enumerate() {
            let expected = surge * k as f32 * TICK_SECS;
            assert!(
                (*com - expected).length() < 1e-4,
                "tick {k}: {com:?}, expected {expected:?}",
            );
            assert!(
                rot.to_scaled_axis().length() < 1e-6,
                "tick {k}: a pure surge must not rock the hull, got {rot:?}",
            );
        }
        let heavy = impulse_response(rolling_basis(), surge, Vec3::ZERO, GRAVITY * 4.0, TICK_SECS);
        for (k, (light, heavy)) in response.iter().zip(&heavy).enumerate() {
            assert!(
                (light.0 - heavy.0).length() < 1e-4,
                "tick {k}: gravity must cancel out of the difference",
            );
        }
    }

    /// THE MICROSIM'S ANGULAR LAW: on a still basis the stored rotation delta is the spin
    /// exponentiated tick by tick — expressed BODY-locally, so the world axis lands in it rotated
    /// by the basis. On a spinning basis the delta rides the basis rotation: the deviation from the
    /// still-basis axis is the proof the base motion is integrated, not ignored. Zeroing the basis
    /// angular velocity inside the microsim erases the deviation and reds the second half.
    #[test]
    fn the_angular_response_rides_the_basis_rotation() {
        let spin = Vec3::new(0.057, 0.0, 0.0);
        let still = FreeBody {
            angular: Vec3::ZERO,
            ..rolling_basis()
        };
        let response = impulse_response(still, Vec3::ZERO, spin, GRAVITY, TICK_SECS);
        // The stored delta is BODY-local: the world spin axis lands in it rotated by the basis.
        let body_spin = still.rot.inverse() * spin;
        for (k, (com, rot)) in response.iter().enumerate() {
            let expected = body_spin * k as f32 * TICK_SECS;
            assert!(
                (rot.to_scaled_axis() - expected).length() < 1e-5,
                "tick {k}: {rot:?}, expected {expected:?} as a rotation vector",
            );
            assert!(
                com.length() < 1e-6,
                "tick {k}: a pure spin must not translate the centre of mass",
            );
        }

        let yawing = FreeBody {
            angular: Vec3::new(0.0, 0.5, 0.0),
            ..rolling_basis()
        };
        let ridden = impulse_response(yawing, Vec3::ZERO, spin, GRAVITY, TICK_SECS);
        let last = ridden[RESPONSE_TICKS].1;
        let angle = last.to_scaled_axis().length();
        let expected_angle = spin.length() * RESPONSE_TICKS as f32 * TICK_SECS;
        assert!(
            (angle - expected_angle).abs() < expected_angle * 0.1,
            "the delta's magnitude stays first-order: {angle} vs {expected_angle}",
        );
        let still_axis = response[RESPONSE_TICKS].1.to_scaled_axis().normalize();
        assert!(
            last.to_scaled_axis().normalize().angle_between(still_axis) > 0.02,
            "a yawing basis must precess the body-frame delta axis off the still-basis one: \
             {last:?}",
        );
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
        // Cursor 8 ticks behind the fire: delivered content clamps to zero.
        let (offset, twist, spent) = kick_offset(&kick, f64::from(FIRE_TICK), 3_992.0);
        assert!(!spent);
        assert!(
            (offset - surge * TICK_SECS).length() < 1e-5,
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
        let first = kick_offset(&kick, f64::from(FIRE_TICK) + 30.0, stalled);
        assert_eq!(
            first,
            kick_offset(&kick, f64::from(FIRE_TICK) + 60.0, stalled)
        );
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

    /// PREDICTED MODE IS INERT. The overlay arms only where the server stream owns the hull; a
    /// predicted root's kick comes from the real impulse and `net::render_error` owns its pose.
    /// The `both` case pins `Without<Predicted>` itself: prediction wins even on a root that also
    /// carries `Interpolated`.
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
        let predicted = app
            .world_mut()
            .spawn((NetTank, Controlled, Predicted, mass_properties()))
            .id();
        // Both markers at once — a role transition in flight. Prediction owns the pose.
        let both = app
            .world_mut()
            .spawn((
                NetTank,
                Controlled,
                Interpolated,
                Predicted,
                mass_properties(),
            ))
            .id();
        // An opponent: interpolated, but not the player's.
        let opponent = app
            .world_mut()
            .spawn((NetTank, Interpolated, mass_properties()))
            .id();
        // A root `net::render_error` already owns — the disjointness guard.
        let smoothed = app
            .world_mut()
            .spawn((
                NetTank,
                Controlled,
                Interpolated,
                RenderErrorOffset::default(),
                mass_properties(),
            ))
            .id();

        app.world_mut()
            .run_system_once(arm_recoil_overlay)
            .expect("arming runs");

        assert!(app.world().get::<RecoilOverlay>(own).is_some());
        assert!(app.world().get::<RecoilOverlay>(predicted).is_none());
        assert!(
            app.world().get::<RecoilOverlay>(both).is_none(),
            "a root prediction owns must never arm, whatever else it carries",
        );
        assert!(app.world().get::<RecoilOverlay>(opponent).is_none());
        assert!(
            app.world().get::<RecoilOverlay>(smoothed).is_none(),
            "two presentation layers must never re-derive one root's Transform",
        );
    }
}
