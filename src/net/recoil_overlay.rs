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
//! # THE HANDOFF IS AN EVENT, NOT AN ESTIMATE
//!
//! The instant the replicated kick begins to appear is the instant the **interpolation cursor**
//! reaches the **fire tick**. Both numbers are held locally: `ShotId::fire_tick` is the client's own
//! `LocalTimeline` tick at the fire (`net::protocol::publish_shot_clock` republishes it as
//! `ShotClock`), and the cursor is `InterpolationTimeline::tick()` + overstep — the same quantity
//! `trace::record_frame` emits as `itick`. The overlay therefore needs no delay model, no threshold
//! and no constant: it measures the window it must live in, at the fire, and then tracks the cursor
//! through it. Lightyear's ±5 % clock dilation and an interpolation clamp both move the crossing;
//! driving the transient off the observed cursor rather than off wall time follows it either way.
//!
//! # NOTHING HERE IS TUNED
//!
//! - **Amplitude and direction** come from `shooting::recoil_impulse` — the SAME expression the sim
//!   applies to the hull, so the two cannot diverge on what a shot does. Linear response is
//!   `J / mass`; angular response is `I⁻¹ (r × J)` about the authored centre of mass, both read from
//!   the root's spec-authored `Mass` / `AngularInertia` / `CenterOfMass`. An MG round falls out
//!   ~700× smaller on its own, with no per-weapon term.
//! - **Duration** is the measured handoff window `fire_tick − cursor`, sampled at the fire. It is
//!   the derived interpolation delay plus the client's lead over the server, observed rather than
//!   reconstructed.
//! - **Shape** is forced by its boundary conditions. Requiring `x(0) = 0`, `ẋ(0) = v₀`, `x(W) = 0`
//!   and `ẋ(W) = 0` — leave at the physical recoil velocity, be back at rest exactly when the stream
//!   takes over — determines a unique cubic, `x(u) = v₀·W·u·(1−u)²`. There is no free coefficient to
//!   pick. `ẋ(W) = 0` is the C¹ condition at the crossing: the overlay adds neither a pose step nor
//!   a velocity step to the frame the replicated kick starts on.
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
//! is plain replicated state rather than predicted state, so a crew-paused reload the server applied
//! is invisible for `RTT/2`; a starved input can also fail the `for_tick` attestation in
//! `net::protocol::bridge_action_state_to_tank_command`. The cost here is bounded to a phantom rock
//! that decays to zero on its own — this overlay carries no negative lobe and cancels nothing out of
//! the stream, so it needs no confirmation gate. (One is available if it ever does: the server's
//! confirmation arrives at `RTT/2` and the crossing at `RTT/2 + D`, so it provably lands first.)
//!
//! Rapid refire composes additively: each shot contributes its own transient over its own measured
//! window, and the offsets sum.
//!
//! Design note: `.agents/scratch/impulse-prediction-mixed-timeline-2026-08-14.md`.

use avian3d::prelude::{
    AngularInertia, CenterOfMass, ComputedAngularInertia, Mass, PhysicsSystems, Position, Rotation,
};
use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::{Interpolated, NetworkTimeline, Predicted};

use super::protocol::NetTank;
use super::render_error::RenderErrorOffset;
use crate::ballistics::{FireShell, FireShellOrigin};
use crate::shooting::recoil_impulse;
use crate::tank::Controlled;

/// Below this the composed offset is treated as spent and zeroed, so it never lingers as denormal
/// dust and a root with nothing in flight keeps a `Transform` bit-identical to Avian's.
const ZERO_EPS_M: f32 = 1e-6;
const ZERO_EPS_RAD: f32 = 1e-6;

/// One shot's transient, in flight between its fire tick and the cursor's crossing of it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RecoilKick {
    /// The client `LocalTimeline` tick the shot was fired on (`ShotId::fire_tick`).
    fire_tick: u32,
    /// The handoff window in ticks — `fire_tick − cursor`, sampled once at the fire. Strictly
    /// positive: a kick whose crossing is already due is never admitted.
    span_ticks: f64,
    /// The same window in seconds. The transient's only time scale.
    window_secs: f32,
    /// World-frame recoil velocity of the centre of mass (m/s), `J / mass`.
    surge: Vec3,
    /// Body-frame recoil angular velocity (rad/s) as a rotation vector, `I⁻¹ (r × J)`.
    twist: Vec3,
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

/// The interpolation cursor in fractional ticks — `trace`'s `itick`, and the only clock this module
/// reads. `f64` because a session's tick counter outgrows `f32`'s sub-tick resolution.
fn cursor_ticks(timeline: &InterpolationTimeline) -> f64 {
    f64::from(timeline.tick().0) + f64::from(timeline.overstep().to_f32())
}

