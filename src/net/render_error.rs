//! Client-side smoothing for rollback corrections.
//!
//! The offset is presentation-only: this module writes the predicted root's `Transform`, never its
//! rollback state (`Position`/`Rotation`). `apply_render_error` runs after Avian writeback and before
//! transform propagation, so the root, children, and camera share one rendered pose.
//!
//! # THE CORRECTION IS LIGHTYEAR'S, MEASURED AT ONE TICK
//!
//! What must be smoothed is the DISCONTINUITY a rollback introduces, and nothing else. Until slice 4
//! this module derived it itself, by diffing `PredictionMetrics::rollbacks` and then taking the
//! PREVIOUS FRAME's displayed pose minus the current corrected one. That difference is the
//! correction PLUS one frame of legitimate travel, so the rollback frame rendered a heavy near-hold
//! of ordinary motion as well as of the correction, and the counter it keyed on is global — another
//! tank's rollback tripped every armed root, two rollbacks between observations collapsed into one,
//! and no attribution survived.
//!
//! lightyear already computes the exact quantity and this module now reads it.
//! `lightyear_prediction::rollback::prepare_rollback` stores `PreviousVisual<C>` — the component
//! value as it stood before the restore, which at `PreUpdate` IS the pose the previous frame
//! displayed, because `FrameInterpolationSystems::Restore` does not run until `RunFixedMainLoop`.
//! `lightyear_prediction::correction::update_frame_interpolation_post_rollback` then turns it into
//! `VisualCorrection<D>`:
//!
//! ```text
//! error = current_visual.diff(&previous_visual)
//! current_visual = interpolate(history.get(tick - 1), component, overstep)
//! ```
//!
//! Both sides are evaluated over the SAME tick pair and the SAME overstep, so they differ only by
//! what the rollback did. No frame of travel is in it.
//!
//! ## Registration: `add_linear_correction_fn`, not `enable_correction`
//!
//! `PreviousVisual` is inserted only when `PredictionRegistry::has_correction::<C>()`, and the two
//! registrations that set that flag are NOT interchangeable
//! (`lightyear_prediction-0.28.0/src/registry.rs`): `enable_correction` flips the flag alone, while
//! `add_correction_fn` (which `add_linear_correction_fn` calls) additionally mounts
//! `add_correction_systems::<C, D>`. Under `enable_correction` `PreviousVisual` would still be
//! inserted on every rollback, `VisualCorrection` would never be produced, and nothing would ever
//! remove `PreviousVisual`. `net::protocol` therefore keeps `.add_linear_correction_fn()` on
//! `Position` and `Rotation`, and this module consumes what it produces.
//!
//! ## SIGN CONVENTIONS, and one asymmetry that is NOT ours to use
//!
//! `lightyear_replication-0.28.0/src/impls/avian3d.rs`: translation `diff(&self, new) = new - self`,
//! rotation `diff(&self, new) = new * self.inverse()` — a LEFT delta. So the captured error is
//! `previous_displayed - corrected` for translation and `previous_displayed * corrected⁻¹` for
//! rotation, which is exactly what has to be added to / left-multiplied onto the corrected pose to
//! keep the previous one on screen. `the_captured_error_reproduces_the_previous_displayed_pose`
//! pins that against lightyear's own `Diffable` impls rather than against a restatement of them.
//!
//! The same file's `apply_diff` for rotation RIGHT-multiplies (`self.0 *= delta.0`) what `diff`
//! LEFT-multiplied. Those are not the same rotation, and nothing here routes through `apply_diff`;
//! this module composes the delta itself, on the left. That asymmetry is a latent upstream bug,
//! currently inert only because `net::client`'s `CorrectionPolicy::instant_correction` decays
//! lightyear's own applied error to ~0 — and, since slice 4, because this module REMOVES
//! `VisualCorrection` from the roots it manages before `RollbackSystems::VisualCorrection` can read
//! it. Not fixed here; the vendored crate is not modified.
//!
//! # ZERO-DEPTH ROLLBACKS PRODUCE NO CORRECTION AT ALL, and that is load-bearing
//!
//! `update_frame_interpolation_post_rollback` reads `history.get(tick - 1)` and `continue`s when it
//! is `None`. `prepare_rollback` CLEARS the prediction history and re-seeds it with a single entry
//! at the rollback tick, and `run_rollback` replays `current_tick - rollback_tick` times — zero when
//! they are equal. So a rollback of depth 0 leaves the history holding only `tick`, the lookup at
//! `tick - 1` misses, and lightyear emits no `VisualCorrection` (and, upstream's own leak, never
//! removes `PreviousVisual`). Nothing is captured here, so the view takes the corrected pose
//! immediately.
//!
//! That is the RIGHT answer for the only depth-0 rollback this game produces. `net::adoption` targets
//! `AuthoritativeFact::produced_at`, which at the shipping loopback lead of 0 is the current tick —
//! `net::lead_zero_rollback::the_zero_lead_shove_arrives_with_no_replayed_tick_at_all` pins that
//! there is no replayed tick at all — and an adopted fact is precisely the correction that must stay
//! SHARP. The other producers cannot reach depth 0: `net::watchdog` targets at most `current - 1`,
//! and lightyear's own state-rollback routes need `confirmed_tick < current_tick` (receive time) or a
//! recorded mismatch that only that same receive-time path can write.
//!
//! It is a COINCIDENCE OF TWO MECHANISMS, not a design, so it is pinned rather than trusted:
//! `a_zero_depth_rollback_produces_no_visual_correction_so_nothing_is_accumulated` fails if lightyear
//! starts emitting one, at which point this module would begin smoothing adoptions and the sharp
//! rule below would have to carry that case too.
//!
//! # SHARP means: do not accumulate that root's correction AT ALL
//!
//! `net::adoption::SharpCorrection` names the predicted root whose correction on this rollback
//! carries an authoritative external event — established from what the rollback DELIVERED, not from
//! the cause its claimant tagged the slot with. For a named root the whole correction delta is
//! dropped: the corrected pose already contains the hit, and adding no compensating offset is what
//! makes it visible immediately.
//!
//! - NOT "accumulate but decay faster". That still delays and attenuates the hit, and introduces a
//!   feel threshold with no semantic meaning.
//! - NOT a smoothed/sharp split of one correction. The corrected pose is the nonlinear result of the
//!   external impulse, ordinary divergence, replay and contacts; the provenance to decompose it does
//!   not exist.
//! - A rollback carrying BOTH a delivered hit and ordinary misprediction on one root leaves that
//!   root's WHOLE delta sharp. The hit wins, matching `AdoptionCause::wins_over`: exposing some
//!   coincident error beats hiding the hit.
//! - Every OTHER root in the same rollback still smooths normally. That is the second reason the
//!   signal names entities.
//! - An older offset already decaying on a sharp root keeps decaying; only this rollback's delta is
//!   refused.
//!
//! # NOTHING CROSSES A SCHEDULE
//!
//! Classification and capture both happen inside the `PreUpdate` rollback transaction:
//! `net::adoption::confirm_forced_rollback` establishes delivery after `RollbackSystems::Prepare`
//! and writes the occurrence; [`capture_render_error`] runs after `RollbackSystems::EndRollback`,
//! DRAINS the queue whether or not anything matches, and either accumulates the correction or
//! refuses it. `PostUpdate` then only decays an already-classified offset and presents it. There is
//! no value established at one schedule point and consumed at another, so this module contributes no
//! row to ADR-0032's latch audit.
//!
//! # THE PRESENTED POSE IS DERIVED, NOT ACCUMULATED
//!
//! [`apply_render_error`] does not add the offset to whatever `Transform` currently holds. It
//! re-derives the pose from `Position`/`Rotation` exactly as Avian's `position_to_transform` does
//! for a root body, and writes `clean + offset`. The previous formulation was idempotent only
//! because that writeback is gated on `Or<(Changed<Position>, Changed<Rotation>)>`
//! (`avian3d-0.7.0/src/physics_transform/mod.rs`) and lightyear's `visual_interpolation` happens to
//! mark `Position` changed every frame; a frame where neither fired would have compounded the offset
//! onto an already-offset `Transform`. Re-deriving removes the dependency instead of documenting it.
//! `arm_render_error` additionally refuses to arm a root that frame interpolation has not armed, so
//! the two predicates cannot diverge in the direction that matters.
//!
//! `Without<ChildOf>` mirrors the branch of `position_to_transform` this formula reproduces. A
//! parented rigid body takes the reparenting branch, which this arithmetic does not reproduce, so
//! such a root is left UNSMOOTHED rather than mispositioned. The predicted tank root is spawned at
//! the top level (`tank::spawn`).