/// Where the cursor stands inside one kick's handoff window: `0` at the fire, `1` at the crossing.
///
/// Clamped at both ends, so a cursor that stalls (an interpolation clamp) holds the transient where
/// it was rather than skipping it, and a cursor at or past the fire tick reports a spent kick.
fn handoff_progress(fire_tick: u32, span_ticks: f64, cursor: f64) -> f32 {
    let remaining = f64::from(fire_tick) - cursor;
    (1.0 - remaining / span_ticks).clamp(0.0, 1.0) as f32
}

/// The unique cubic on `[0, 1]` with `s(0) = 0`, `s'(0) = 1`, `s(1) = 0`, `s'(1) = 0`, in units of
/// the window: the offset is `v₀ · W · s(u)`.
fn transient(u: f32) -> f32 {
    u * (1.0 - u) * (1.0 - u)
}

/// One kick's contribution at cursor position `cursor`: `(surge, twist, spent)`.
fn kick_offset(kick: &RecoilKick, cursor: f64) -> (Vec3, Vec3, bool) {
    let progress = handoff_progress(kick.fire_tick, kick.span_ticks, cursor);
    let scale = kick.window_secs * transient(progress);
    (kick.surge * scale, kick.twist * scale, progress >= 1.0)
}

/// The hull's response to one shot: `(surge, twist)` from the sim's own recoil impulse.
///
/// `surge` is world-frame linear velocity; `twist` is BODY-frame angular velocity, which is the
/// frame the offset is stored and applied in. Working the moment arm in the body frame is what keeps
/// the inertia tensor un-rotated: `I_world = R I R⁻¹`, so `R⁻¹ I_world⁻¹ = I⁻¹ R⁻¹`.
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