use avian3d::math::AsF32;
use avian3d::prelude::{PhysicsSystems, Position, Rotation};
use bevy::ecs::message::Messages;
use bevy::prelude::*;
use lightyear::frame_interpolation::FrameInterpolate;
use lightyear::prelude::{Predicted, RollbackSystems, VisualCorrection};

use super::adoption::SharpCorrection;
use super::protocol::NetTank;

/// Presentation-only correction accumulated on the predicted root.
#[derive(Component, Default)]
pub struct RenderErrorOffset {
    pub translation: Vec3,
    pub rotation: Quat,
}

// Frame-rate-normalized decay dials.
const DECAY_RETAIN_NEAR: f32 = 0.95;
const DECAY_RETAIN_FAR: f32 = 0.85;
/// Translation error (m) at/below which decay is NEAR-slow, and at/above which it is FAR-fast.
const DECAY_LERP_LO_M: f32 = 0.25;
const DECAY_LERP_HI_M: f32 = 1.0;
/// Rotation-error bracket for adaptive decay.
const DECAY_LERP_LO_RAD: f32 = 0.25;
const DECAY_LERP_HI_RAD: f32 = 1.0;
/// Maximum presentation-only correction speed.
const CAP_TRANSLATION_MPS: f32 = 3.0;
const CAP_ROTATION_DPS: f32 = 120.0;
/// Offsets beyond this threshold are consumed without smoothing.
const SNAP_TRANSLATION_M: f32 = 2.0;
const SNAP_ROTATION_DEG: f32 = 60.0;
/// Below this the offset is treated as spent and zeroed, so it never lingers as denormal dust.
const ZERO_EPS_M: f32 = 1e-4;
const ZERO_EPS_RAD: f32 = 1e-4;

/// Ordering owner for presentation smoothing after writeback and before camera/propagation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderErrorApplied;

/// Install client-side predicted-root smoothing.
pub fn plugin(app: &mut App) {
    app.add_systems(Update, arm_render_error);
    // INSIDE THE ROLLBACK TRANSACTION. `update_frame_interpolation_post_rollback` inserts
    // `VisualCorrection` in `RollbackSystems::EndRollback` through `Commands`, and
    // `net::adoption::confirm_forced_rollback` writes its occurrence between `Prepare` and
    // `Rollback` — both strictly before this seam, in the same `PreUpdate`.
    app.add_systems(
        PreUpdate,
        capture_render_error.after(RollbackSystems::EndRollback),
    );
    app.add_systems(
        PostUpdate,
        apply_render_error
            .in_set(RenderErrorApplied)
            .after(PhysicsSystems::Writeback)
            .before(TransformSystems::Propagate),
    );
    // The camera must consume the same presentation pose as the hull.
    app.configure_sets(
        PostUpdate,
        crate::camera::OrbitCameraSet.after(RenderErrorApplied),
    );
    // So must the track view (links/wheels are written FROM the presented root pose). The edge
    // lives here, not in `track::view`, because the net-boundary guard keeps that module from
    // naming the netcode; in SP the set simply lacks this constraint.
    app.configure_sets(
        PostUpdate,
        crate::track::view::TrackViewSet.after(RenderErrorApplied),
    );
}

/// Arm a predicted root once both replication markers AND frame interpolation are on it.
///
/// The `FrameInterpolate` requirements are not decoration. They make this module's arming a strict
/// subset of `net::rig::arm_predicted_smoothing`'s, which is what keeps two independently written
/// predicates from disagreeing: a root without frame interpolation gets no `VisualCorrection` from
/// lightyear (`update_frame_interpolation_post_rollback` queries `&mut FrameInterpolate<C>`), so an
/// armed offset there could never be captured — and its `Position` would not be marked changed every
/// frame, which is the condition Avian's writeback gate needs. [`apply_render_error`] no longer
/// depends on that gate, but arming past it would still be arming something that cannot work.
fn arm_render_error(
    tanks: Query<
        Entity,
        (
            With<Predicted>,
            With<NetTank>,
            With<FrameInterpolate<Position>>,
            With<FrameInterpolate<Rotation>>,
            Without<RenderErrorOffset>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!("net: {entity} predicted root armed with render-space error offset");
        commands.entity(entity).insert(RenderErrorOffset::default());
    }
}

/// Consume this rollback's same-tick correction: accumulate it, or refuse it as a delivered hit.
///
/// CONSUME is literal for both inputs. The occurrence queue is DRAINED unconditionally, so an
/// occurrence naming a root that no longer exists — or that this rollback did not correct — cannot
/// survive to sharpen an unrelated correction on a later frame. The `VisualCorrection` components
/// are REMOVED from every armed root that had one, so the same correction cannot be read twice:
/// lightyear's own `add_visual_correction` only removes them once the decayed error falls under the
/// registered rollback condition, which leaves the component alive into the next frame's
/// `PreUpdate`. Removing them here also retires lightyear's `PostUpdate` application for these
/// roots, which `CorrectionPolicy::instant_correction` had already reduced to ~0.
fn capture_render_error(
    mut occurrences: ResMut<Messages<SharpCorrection>>,
    mut roots: Query<(
        Entity,
        &mut RenderErrorOffset,
        Option<&VisualCorrection<Position>>,
        Option<&VisualCorrection<Rotation>>,
    )>,
    mut commands: Commands,
) {
    // FIRST, and whatever the query holds. A drain that depended on a matching root would leave the
    // occurrence queued exactly in the cases where no root matched it.
    let sharp: Vec<SharpCorrection> = occurrences.drain().collect();

    for (entity, mut offset, translation_error, rotation_error) in &mut roots {
        if translation_error.is_none() && rotation_error.is_none() {
            continue;
        }
        // `try_remove`, not `remove`: a root despawned between this system and the schedule's sync
        // point is a lifecycle event, not an error to report, and the components are going with it.
        commands
            .entity(entity)
            .try_remove::<(VisualCorrection<Position>, VisualCorrection<Rotation>)>();
        // KEYED ON THE WHOLE `Entity`, generation included: a despawned victim's occurrence must
        // not match the root that replaced it.
        if let Some(occurrence) = sharp.iter().find(|occurrence| occurrence.entity == entity) {
            debug!(
                "net: {entity} rollback correction left SHARP — the restore from tick {} carries \
                 an adopted authoritative event",
                occurrence.restored_from.0,
            );
            continue;
        }
        if let Some(error) = translation_error {
            offset.translation += error.error.f32();
        }
        if let Some(error) = rotation_error {
            offset.rotation = (offset.rotation * error.error.f32()).normalize();
        }
    }
}

/// Decay the classified offset and present it, re-derived from the sim pose.
fn apply_render_error(
    time: Res<Time<Real>>,
    mut roots: Query<
        (&mut Transform, &Position, &Rotation, &mut RenderErrorOffset),
        Without<ChildOf>,
    >,
) {
    let dt = time.delta_secs();

    for (mut transform, position, rotation, mut offset) in &mut roots {
        decay_translation(&mut offset.translation, dt);
        decay_rotation(&mut offset.rotation, dt);

        // Avian's `position_to_transform` root branch, verbatim, plus the offset. Written through
        // `set_if_neq` so a spent offset neither writes nor dirties the root — which would propagate
        // through the tank's ~194 link children for no visual difference. The zero cases are spelled
        // out rather than folded into the arithmetic so the written value is BIT-IDENTICAL to
        // Avian's, which is what makes the comparison skip.
        let clean_translation = position.f32();
        let clean_rotation = rotation.f32();
        let presented = Transform {
            translation: if offset.translation == Vec3::ZERO {
                clean_translation
            } else {
                clean_translation + offset.translation
            },
            rotation: if offset.rotation == Quat::IDENTITY {
                clean_rotation
            } else {
                (offset.rotation * clean_rotation).normalize()
            },
            scale: transform.scale,
        };
        transform.set_if_neq(presented);
    }
}

/// Frame-rate-normalized, capped decay shared by translation and rotation.
fn decay_magnitude(mag: f32, lo: f32, hi: f32, cap: f32, snap: f32, dt: f32) -> f32 {
    if mag > snap {
        return 0.0;
    }
    let t = ((mag - lo) / (hi - lo)).clamp(0.0, 1.0);
    let retain = (DECAY_RETAIN_NEAR + (DECAY_RETAIN_FAR - DECAY_RETAIN_NEAR) * t).powf(dt * 60.0);
    let reduction = (mag - mag * retain).min(cap * dt);
    (mag - reduction).max(0.0)
}

fn decay_translation(offset: &mut Vec3, dt: f32) {
    let mag = offset.length();
    if mag <= ZERO_EPS_M {
        *offset = Vec3::ZERO;
        return;
    }
    let new_mag = decay_magnitude(
        mag,
        DECAY_LERP_LO_M,
        DECAY_LERP_HI_M,
        CAP_TRANSLATION_MPS,
        SNAP_TRANSLATION_M,
        dt,
    );
    *offset *= new_mag / mag;
}