/// Admit one locally-fired shot's transient, sized by the handoff window observed at the fire.
fn excite_recoil_overlay(
    fire: On<FireShell>,
    cursors: Query<&InterpolationTimeline>,
    tick: Res<TickDuration>,
    mut roots: Query<(
        &Position,
        &Rotation,
        &Mass,
        &AngularInertia,
        &CenterOfMass,
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
    let Ok((position, rotation, mass, inertia, center_of_mass, mut overlay)) =
        roots.get_mut(source.tank)
    else {
        return;
    };
    let Ok(timeline) = cursors.single() else {
        return;
    };
    // The cursor is already at or past the fire tick: the stream is about to carry the kick itself,
    // and there is no window to fill.
    let span_ticks = f64::from(shot.fire_tick) - cursor_ticks(timeline);
    if span_ticks <= 0.0 {
        return;
    }
    let impulse = recoil_impulse(fire.direction, fire.mass, fire.speed);
    let (surge, twist) = hull_response(
        impulse,
        fire.origin,
        position.0,
        rotation.0,
        mass,
        inertia,
        center_of_mass,
    );
    overlay.kicks.push(RecoilKick {
        fire_tick: shot.fire_tick,
        span_ticks,
        window_secs: (span_ticks * tick.0.as_secs_f64()) as f32,
        surge,
        twist,
    });
}

/// Compose every in-flight transient and present it, re-derived from the sim pose.
fn apply_recoil_overlay(
    cursors: Query<&InterpolationTimeline>,
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

    for (mut transform, position, rotation, center_of_mass, mut overlay) in &mut roots {
        let mut surge = Vec3::ZERO;
        let mut twist = Vec3::ZERO;
        // A spent kick contributes exactly zero (`transient(1) == 0`), so summing and retiring in
        // one pass cannot change the composed offset.
        overlay.kicks.retain(|kick| {
            let (kick_surge, kick_twist, spent) = kick_offset(kick, cursor);
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
    const TICK_SECS: f64 = 1.0 / 64.0;

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

    fn kick(fire_tick: u32, span_ticks: f64) -> RecoilKick {
        RecoilKick {
            fire_tick,
            span_ticks,
            window_secs: (span_ticks * TICK_SECS) as f32,
            surge: Vec3::new(0.0, 0.0, 1.0),
            twist: Vec3::new(1.0, 0.0, 0.0),
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

    /// THE PROPERTY THIS MODULE EXISTS FOR, and the one the mutation check breaks: the overlay is
    /// exactly zero at the cursor's crossing of the fire tick, whatever the interpolation delay in
    /// force — because the window it decays over IS the observed distance to that crossing.
    ///
    /// Five spans, from one tick to forty, each sampled at its own crossing. A decay duration
    /// decoupled from the cursor (a fixed tick count, a wall-clock constant) leaves a residual in
    /// every span but the one it happens to match.
    #[test]
    fn the_overlay_is_spent_exactly_at_the_cursors_crossing_for_every_interp_delay() {
        const FIRE_TICK: u32 = 4_000;
        for span in [1.0_f64, 3.0, 6.7, 12.5, 40.0] {
            let kick = kick(FIRE_TICK, span);
            // The START is zero only to floating point: reconstructing `remaining` from a cursor
            // that was itself derived by subtraction loses the last bits of a fractional span. The
            // CROSSING below is exact — `remaining = 0` makes the progress clamp return exactly 1
            // and the cubic's `(1 − u)²` factor exactly 0.
            let cursor_at_fire = f64::from(FIRE_TICK) - span;
            let (surge, twist, spent) = kick_offset(&kick, cursor_at_fire);
            assert!(
                surge.length() < 1e-9 && twist.length() < 1e-9 && !spent,
                "span {span}: the transient must start from the clean pose, got {surge:?}",
            );

            // Halfway to the crossing the overlay must be genuinely present, or "zero at the
            // crossing" would be satisfied by an overlay that never existed.
            let (surge, twist, spent) = kick_offset(&kick, cursor_at_fire + span / 2.0);
            assert!(
                surge.length() > 0.0 && twist.length() > 0.0 && !spent,
                "span {span}: the overlay must be live mid-window",
            );

            assert_eq!(
                kick_offset(&kick, f64::from(FIRE_TICK)),
                (Vec3::ZERO, Vec3::ZERO, true),
                "span {span}: the overlay must be spent AT the crossing",
            );
            // And past it, forever.
            assert_eq!(
                kick_offset(&kick, f64::from(FIRE_TICK) + 30.0),
                (Vec3::ZERO, Vec3::ZERO, true),
                "span {span}: the overlay must stay spent past the crossing",
            );
        }
    }

    /// The transient LANDS at rest, so the frame the replicated kick starts on inherits neither a
    /// pose step nor a velocity step from the overlay — and LEAVES at the physical recoil velocity,
    /// so the amplitude derived above is the one actually presented.
    #[test]
    fn the_transient_leaves_at_the_recoil_velocity_and_lands_at_rest() {
        const FIRE_TICK: u32 = 900;
        const SPAN: f64 = 8.0;
        let kick = kick(FIRE_TICK, SPAN);
        let at = |progress: f64| {
            kick_offset(&kick, f64::from(FIRE_TICK) - SPAN * (1.0 - progress))
                .0
                .length()
        };
        let peak = at(1.0 / 3.0);
        assert!(
            at(0.99) < peak * 1e-3,
            "the last percent of the window must be at rest: {} vs peak {peak}",
            at(0.99),
        );
        let ballistic = 0.01 * f64::from(kick.window_secs) * f64::from(kick.surge.length());
        let leaving = f64::from(at(0.01)) / ballistic;
        assert!(
            (leaving - 1.0).abs() < 0.03,
            "the transient must leave at the physical recoil velocity, ratio {leaving}",
        );
    }

    /// A CURSOR THAT STALLS HOLDS THE OVERLAY rather than skipping it. An interpolation clamp
    /// freezes the stream; the overlay must freeze with it, not retire early against wall time.
    #[test]
    fn a_stalled_cursor_holds_the_transient_where_it_was() {
        const FIRE_TICK: u32 = 512;
        const SPAN: f64 = 10.0;
        let kick = kick(FIRE_TICK, SPAN);
        let stalled = f64::from(FIRE_TICK) - SPAN * 0.6;
        let first = kick_offset(&kick, stalled);
        assert_eq!(first, kick_offset(&kick, stalled));
        assert!(!first.2, "a stalled cursor has not reached the crossing");
        assert!(first.0.length() > 0.0, "and the offset is still presented");
    }

    /// RAPID REFIRE COMPOSES ADDITIVELY: a second shot before the first transient is spent adds its
    /// own, and neither is reset.
    #[test]
    fn a_second_shot_adds_its_transient_to_the_one_still_in_flight() {
        const FIRST: u32 = 2_000;
        const SPAN: f64 = 10.0;
        let first = kick(FIRST, SPAN);
        let second = kick(FIRST + 4, SPAN);
        // Five ticks short of the first crossing: the first kick is 50 % through its window, the
        // second 10 %.
        let cursor = f64::from(FIRST) - 5.0;
        let (first_surge, first_twist, _) = kick_offset(&first, cursor);
        let (second_surge, second_twist, _) = kick_offset(&second, cursor);
        assert!(first_surge.length() > 0.0 && second_surge.length() > 0.0);
        assert!(
            (first_surge + second_surge).length() > first_surge.length()
                && (first_twist + second_twist).length() > first_twist.length(),
            "the second shot must add to the first, not replace it",
        );
    }

    /// PREDICTED MODE IS INERT. The overlay arms only where the server stream owns the hull; a
    /// predicted root's kick comes from the real impulse and `net::render_error` owns its pose.
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
        assert!(app.world().get::<RecoilOverlay>(opponent).is_none());
        assert!(
            app.world().get::<RecoilOverlay>(smoothed).is_none(),
            "two presentation layers must never re-derive one root's Transform",
        );
    }
}