fn decay_rotation(offset: &mut Quat, dt: f32) {
    // Use the shortest-path quaternion representative.
    let mut q = *offset;
    if q.w < 0.0 {
        q = -q;
    }
    let angle = 2.0 * q.w.clamp(-1.0, 1.0).acos();
    if angle <= ZERO_EPS_RAD {
        *offset = Quat::IDENTITY;
        return;
    }
    let new_angle = decay_magnitude(
        angle,
        DECAY_LERP_LO_RAD,
        DECAY_LERP_HI_RAD,
        CAP_ROTATION_DPS.to_radians(),
        SNAP_ROTATION_DEG.to_radians(),
        dt,
    );
    if new_angle <= ZERO_EPS_RAD {
        *offset = Quat::IDENTITY;
        return;
    }
    *offset = Quat::IDENTITY.slerp(q, new_angle / angle).normalize();
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use avian3d::prelude::{Position, Rotation};
    use bevy::ecs::message::Messages;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use lightyear::core::confirmed_history::ConfirmedHistory;
    use lightyear::prelude::client::{Client, ClientPlugins, Connected};
    use lightyear::prelude::{
        Diffable, InputTimeline, IsSynced, LocalTimeline, PeerId, Predicted, PredictionHistory,
        RemoteId, RollbackSystems, StateRollbackMetadata, Tick,
    };

    use super::*;
    use crate::net::adoption::{AdoptionCause, ForcedRollbackSlot};
    use crate::net::test_harness::TICK;

    /// The tick a fixture rollback restores from, and the tick the client is on when it lands.
    ///
    /// ONE TICK OF DEPTH, deliberately. It is the shallowest rollback for which lightyear produces
    /// a `VisualCorrection` at all — `update_frame_interpolation_post_rollback` needs
    /// `PredictionHistory::get(tick - 1)` to resolve, and `prepare_rollback` re-seeds the history
    /// with a single entry at the rollback tick — so it is also the boundary the zero-depth fixture
    /// sits one tick below.
    const ROLLBACK_TICK: Tick = Tick(100);
    const CURRENT_TICK: Tick = Tick(101);

    /// The pose the client had on screen when the rollback landed.
    const PREDICTED_POSITION: Vec3 = Vec3::new(10.0, 2.0, -3.0);
    /// The pose the authority's confirmed sample restores it to. 0.206 m away, which is inside the
    /// NEAR decay bracket and well under the snap threshold.
    const AUTHORITY_POSITION: Vec3 = Vec3::new(10.2, 2.0, -3.05);
    /// A second authority pose, for the fixture that lands two rollbacks in a row.
    const LATER_AUTHORITY_POSITION: Vec3 = Vec3::new(10.35, 2.0, -3.02);

    /// Rotations about DIFFERENT axes, so left- and right-composing the captured delta give
    /// visibly different results. `the_captured_rotation_error_is_a_left_delta` depends on that.
    fn predicted_rotation() -> Quat {
        Quat::from_rotation_y(0.10)
    }

    fn authority_rotation() -> Quat {
        Quat::from_rotation_x(0.06)
    }

    /// What `apply_render_error` has to ADD to the corrected pose to keep the previous one on
    /// screen. Written as the presentation identity rather than as a copy of lightyear's `diff`,
    /// so `the_captured_error_reproduces_the_previous_displayed_pose` can hold the two against
    /// each other.
    fn expected_translation_error() -> Vec3 {
        PREDICTED_POSITION - AUTHORITY_POSITION
    }

    fn expected_rotation_error() -> Quat {
        predicted_rotation() * authority_rotation().inverse()
    }

    /// A forced rollback ordered through the PRODUCTION slot, exactly as `net::watchdog` orders one
    /// — never by poking `StateRollbackMetadata::request_forced_rollback`, which `net::adoption`'s
    /// source scan pins as having a single production caller.
    #[derive(Resource, Default)]
    struct PendingRollback(Option<Tick>);

    fn order_the_rollback(
        mut pending: ResMut<PendingRollback>,
        mut metadata: ResMut<StateRollbackMetadata>,
        mut slot: ResMut<ForcedRollbackSlot>,
    ) {
        let Some(tick) = pending.0.take() else {
            return;
        };
        assert!(
            slot.claim(&mut metadata, tick, AdoptionCause::Misprediction),
            "the fixture's own claim must own the slot, or it is not the rollback under test",
        );
    }

    /// Presentation occurrences to emit at exactly the schedule point
    /// `net::adoption::confirm_forced_rollback` emits them from.
    ///
    /// The EMISSION RULE — which retirements produce one — is pinned where it lives:
    /// `net::adoption::only_the_two_retirements_that_delivered_the_fact_keep_the_seam_sharp` over
    /// every outcome, and `net::lead_zero_rollback` end to end over a real adopted, bypassed and
    /// undelivered-hull run. What these fixtures pin is the CONSUMPTION: that an occurrence written
    /// there reaches the capture below, once, on the frame it was written.
    #[derive(Resource, Default)]
    struct PendingSharp(Vec<Entity>);

    fn emit_sharp_occurrences(
        mut pending: ResMut<PendingSharp>,
        mut sharp: MessageWriter<SharpCorrection>,
    ) {
        for entity in core::mem::take(&mut pending.0) {
            sharp.write(SharpCorrection {
                entity,
                restored_from: ROLLBACK_TICK,
            });
        }
    }

    /// Ordinary forward motion the replay produces, applied by a `FixedUpdate` system so it lands
    /// on exactly the replayed ticks — this app only ever runs `FixedMain` from inside
    /// `run_rollback`. It exists so a capture that folded in a frame of travel would be visibly
    /// wrong instead of merely imprecise.
    #[derive(Resource, Default)]
    struct ReplayTravel(Vec3);

    fn travel_during_replay(travel: Res<ReplayTravel>, mut roots: Query<&mut Position>) {
        if travel.0 == Vec3::ZERO {
            return;
        }
        for mut position in &mut roots {
            position.0 += travel.0;
        }
    }

    /// A real lightyear client on the PRODUCTION registration, with this module's plugin mounted.
    ///
    /// The registration matters: `prepare_rollback` stores `PreviousVisual<C>` only when
    /// `PredictionRegistry::has_correction::<C>()`, and `update_frame_interpolation_post_rollback`
    /// runs only for components registered through `add_correction_fn`. Both come from
    /// `net::protocol`'s `.add_linear_correction_fn()`, so a fixture that registered its own
    /// components would be testing a seam the game does not have.
    fn client_app() -> App {
        // THE CLIENT'S PHYSICS COMPOSITION, not Avian's default. Avian's `PhysicsTransformPlugin`
        // syncs `Transform` back into `Position` inside `FixedPostUpdate`, which `run_rollback`
        // executes on every replayed tick — it would undo the restore this whole fixture is about.
        let mut app = crate::net::test_harness::net_physics_app();
        app.add_plugins(ClientPlugins {
            tick_duration: crate::net::test_harness::TICK,
        });
        crate::state::sim_plugin(&mut app);
        crate::net::protocol::plugin(&mut app);
        app.insert_state(crate::state::AppState::Playing);
        app.add_plugins(plugin);
        app.init_resource::<PendingRollback>();
        app.init_resource::<PendingSharp>();
        app.init_resource::<ReplayTravel>();
        app.add_systems(PreUpdate, order_the_rollback.before(RollbackSystems::Check));
        // WHERE `confirm_forced_rollback` SITS: after the restore is established, before the replay.
        app.add_systems(
            PreUpdate,
            emit_sharp_occurrences
                .after(RollbackSystems::Prepare)
                .before(RollbackSystems::Rollback),
        );
        app.add_systems(FixedUpdate, travel_during_replay);
        crate::net::test_harness::finish(&mut app);

        app.world_mut().spawn((
            Client::default(),
            RemoteId(PeerId::Server),
            Connected,
            crate::net::test_harness::prediction_manager(),
            IsSynced::<InputTimeline>::default(),
        ));
        advance_to(&mut app, CURRENT_TICK);
        app
    }

    /// A predicted root as `net::rig` leaves one: rollback-eligible, frame-interpolated, and armed.
    fn spawn_armed_root(app: &mut App) -> Entity {
        let mut predicted_position = PredictionHistory::<Position>::default();
        predicted_position.add_predicted(ROLLBACK_TICK, Some(Position(PREDICTED_POSITION)));
        let mut predicted_rotation_history = PredictionHistory::<Rotation>::default();
        predicted_rotation_history
            .add_predicted(ROLLBACK_TICK, Some(Rotation(predicted_rotation())));
        let root = app
            .world_mut()
            .spawn((
                Predicted,
                Position(PREDICTED_POSITION),
                Rotation(predicted_rotation()),
                predicted_position,
                predicted_rotation_history,
                ConfirmedHistory::<Position>::default(),
                ConfirmedHistory::<Rotation>::default(),
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
                RenderErrorOffset::default(),
                Transform::default(),
            ))
            .id();
        app.world_mut().flush();
        root
    }

    /// Deposit the authority's pose at `at`, which is what a restore targeting `at` resolves.
    fn confirm_pose(app: &mut App, root: Entity, at: Tick, position: Vec3, rotation: Quat) {
        app.world_mut()
            .get_mut::<ConfirmedHistory<Position>>(root)
            .expect("the root's confirmed position history")
            .insert_present_explicit(at, Position(position));
        app.world_mut()
            .get_mut::<ConfirmedHistory<Rotation>>(root)
            .expect("the root's confirmed rotation history")
            .insert_present_explicit(at, Rotation(rotation));
    }

    /// Walk both the tick counter and `Time<Fixed>` forward, keeping them consistent — the rollback
    /// path reads the timeline and the replay loop reads the fixed clock.
    fn advance_to(app: &mut App, target: Tick) {
        let current = app.world().resource::<LocalTimeline>().tick();
        let steps = target - current;
        assert!(steps >= 0, "the fixture only ever moves the clock forward");
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(steps);
        for _ in 0..steps {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(TICK);
        }
    }

    fn order_rollback(app: &mut App, tick: Tick) {
        app.world_mut().resource_mut::<PendingRollback>().0 = Some(tick);
    }

    fn order_sharp(app: &mut App, entity: Entity) {
        app.world_mut()
            .resource_mut::<PendingSharp>()
            .0
            .push(entity);
    }

    /// One client frame's `PreUpdate`: lightyear's whole rollback transaction, with this module's
    /// capture inside it.
    fn run_pre_update(app: &mut App) {
        app.world_mut().run_schedule(PreUpdate);
    }

    fn offset_of(app: &App, root: Entity) -> (Vec3, Quat) {
        let offset = app
            .world()
            .get::<RenderErrorOffset>(root)
            .expect("the root stays armed");
        (offset.translation, offset.rotation)
    }

    fn assert_near(actual: Vec3, expected: Vec3, what: &str) {
        assert!(
            (actual - expected).length() < 1e-5,
            "{what}: expected {expected:?}, got {actual:?}",
        );
    }

    /// Compared on the DOT PRODUCT rather than through `Quat::angle_between`, which is
    /// `2·acos(|dot|)` and is badly conditioned for f32 exactly where these fixtures live: two
    /// quaternions that agree to the last printed digit can still report 3e-4 rad of separation.
    /// `|dot|` handles the double-cover the same way and stays well-conditioned near identity.
    fn assert_near_rotation(actual: Quat, expected: Quat, what: &str) {
        let alignment = actual.dot(expected).abs();
        assert!(
            alignment > 1.0 - 1e-6,
            "{what}: expected {expected:?}, got {actual:?} (|dot| {alignment})",
        );
    }

    /// THE CAPTURE, and the thing the count-based detector it replaced could not do: the offset is
    /// the rollback's own discontinuity and contains NONE of the ordinary motion the replay
    /// produced on the same frame.
    ///
    /// The fixture makes those two separable by construction. The replay advances the hull by
    /// [`ReplayTravel`], so the post-rollback `Position` is the authority's restored pose PLUS a
    /// tick of travel — while the correction lightyear measures is taken over one tick pair at one
    /// overstep and is exactly `previous displayed − corrected`. A capture that diffed the previous
    /// frame's displayed pose against the current corrected one, which is what this module did
    /// before slice 4, would land `expected + travel` here.
    #[test]
    fn the_captured_offset_is_the_rollback_discontinuity_and_excludes_the_replays_own_travel() {
        const TRAVEL: Vec3 = Vec3::new(0.0, 0.0, 0.4);
        let mut app = client_app();
        app.world_mut().resource_mut::<ReplayTravel>().0 = TRAVEL;
        let root = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );

        order_rollback(&mut app, ROLLBACK_TICK);
        run_pre_update(&mut app);

        let live = app.world().get::<Position>(root).expect("live position").0;
        assert_near(
            live,
            AUTHORITY_POSITION + TRAVEL,
            "the replay must have restored the authority pose AND moved the hull on from it, or \
             this fixture cannot tell travel from correction",
        );
        let (translation, rotation) = offset_of(&app, root);
        assert_near(
            translation,
            expected_translation_error(),
            "the offset must hold the correction ALONE — the replay's own travel belongs to the \
             sim and must never be held back on screen",
        );
        assert_near_rotation(
            rotation,
            expected_rotation_error(),
            "the rotation offset must be the left delta that carries the corrected pose back to \
             the previously displayed one",
        );
    }

    /// ONE-SHOT. `VisualCorrection` is a component that lightyear leaves on the entity until its
    /// own decayed error falls under the registered rollback condition — which is the NEXT frame's
    /// `PostUpdate` at the earliest — so a reader that merely looked at it would count the same
    /// correction on every frame until then. The capture removes it, and a later frame with no
    /// rollback therefore adds nothing.
    #[test]
    fn a_correction_is_accumulated_once_and_a_later_frame_without_a_rollback_adds_nothing() {
        let mut app = client_app();
        let root = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );

        order_rollback(&mut app, ROLLBACK_TICK);
        run_pre_update(&mut app);
        let after_rollback = offset_of(&app, root);
        assert_near(
            after_rollback.0,
            expected_translation_error(),
            "the rollback frame must capture the correction",
        );
        assert!(
            app.world()
                .get::<VisualCorrection<Position>>(root)
                .is_none()
                && app
                    .world()
                    .get::<VisualCorrection<Rotation>>(root)
                    .is_none(),
            "the consumed correction must be REMOVED, not left for the next frame to read again",
        );

        advance_to(&mut app, CURRENT_TICK + 1);
        run_pre_update(&mut app);

        assert_eq!(
            offset_of(&app, root).0,
            after_rollback.0,
            "a frame with no rollback must add nothing at all",
        );
        assert_eq!(offset_of(&app, root).1, after_rollback.1);
    }

    /// TWO ROLLBACKS, TWO CORRECTIONS. The offset already on the root is not reset and not
    /// re-counted: the second frame adds its own same-tick error to it and nothing else.
    ///
    /// `PostUpdate` never runs here, so no decay stands between the two captures and the sum is
    /// exact. That is the point — a fixture that let decay run could not tell "added once" from
    /// "added twice and decayed".
    #[test]
    fn a_second_rollback_adds_its_own_correction_to_the_offset_already_present() {
        let mut app = client_app();
        let root = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );

        order_rollback(&mut app, ROLLBACK_TICK);
        run_pre_update(&mut app);
        let first = offset_of(&app, root).0;

        // The second rollback restores a LATER authority sample, from the pose the first one left.
        advance_to(&mut app, CURRENT_TICK + 1);
        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK,
            LATER_AUTHORITY_POSITION,
            authority_rotation(),
        );
        order_rollback(&mut app, CURRENT_TICK);
        run_pre_update(&mut app);

        let second_error = AUTHORITY_POSITION - LATER_AUTHORITY_POSITION;
        assert_near(
            offset_of(&app, root).0,
            first + second_error,
            "the second correction must be ADDED to the first, once",
        );
    }

    /// SHARP. A correction the occurrence names carries an authoritative event the client could not
    /// predict, and the corrected pose already contains it: accumulating any offset at all would
    /// hold the hit back, so none is accumulated.
    #[test]
    fn a_correction_named_by_a_sharp_occurrence_is_not_accumulated_at_all() {
        let mut app = client_app();
        let root = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );

        order_rollback(&mut app, ROLLBACK_TICK);
        order_sharp(&mut app, root);
        run_pre_update(&mut app);

        let (translation, rotation) = offset_of(&app, root);
        assert_eq!(
            translation,
            Vec3::ZERO,
            "a delivered hit must reach the screen on the frame it lands — not decayed in, not \
             decayed in faster",
        );
        assert_eq!(rotation, Quat::IDENTITY);
        assert!(
            app.world()
                .get::<VisualCorrection<Position>>(root)
                .is_none(),
            "a refused correction must still be consumed, or the next frame smooths it after all",
        );
    }

    /// A rollback is world-wide and sharpness is per-root. The root the occurrence names goes
    /// sharp; every other root corrected by the same rollback smooths normally.
    #[test]
    fn a_root_the_occurrence_does_not_name_is_still_smoothed_by_the_same_rollback() {
        let mut app = client_app();
        let shot = spawn_armed_root(&mut app);
        let bystander = spawn_armed_root(&mut app);
        for root in [shot, bystander] {
            confirm_pose(
                &mut app,
                root,
                ROLLBACK_TICK,
                AUTHORITY_POSITION,
                authority_rotation(),
            );
        }

        order_rollback(&mut app, ROLLBACK_TICK);
        order_sharp(&mut app, shot);
        run_pre_update(&mut app);

        assert_eq!(offset_of(&app, shot).0, Vec3::ZERO);
        assert_near(
            offset_of(&app, bystander).0,
            expected_translation_error(),
            "the other predicted root in the same rollback was not shot, and its seam is an \
             ordinary misprediction the view should still hide",
        );
    }

    /// TWO OCCURRENCES, ONE READER PASS. Both are written before the capture runs, and the capture
    /// runs once — a consumer that took only the first (or that collapsed them into one global
    /// answer) would leave the second root smoothing a hit away.
    #[test]
    fn two_occurrences_written_in_one_frame_each_sharpen_their_own_root() {
        let mut app = client_app();
        let first = spawn_armed_root(&mut app);
        let second = spawn_armed_root(&mut app);
        let bystander = spawn_armed_root(&mut app);
        for root in [first, second, bystander] {
            confirm_pose(
                &mut app,
                root,
                ROLLBACK_TICK,
                AUTHORITY_POSITION,
                authority_rotation(),
            );
        }

        order_rollback(&mut app, ROLLBACK_TICK);
        order_sharp(&mut app, first);
        order_sharp(&mut app, second);
        run_pre_update(&mut app);

        assert_eq!(offset_of(&app, first).0, Vec3::ZERO);
        assert_eq!(offset_of(&app, second).0, Vec3::ZERO);
        assert_near(
            offset_of(&app, bystander).0,
            expected_translation_error(),
            "and the root neither occurrence named is untouched by either of them",
        );
    }

    /// An offset ALREADY DECAYING on a root that then takes a delivered hit keeps decaying: only
    /// this rollback's delta is refused. Zeroing it instead would snap away a seam the player has
    /// been watching smooth out, which is a second discontinuity for free.
    #[test]
    fn a_sharp_rollback_refuses_only_its_own_delta_and_leaves_an_older_offset_alone() {
        let mut app = client_app();
        let root = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );

        order_rollback(&mut app, ROLLBACK_TICK);
        run_pre_update(&mut app);
        let older = offset_of(&app, root);
        assert_ne!(older.0, Vec3::ZERO, "the fixture needs a live offset first");

        advance_to(&mut app, CURRENT_TICK + 1);
        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK,
            LATER_AUTHORITY_POSITION,
            authority_rotation(),
        );
        order_rollback(&mut app, CURRENT_TICK);
        order_sharp(&mut app, root);
        run_pre_update(&mut app);

        assert_eq!(
            offset_of(&app, root).0,
            older.0,
            "the older offset must be neither reset nor added to",
        );
        assert_eq!(offset_of(&app, root).1, older.1);
    }

    /// The PREVIOUS incarnation of `entity`: the same entity INDEX with the generation stepped
    /// back, which is what a despawn/respawn that reuses the index leaves behind.
    ///
    /// Built from the bits rather than by cycling the allocator, because Bevy does not promise to
    /// hand an index straight back on the next spawn — in this fixture it does not — and a test
    /// that could only reach two DIFFERENT indices would prove nothing about generations at all.
    fn previous_incarnation_of(entity: Entity) -> Entity {
        let previous = Entity::from_bits(entity.to_bits().wrapping_sub(1 << 32));
        assert_eq!(
            previous.index(),
            entity.index(),
            "the stale id must share the live root's INDEX, or the fixture is only asserting that \
             an unrelated entity is ignored",
        );
        assert_ne!(previous, entity);
        previous
    }

    /// GENERATIONS. An occurrence naming a despawned root cannot sharpen the replacement that
    /// reuses its index. Bevy's `Entity` carries the generation and the membership test compares
    /// the whole value, which is why the signal names an entity rather than merely saying "this
    /// rollback delivered a hit".
    #[test]
    fn an_occurrence_naming_the_previous_incarnation_of_an_index_cannot_sharpen_the_current_one() {
        let mut app = client_app();
        let root = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );

        order_rollback(&mut app, ROLLBACK_TICK);
        order_sharp(&mut app, previous_incarnation_of(root));
        run_pre_update(&mut app);

        assert_near(
            offset_of(&app, root).0,
            expected_translation_error(),
            "the live root inherited a previous incarnation's sharpness — nothing about THIS \
             root's correction carries a hit",
        );
    }

    /// THE QUEUE NEVER SURVIVES A FRAME, whether or not anything matched. That is what makes this
    /// signal impossible to consume at the wrong schedule point, and it is also the whole answer to
    /// "what happens across a reconnect": there is no state to carry over.
    #[test]
    fn an_occurrence_is_drained_on_the_frame_it_is_written_even_with_no_rollback_to_apply_it_to() {
        let mut app = client_app();
        let root = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );

        // A frame with an occurrence and no rollback at all.
        order_sharp(&mut app, root);
        run_pre_update(&mut app);
        assert!(
            app.world()
                .resource::<Messages<SharpCorrection>>()
                .is_empty(),
            "the occurrence must be consumed on its own frame, not left queued",
        );

        // The next frame's rollback carries no hit, so it must be smoothed like any other.
        advance_to(&mut app, CURRENT_TICK + 1);
        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );
        order_rollback(&mut app, CURRENT_TICK);
        run_pre_update(&mut app);

        assert_ne!(
            offset_of(&app, root).0,
            Vec3::ZERO,
            "a stale occurrence must not sharpen a later, unrelated rollback",
        );
    }

    /// A ROOT CARRIES ITS OWN STATE AND NOTHING ELSE DOES. The detector this replaced kept a
    /// per-root mirror of the GLOBAL `PredictionMetrics::rollbacks` counter, so what a fresh root
    /// captured depended on how many rollbacks the session had already had. Nothing outside the
    /// component decides anything now, so a respawned root — which is what a reconnect produces —
    /// starts clean and captures nothing until a rollback gives it something to capture.
    #[test]
    fn a_root_respawned_after_earlier_rollbacks_starts_clean_and_captures_nothing_without_one() {
        let mut app = client_app();
        let first = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            first,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );
        order_rollback(&mut app, ROLLBACK_TICK);
        run_pre_update(&mut app);
        assert_ne!(offset_of(&app, first).0, Vec3::ZERO);

        app.world_mut().entity_mut(first).despawn();
        app.world_mut().flush();
        advance_to(&mut app, CURRENT_TICK + 1);
        let replacement = spawn_armed_root(&mut app);
        run_pre_update(&mut app);

        assert_eq!(
            offset_of(&app, replacement),
            (Vec3::ZERO, Quat::IDENTITY),
            "a fresh root with no rollback of its own must hold no offset",
        );
    }

    /// ZERO-DEPTH ROLLBACKS PRODUCE NO CORRECTION, and this module therefore accumulates nothing.
    ///
    /// `prepare_rollback` clears the prediction history and re-seeds it with one entry at the
    /// rollback tick, and `run_rollback` replays `current - rollback` times — zero here — so
    /// `update_frame_interpolation_post_rollback`'s `history.get(tick - 1)` misses and it returns
    /// before inserting `VisualCorrection`. The restore still HAPPENS, which is why this fixture
    /// asserts the live pose moved: "nothing captured" must mean "lightyear produced nothing", not
    /// "nothing rolled back".
    ///
    /// The result is right for the only depth-0 rollback this game produces — `net::adoption`'s, at
    /// the shipping loopback lead of 0, which must be sharp anyway — and that is a COINCIDENCE OF
    /// TWO MECHANISMS. This test is what fails if lightyear starts emitting a correction here, at
    /// which point adopted shoves would begin to be smoothed and the sharp rule would have to cover
    /// the case.
    #[test]
    fn a_zero_depth_rollback_produces_no_visual_correction_so_nothing_is_accumulated() {
        let mut app = client_app();
        let root = spawn_armed_root(&mut app);
        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );

        order_rollback(&mut app, CURRENT_TICK);
        run_pre_update(&mut app);

        assert_near(
            app.world().get::<Position>(root).expect("live position").0,
            AUTHORITY_POSITION,
            "the restore must have happened, or this fixture is asserting nothing",
        );
        assert!(
            app.world()
                .get::<VisualCorrection<Position>>(root)
                .is_none(),
            "lightyear must not have produced a correction at zero depth — if it now does, this \
             module has to decide what to do with it",
        );
        assert_eq!(offset_of(&app, root), (Vec3::ZERO, Quat::IDENTITY));
    }

    /// THE SIGN CONVENTION, held against lightyear's own `Diffable` impls rather than against a
    /// restatement of them. `update_frame_interpolation_post_rollback` computes
    /// `current_visual.diff(&previous_visual)`, and what this module needs is the value that carries
    /// the corrected pose back to the previously displayed one.
    #[test]
    fn the_captured_error_reproduces_the_previous_displayed_pose() {
        let corrected = Position(AUTHORITY_POSITION);
        let previous = Position(PREDICTED_POSITION);
        let error = corrected.diff(&previous);

        assert_eq!(
            error.0,
            corrected.0 + error.0 - corrected.0,
            "sanity: the delta is a plain vector difference",
        );
        assert_near(
            corrected.0 + error.0,
            previous.0,
            "ADDING the captured error to the corrected pose must reproduce the previous displayed \
             pose — which is exactly what `apply_render_error` does with the accumulated offset",
        );
        assert_eq!(
            error.0,
            expected_translation_error(),
            "and it must be the same quantity the schedule fixtures expect",
        );
    }

    /// The rotation half, and the upstream asymmetry it must not be routed through.
    ///
    /// `Diffable::diff` for `Rotation` is `new * self.inverse()` — a LEFT delta — while the same
    /// impl's `apply_diff` is `self.0 *= delta.0`, a RIGHT multiplication. Those are not the same
    /// rotation. This module composes the delta itself, on the left; nothing here calls
    /// `apply_diff`, and this fixture pins WHY by showing the right-composition landing somewhere
    /// else entirely.
    #[test]
    fn the_captured_rotation_error_is_a_left_delta_and_right_composing_it_is_a_different_rotation()
    {
        let corrected = Rotation(authority_rotation());
        let previous = Rotation(predicted_rotation());
        let error = corrected.diff(&previous);

        assert_near_rotation(
            error.0 * corrected.0,
            previous.0,
            "LEFT-composing the captured error onto the corrected pose must reproduce the previous \
             displayed pose",
        );
        assert!(
            (corrected.0 * error.0).angle_between(previous.0) > 1e-3,
            "and RIGHT-composing it — which is what `Diffable::apply_diff` does — must not, or this \
             fixture is asserting nothing about the asymmetry",
        );
        assert_near_rotation(
            error.0,
            expected_rotation_error(),
            "and it must be the same quantity the schedule fixtures expect",
        );
    }

    /// ARMING IS A SUBSET OF FRAME INTERPOLATION'S. Without `FrameInterpolate`, lightyear produces
    /// no `VisualCorrection` for a root at all (`update_frame_interpolation_post_rollback` queries
    /// `&mut FrameInterpolate<C>`), so an offset armed there could never be captured — and the
    /// `Position` change that Avian's writeback gate depends on would stop being marked every frame.
    /// The two arming predicates are written in different modules and are not ordered against each
    /// other, so this one is made the narrower of the two rather than merely documented.
    #[test]
    fn a_predicted_root_is_not_armed_before_frame_interpolation_is_armed_on_it() {
        let mut world = World::new();
        let root = world.spawn((Predicted, NetTank)).id();

        world
            .run_system_once(arm_render_error)
            .expect("the arming system runs");
        assert!(
            world.get::<RenderErrorOffset>(root).is_none(),
            "a root frame interpolation has not reached yet must not be armed",
        );

        world.entity_mut(root).insert((
            FrameInterpolate::<Position>::default(),
            FrameInterpolate::<Rotation>::default(),
        ));
        world
            .run_system_once(arm_render_error)
            .expect("the arming system runs");
        assert!(
            world.get::<RenderErrorOffset>(root).is_some(),
            "and it must be armed once both interpolators are on it",
        );
    }

    /// A world with the presented pose already written the way Avian's writeback leaves it.
    fn presentation_world(offset: Vec3, rotation_offset: Quat, dt: Duration) -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<Time<Real>>();
        world.resource_mut::<Time<Real>>().advance_by(dt);
        let root = world
            .spawn((
                Transform {
                    translation: AUTHORITY_POSITION,
                    rotation: authority_rotation(),
                    scale: Vec3::ONE,
                },
                Position(AUTHORITY_POSITION),
                Rotation(authority_rotation()),
                RenderErrorOffset {
                    translation: offset,
                    rotation: rotation_offset,
                },
            ))
            .id();
        (world, root)
    }

    /// WHAT THE ROLLBACK FRAME ACTUALLY RENDERS, in all three decay regimes. The wording this
    /// replaces — "one frame of render freeze per rollback" — was wrong twice over: the offset is
    /// decayed BEFORE it is applied, so the frame is a near-hold and not a hold, and nothing about
    /// the sim is frozen at all (the sibling fixture below pins that half).
    #[test]
    fn the_rollback_frame_renders_a_decayed_near_hold_and_the_speed_cap_bounds_a_large_one() {
        let dt = TICK.as_secs_f32();

        // NEAR regime: 0.2 m, under `DECAY_LERP_LO_M`, so the retain factor alone decides.
        let (mut world, root) = presentation_world(Vec3::new(0.2, 0.0, 0.0), Quat::IDENTITY, TICK);
        world.run_system_once(apply_render_error).unwrap();
        let held = world.get::<RenderErrorOffset>(root).unwrap().translation.x;
        assert!(
            (0.94..0.96).contains(&(held / 0.2)),
            "a small correction must render as roughly 95% of the previous displayed pose, not as \
             a freeze — held {held} of 0.2 m",
        );
        assert!(held < 0.2, "and strictly less than all of it");
        assert_eq!(
            world.get::<Transform>(root).unwrap().translation,
            AUTHORITY_POSITION + Vec3::new(held, 0.0, 0.0),
            "the presented pose is the corrected pose plus the surviving offset",
        );

        // FAR regime: 1.0 m. The adaptive retain would remove 0.141 m, so the 3 m/s speed cap binds
        // first and the frame holds MORE than the retain factor alone would suggest.
        let (mut world, root) = presentation_world(Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY, TICK);
        world.run_system_once(apply_render_error).unwrap();
        let held = world.get::<RenderErrorOffset>(root).unwrap().translation.x;
        assert!(
            ((1.0 - held) - CAP_TRANSLATION_MPS * dt).abs() < 1e-5,
            "past roughly half a metre the correction VELOCITY cap decides the step, not the \
             adaptive retain — removed {} m, cap allows {} m",
            1.0 - held,
            CAP_TRANSLATION_MPS * dt,
        );

        // BEYOND THE SNAP THRESHOLD: not held at all.
        let (mut world, root) = presentation_world(
            Vec3::new(SNAP_TRANSLATION_M + 0.5, 0.0, 0.0),
            Quat::IDENTITY,
            TICK,
        );
        world.run_system_once(apply_render_error).unwrap();
        assert_eq!(
            world.get::<RenderErrorOffset>(root).unwrap().translation,
            Vec3::ZERO,
        );
        assert_eq!(
            world.get::<Transform>(root).unwrap().translation,
            AUTHORITY_POSITION,
            "past the snap threshold the corrected pose is presented immediately",
        );
    }

    /// THE SIM IS NEVER FROZEN. This layer writes `Transform` and nothing else; `Position` and
    /// `Rotation` — the rollback state the fixed loop, the replay and replication all read — come
    /// out of the frame exactly as the corrected sim left them.
    #[test]
    fn the_presentation_layer_never_writes_the_rollback_state_it_offsets() {
        let (mut world, root) =
            presentation_world(Vec3::new(0.2, 0.0, 0.0), Quat::from_rotation_z(0.2), TICK);
        world.run_system_once(apply_render_error).unwrap();

        assert_eq!(world.get::<Position>(root).unwrap().0, AUTHORITY_POSITION);
        assert_eq!(world.get::<Rotation>(root).unwrap().0, authority_rotation());
        assert_ne!(
            world.get::<Transform>(root).unwrap().translation,
            AUTHORITY_POSITION,
            "and the VIEW must be the thing that moved",
        );
    }

    /// IDEMPOTENCE WITHOUT A HIDDEN DEPENDENCY. The presented pose is re-derived from
    /// `Position`/`Rotation` every frame, so running the apply twice over a `Transform` Avian did
    /// NOT re-derive in between produces the same pose. The previous formulation added the offset to
    /// whatever `Transform` held, and was idempotent only because Avian's writeback is gated on
    /// `Or<(Changed<Position>, Changed<Rotation>)>` and lightyear's frame interpolation happens to
    /// mark `Position` changed every frame. A frame where neither fired compounded the offset.
    ///
    /// `dt` is zero so decay cannot mask the difference: with it, the second application would land
    /// at twice the offset instead of at the same pose.
    #[test]
    fn the_presented_pose_is_re_derived_so_a_missing_writeback_cannot_compound_the_offset() {
        let offset = Vec3::new(0.2, 0.0, -0.05);
        let (mut world, root) = presentation_world(offset, Quat::IDENTITY, Duration::ZERO);

        world.run_system_once(apply_render_error).unwrap();
        let once = *world.get::<Transform>(root).unwrap();
        world.run_system_once(apply_render_error).unwrap();
        let twice = *world.get::<Transform>(root).unwrap();

        assert_eq!(
            once.translation,
            AUTHORITY_POSITION + offset,
            "the fixture needs a live offset for the compounding to be visible",
        );
        assert_eq!(
            once, twice,
            "a second apply without an intervening writeback must present the SAME pose",
        );
    }

    /// A SPENT OFFSET DOES NOT DIRTY THE ROOT. The tank root has ~194 link children, so a
    /// `Transform` write it does not need costs a propagation pass through all of them, every
    /// frame, forever — the steady state is no offset at all.
    #[test]
    fn a_spent_offset_leaves_the_transform_unwritten_and_undirtied() {
        let (mut world, root) = presentation_world(Vec3::ZERO, Quat::IDENTITY, TICK);
        world.increment_change_tick();
        let before = world
            .entity(root)
            .get_ref::<Transform>()
            .unwrap()
            .last_changed();

        world.run_system_once(apply_render_error).unwrap();

        assert_eq!(
            world
                .entity(root)
                .get_ref::<Transform>()
                .unwrap()
                .last_changed(),
            before,
            "a zero offset re-derives to exactly what Avian already wrote, so nothing may be \
             written and change detection must not fire",
        );

        // The positive control: with an offset there IS something to write, and it is written.
        let (mut world, root) = presentation_world(Vec3::new(0.2, 0.0, 0.0), Quat::IDENTITY, TICK);
        world.increment_change_tick();
        let before = world
            .entity(root)
            .get_ref::<Transform>()
            .unwrap()
            .last_changed();
        world.run_system_once(apply_render_error).unwrap();
        assert_ne!(
            world
                .entity(root)
                .get_ref::<Transform>()
                .unwrap()
                .last_changed(),
            before,
        );
    }

    /// A `Transform` that has drifted from the sim pose is CORRECTED even when the offset is spent,
    /// because the presented pose is derived rather than accumulated. Skipping the write on a zero
    /// offset alone — without the equality check — would strand the root at the last offset pose
    /// forever on any frame Avian's writeback did not run.
    #[test]
    fn a_spent_offset_still_restores_a_transform_that_no_longer_matches_the_sim_pose() {
        let (mut world, root) = presentation_world(Vec3::ZERO, Quat::IDENTITY, TICK);
        world.get_mut::<Transform>(root).unwrap().translation = PREDICTED_POSITION;

        world.run_system_once(apply_render_error).unwrap();

        assert_eq!(
            world.get::<Transform>(root).unwrap().translation,
            AUTHORITY_POSITION,
        );
    }

    /// The track view detects discontinuities LOCALLY (pose delta per frame) because this
    /// module publishes no signal. That only works while its thresholds sit strictly below the
    /// snap thresholds here: a correction consumed unsmoothed (>= these bounds) must always
    /// exceed the track's trip point, or a snapped hull keeps its old wrap filter memory and the
    /// tracks settle in from a stale pose. Changing either side's constants must confront this
    /// bracket.
    #[test]
    #[allow(clippy::assertions_on_constants)] // constant is the point: a compile-time bracket
    fn track_discontinuity_thresholds_bracket_render_error_snaps() {
        assert!(crate::track::view::SNAP_TRANSLATION < SNAP_TRANSLATION_M);
        // The track compares AXIS CHORDS; a rotation snap of SNAP_ROTATION_DEG displaces at
        // least one basis axis by 2·sin(θ/2) in the worst-aligned case it must still catch.
        let snap_chord = 2.0 * (SNAP_ROTATION_DEG.to_radians() / 2.0).sin();
        assert!(crate::track::view::SNAP_AXIS < snap_chord);
    }
}
