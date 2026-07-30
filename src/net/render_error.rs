//! Client-side smoothing for rollback corrections.
//!
//! The offset is presentation-only: this module writes the predicted root's `Transform`, never its
//! rollback state (`Position`/`Rotation`). `apply_render_error` runs after Avian writeback and before
//! transform propagation, so the root, children, and camera share one rendered pose. That
//! PostUpdate ordering does not exclude replay travel from capture; the same-instant arithmetic in
//! PreUpdate below does.
//!
//! # THE CORRECTION IS LIGHTYEAR'S, MEASURED AT THE PREVIOUS DISPLAY INSTANT
//!
//! What must be smoothed is the DISCONTINUITY a rollback introduces, and nothing else. Until slice 4
//! this module derived it itself, by diffing `PredictionMetrics::rollbacks` and then taking the
//! PREVIOUS FRAME's displayed pose minus the current corrected one. That difference is the
//! correction PLUS one frame of legitimate travel, so the rollback frame rendered a heavy near-hold
//! of ordinary motion as well as of the correction, and the counter it keyed on is global — another
//! tank's rollback tripped every armed root, two rollbacks between observations collapsed into one,
//! and no attribution survived.
//!
//! Lightyear already captures the first half of the exact quantity at the instant that matters.
//! `lightyear_prediction::rollback::prepare_rollback` stores `PreviousVisual<C>` — the component
//! value as it stood before the restore, which at `PreUpdate` is the pose the previous frame
//! displayed. Capture then asks what Lightyear would display from the corrected timeline at that
//! SAME previous display instant, deliberately using the previous frame's fixed-time overstep
//! before `RunFixedMainLoop`:
//!
//! ```text
//! predecessor = history.get(tick - 1) or retained FrameInterpolate.previous_value
//! corrected_display = predecessor
//!     .map(|p| interpolate(p, component, overstep))
//!     .unwrap_or(component)
//! error = corrected_display.diff(&previous_visual)
//! ```
//!
//! Lightyear's own correction system documents this exact scheduling choice at
//! `lightyear_prediction-0.28.0/src/correction.rs:98`. Evaluating the corrected side at this frame's
//! later PostUpdate instant would subtract legitimate motion performed in `RunFixedMainLoop` and
//! turn it back into a near-hold. At positive rollback depth, replay rebuilds the predecessor in
//! history. At depth zero, the retained pre-rollback `FrameInterpolate.previous_value` supplies the
//! shared `tick - 1` baseline. Those are two sources for one optional input, not separate correction
//! strategies. If neither source supplies it, Lightyear's `visual_interpolation` skips and leaves the
//! component at its raw endpoint, so capture compares the raw predicted and corrected endpoints on
//! both axes.
//!
//! `PredictionHistory<C>`'s inner field is private, but the pinned 0.28 source implements public
//! `Deref<Target = HistoryBuffer<C>>`, and `HistoryBuffer::get` is public. Capture only reads that
//! history. It also reads the still-retained `FrameInterpolate` samples before Lightyear's
//! `EndRollback` job replaces them; it never promotes a presentation sample into rollback truth.
//!
//! `net::protocol` keeps `.add_linear_correction_fn()` rather than switching to
//! `enable_correction()`: lightyear's `EndRollback` system still owns rebuilding
//! `FrameInterpolate`. Capture runs after replay and before that set, consumes and unconditionally
//! removes `PreviousVisual`, so lightyear rebuilds interpolation but emits no duplicate
//! `VisualCorrection`.
//!
//! ## SIGN CONVENTIONS, and one asymmetry that is NOT ours to use
//!
//! `lightyear_replication-0.28.0/src/impls/avian3d.rs`: translation `diff(&self, new) = new - self`,
//! rotation `diff(&self, new) = new * self.inverse()` — a WORLD-LEFT delta. Translation stores that
//! difference directly. Rotation instead stores a BODY-LOCAL offset `O`, presented as `R_sim * O`.
//! If `C` is the corrected display, `P` is `PreviousVisual`, and `O_old` is the retained offset,
//! capture derives `O_new = C⁻¹ * P * O_old`. This reconstructs the previous ACTUAL display because
//! `C * O_new = P * O_old`.
//!
//! Avian 0.7 advances rotation by WORLD-LEFT multiplication: `R_sim' = F * R_sim`
//! (`dynamics/integrator/mod.rs`). The body-local form therefore transports without distorting
//! later travel: `(F * R_sim) * O = F * (R_sim * O)`.
//! `the_body_local_rotation_offset_is_derived_explicitly_not_applied_as_diffables_left_delta` pins
//! the frame conversion against lightyear's own `Diffable` impl.
//!
//! Ironically, the same upstream `Diffable` impl's `apply_diff` RIGHT-multiplies
//! (`self.0 *= delta.0`) even though `diff` returns the WORLD-LEFT delta `P * C⁻¹`. Right
//! multiplication is the correct side for TRANSPORT across Avian's later motion, but applying that
//! unconverted delta is still wrong: the required body-local value is `C⁻¹ * P`. Nothing here
//! routes through `apply_diff`; capture derives the local offset explicitly. The upstream asymmetry
//! remains inert because this module removes `PreviousVisual` before lightyear can create that
//! correction, and removes any stale `VisualCorrection` already present on a managed root. The
//! vendored crate is not modified.
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
//! # ONLY THE CLASSIFIED OFFSET CROSSES INTO PRESENTATION
//!
//! Classification and capture both happen inside the `PreUpdate` rollback transaction:
//! `net::adoption::confirm_forced_rollback` establishes delivery after `RollbackSystems::Prepare`
//! and writes the occurrence; [`capture_render_error`] runs after replay and before
//! `RollbackSystems::EndRollback`, DRAINS the queue whether or not anything matches, and either
//! accumulates the correction or refuses it. The resulting `RenderErrorOffset` deliberately survives
//! into `PostUpdate`, where it is decayed and composed onto the CURRENT interpolated pose. That
//! crossing is safe because the offset is already an entity-keyed, one-shot classification of the
//! correction at the previous display instant; adding translation and right-multiplying the
//! body-local rotation offset onto the current pose lets `RunFixedMainLoop` travel pass through
//! instead of subtracting it. The transient occurrence and `PreviousVisual` inputs do not cross the
//! transaction, so they cannot become latches.
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
//! The formula mirrors the top-level branch of `position_to_transform`. `arm_render_error` excludes
//! `ChildOf`, and both capture and apply use the same late-transition predicate. If an armed root
//! later gains a parent, the cosmetic layer removes its owned offset and transient correction
//! inputs, logs once, and leaves the hierarchy's raw pose alone. The production tank root is spawned
//! at the top level (`tank::spawn`); the fallback covers composition and future role changes without
//! turning a visual artefact into a client crash.

use avian3d::math::AsF32;
use avian3d::prelude::{PhysicsSystems, Position, Rotation};
use bevy::ecs::message::Messages;
use bevy::prelude::*;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationSystems};
use lightyear::interpolation::prelude::InterpolationRegistry;
use lightyear::prediction::correction::PreviousVisual;
use lightyear::prelude::{
    Diffable, LocalTimeline, Predicted, PredictionHistory, PredictionManager, Rollback,
    RollbackSystems, VisualCorrection,
};

use super::adoption::SharpCorrection;
use super::protocol::NetTank;

/// Presentation-only correction accumulated on the predicted root.
///
/// Translation is a world-space additive delta. Rotation is a body-local delta applied on the
/// right of the current simulated rotation.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq)]
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
    // INSIDE THE ROLLBACK TRANSACTION. `prepare_rollback` inserts `PreviousVisual`; replay has
    // completed by this seam, while lightyear's `EndRollback` consumer has not run yet.
    app.add_systems(
        PreUpdate,
        capture_render_error
            .after(RollbackSystems::Rollback)
            .before(RollbackSystems::EndRollback),
    );
    app.add_systems(
        PostUpdate,
        apply_render_error
            .in_set(RenderErrorApplied)
            .after(PhysicsSystems::Writeback)
            .after(FrameInterpolationSystems::Interpolate)
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
/// predicates from disagreeing: at depth zero, capture may need frame interpolation's retained
/// `tick - 1` sample to evaluate the corrected timeline at the previous display instant.
/// [`apply_render_error`] also runs explicitly after `FrameInterpolationSystems::Interpolate`.
fn arm_render_error(
    tanks: Query<
        Entity,
        (
            With<Predicted>,
            With<NetTank>,
            With<FrameInterpolate<Position>>,
            With<FrameInterpolate<Rotation>>,
            Without<RenderErrorOffset>,
            Without<ChildOf>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!("net: {entity} predicted root armed with render-space error offset");
        commands.entity(entity).insert(RenderErrorOffset::default());
    }
}

fn disable_if_parented(entity: Entity, parent: Option<&ChildOf>, commands: &mut Commands) -> bool {
    if parent.is_none() {
        return false;
    }
    warn!(
        "net: render-error root {entity} acquired ChildOf after arming; disabling its cosmetic \
         offset and accepting the hierarchy's raw pose"
    );
    commands.entity(entity).try_remove::<(
        RenderErrorOffset,
        PreviousVisual<Position>,
        PreviousVisual<Rotation>,
        VisualCorrection<Position>,
        VisualCorrection<Rotation>,
    )>();
    true
}

/// Reproduce Lightyear's display rule for one corrected component at capture's sample instant.
///
/// `visual_interpolation` interpolates only when `previous_value` exists; otherwise it skips and
/// leaves the raw component untouched. The predecessor's two possible owners are selected by the
/// caller, but once reduced to this optional input every capture path follows this one rule.
fn corrected_display<C: Component + Clone>(
    registry: &InterpolationRegistry,
    predecessor: Option<&C>,
    current: &C,
    overstep: f32,
) -> C {
    predecessor.map_or_else(
        || current.clone(),
        |previous| registry.interpolate(previous.clone(), current.clone(), overstep),
    )
}

/// Consume this rollback's same-instant correction: accumulate it, or refuse it as a delivered hit.
///
/// CONSUME is literal for both inputs. The occurrence queue is DRAINED unconditionally, so an
/// occurrence naming a root that no longer exists — or that this rollback did not correct — cannot
/// survive to sharpen an unrelated correction on a later frame. `PreviousVisual` is removed from
/// every managed root read at this seam, including depth zero where Lightyear otherwise leaks it;
/// stale `VisualCorrection` components are removed with it. The only correction state allowed to
/// survive the transaction is this module's already-classified `RenderErrorOffset`.
fn capture_render_error(
    time: Res<Time<Fixed>>,
    timeline: Res<LocalTimeline>,
    registry: Res<InterpolationRegistry>,
    mut occurrences: ResMut<Messages<SharpCorrection>>,
    managers: Query<&PredictionManager, With<Rollback>>,
    mut roots: Query<(
        Entity,
        &Position,
        &Rotation,
        &PredictionHistory<Position>,
        &PredictionHistory<Rotation>,
        &FrameInterpolate<Position>,
        &FrameInterpolate<Rotation>,
        Option<&PreviousVisual<Position>>,
        Option<&PreviousVisual<Rotation>>,
        Option<&VisualCorrection<Position>>,
        Option<&VisualCorrection<Rotation>>,
        &mut RenderErrorOffset,
        Option<&ChildOf>,
    )>,
    mut commands: Commands,
) {
    // FIRST, and whatever the query holds. A drain that depended on a matching root would leave the
    // occurrence queued exactly in the cases where no root matched it.
    let sharp: Vec<SharpCorrection> = occurrences.drain().collect();

    let tick = timeline.tick();
    let overstep = time.overstep_fraction();
    let depth_zero = managers
        .single()
        .is_ok_and(|manager| manager.get_rollback_start_tick() == Some(tick));
    for (
        entity,
        position,
        rotation,
        position_history,
        rotation_history,
        position_interpolate,
        rotation_interpolate,
        previous_position,
        previous_rotation,
        stale_translation_error,
        stale_rotation_error,
        mut offset,
        parent,
    ) in &mut roots
    {
        if disable_if_parented(entity, parent, &mut commands) {
            continue;
        }

        if previous_position.is_none()
            && previous_rotation.is_none()
            && stale_translation_error.is_none()
            && stale_rotation_error.is_none()
        {
            continue;
        }
        // Unconditional for every managed root read at this seam. `PreviousVisual` otherwise leaks
        // on depth zero, and either component surviving into a later frame would turn a one-shot
        // rollback fact into a latch.
        commands.entity(entity).try_remove::<(
            PreviousVisual<Position>,
            PreviousVisual<Rotation>,
            VisualCorrection<Position>,
            VisualCorrection<Rotation>,
        )>();
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

        // Measure both timelines at the PREVIOUS display instant. Lightyear deliberately reads this
        // overstep before RunFixedMainLoop for the same reason (`correction.rs:98`): this frame's
        // travel must not become part of a rollback correction.
        //
        // Replay normally rebuilds tick - 1 in PredictionHistory. A depth-zero restore cannot; in
        // that case FrameInterpolate may still hold the pre-rollback tick - 1 sample, where
        // predicted and corrected timelines agree. These are the two providers for ONE optional
        // predecessor. With it, Lightyear interpolates; without it, Lightyear skips interpolation
        // and the raw corrected endpoint is the display. Both outcomes feed the same diff rule.
        if let Some(previous_visual) = previous_position {
            let predecessor = position_history.get(tick - 1).or_else(|| {
                depth_zero
                    .then_some(position_interpolate.previous_value.as_ref())
                    .flatten()
            });
            let corrected_visual = corrected_display(&registry, predecessor, position, overstep);
            offset.translation += corrected_visual.diff(&previous_visual.0).0.f32();
        }
        if let Some(previous_visual) = previous_rotation {
            let predecessor = rotation_history.get(tick - 1).or_else(|| {
                depth_zero
                    .then_some(rotation_interpolate.previous_value.as_ref())
                    .flatten()
            });
            let corrected_visual = corrected_display(&registry, predecessor, rotation, overstep);
            let corrected = corrected_visual.0.f32();
            let previous = previous_visual.0.0.f32();
            // Preserve the previous ACTUAL display, including a retained old offset:
            //
            //     corrected * O_new = previous * O_old
            //     O_new = corrected^-1 * previous * O_old
            //
            // This is deliberately derived here instead of using `Diffable::apply_diff`: its
            // rotation diff is world-left even though its application is right-multiplied.
            offset.rotation = (corrected.inverse() * previous * offset.rotation).normalize();
        }
    }
}

/// Decay the classified offset and present it, re-derived from the sim pose.
fn apply_render_error(
    time: Res<Time<Real>>,
    mut roots: Query<(
        Entity,
        &mut Transform,
        &Position,
        &Rotation,
        &mut RenderErrorOffset,
        Option<&ChildOf>,
    )>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();

    for (entity, mut transform, position, rotation, mut offset, parent) in &mut roots {
        if disable_if_parented(entity, parent, &mut commands) {
            continue;
        }
        decay_translation(&mut offset.translation, dt);
        decay_rotation(&mut offset.rotation, dt);

        // Avian's `position_to_transform` root branch, verbatim, plus the offset. Written through
        // `set_if_neq` so a spent offset neither writes nor dirties the root — which would propagate
        // through the tank's DERIVED ~194 link children for no visual difference. The zero cases are
        // spelled out rather than folded into the arithmetic so the written value is BIT-IDENTICAL
        // to Avian's, which is what makes the comparison skip.
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
                (clean_rotation * offset.rotation).normalize()
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
    // Use the shortest-path quaternion representative. Angle-to-identity and the slerp fraction are
    // frame-independent, so the cap/snap/zero semantics are unchanged for a body-local offset.
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
    use bevy_replicon::prelude::RepliconTick;
    use lightyear::core::confirmed_history::ConfirmedHistory;
    use lightyear::prelude::client::{Client, ClientPlugins, Connected};
    use lightyear::prelude::{
        Diffable, InputTimeline, IsSynced, LocalTimeline, PeerId, Predicted, PredictionHistory,
        RemoteId, ReplicationCheckpointMap, RollbackSystems, StateRollbackMetadata, Tick,
    };

    use super::*;
    use crate::net::adoption::{AdoptionCause, ForcedRollbackSlot};
    use crate::net::test_harness::TICK;
    use crate::track::sim::TrackGripElements;

    type PositionHistoryBits = Vec<(u32, Option<[u32; 3]>)>;
    type RotationHistoryBits = Vec<(u32, Option<[u32; 4]>)>;

    #[derive(Resource, Default)]
    struct HistoriesBeforeCapture {
        roots: Vec<(Entity, PositionHistoryBits, RotationHistoryBits)>,
    }

    #[derive(Resource, Default)]
    struct HistoriesAfterCapture {
        roots: Vec<(Entity, PositionHistoryBits, RotationHistoryBits)>,
    }

    fn position_history_bits(history: &PredictionHistory<Position>) -> PositionHistoryBits {
        history
            .buffer()
            .iter()
            .map(|(tick, state)| {
                let value = match state {
                    lightyear_core::history_buffer::HistoryState::Updated(position) => {
                        Some(position.0.to_array().map(f32::to_bits))
                    }
                    lightyear_core::history_buffer::HistoryState::Removed => None,
                };
                (tick.0, value)
            })
            .collect()
    }

    fn rotation_history_bits(history: &PredictionHistory<Rotation>) -> RotationHistoryBits {
        history
            .buffer()
            .iter()
            .map(|(tick, state)| {
                let value = match state {
                    lightyear_core::history_buffer::HistoryState::Updated(rotation) => {
                        Some(rotation.0.to_array().map(f32::to_bits))
                    }
                    lightyear_core::history_buffer::HistoryState::Removed => None,
                };
                (tick.0, value)
            })
            .collect()
    }

    fn snapshot_histories_before_capture(
        roots: Query<(
            Entity,
            &PredictionHistory<Position>,
            &PredictionHistory<Rotation>,
        )>,
        mut snapshots: ResMut<HistoriesBeforeCapture>,
    ) {
        snapshots.roots = roots
            .iter()
            .map(|(entity, position, rotation)| {
                (
                    entity,
                    position_history_bits(position),
                    rotation_history_bits(rotation),
                )
            })
            .collect();
        snapshots.roots.sort_by_key(|(entity, ..)| entity.to_bits());
    }

    fn snapshot_histories_after_capture(
        roots: Query<(
            Entity,
            &PredictionHistory<Position>,
            &PredictionHistory<Rotation>,
        )>,
        mut snapshots: ResMut<HistoriesAfterCapture>,
    ) {
        snapshots.roots = roots
            .iter()
            .map(|(entity, position, rotation)| {
                (
                    entity,
                    position_history_bits(position),
                    rotation_history_bits(rotation),
                )
            })
            .collect();
        snapshots.roots.sort_by_key(|(entity, ..)| entity.to_bits());
    }

    /// The tick a fixture rollback restores from, and the tick the client is on when it lands.
    ///
    /// ONE TICK OF DEPTH, deliberately. It is the shallowest rollback whose replay recreates
    /// `PredictionHistory::get(tick - 1)`; the age-zero fixtures sit one tick below and therefore
    /// exercise Lightyear's raw-display rule when that predecessor is absent.
    const ROLLBACK_TICK: Tick = Tick(100);
    const CURRENT_TICK: Tick = Tick(101);

    /// The pose the client had on screen when the rollback landed.
    const PREDICTED_POSITION: Vec3 = Vec3::new(10.0, 2.0, -3.0);
    /// The pose the authority's confirmed sample restores it to. DERIVED distance 0.206 m, inside
    /// the NEAR decay bracket and well under the snap threshold.
    const AUTHORITY_POSITION: Vec3 = Vec3::new(10.2, 2.0, -3.05);
    /// A second authority pose, for the fixture that lands two rollbacks in a row.
    const LATER_AUTHORITY_POSITION: Vec3 = Vec3::new(10.35, 2.0, -3.02);

    /// Rotations about DIFFERENT axes, so world-left and body-local correction frames give visibly
    /// different results.
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

    fn expected_rotation_offset() -> Quat {
        authority_rotation().inverse() * predicted_rotation()
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

    /// Ordinary motion applied by a `FixedUpdate` system. Most fixtures drive only replay schedules;
    /// the full-frame positive control switches this resource after capture so its replay tick and
    /// forward tick can carry distinct travel.
    #[derive(Resource, Default)]
    struct ReplayTravel(Vec3);

    #[derive(Resource)]
    struct ReplayRotation(Quat);

    impl Default for ReplayRotation {
        fn default() -> Self {
            Self(Quat::IDENTITY)
        }
    }

    fn travel_during_replay(
        travel: Res<ReplayTravel>,
        turn: Res<ReplayRotation>,
        mut roots: Query<(&mut Position, &mut Rotation)>,
    ) {
        if travel.0 == Vec3::ZERO && turn.0 == Quat::IDENTITY {
            return;
        }
        for (mut position, mut rotation) in &mut roots {
            position.0 += travel.0;
            rotation.0 = (turn.0 * rotation.0).normalize();
        }
    }

    /// Test-only samples installed after Lightyear has prepared/replayed a rollback and before
    /// capture reads the corrected histories. This is deliberately a seam fixture, not production
    /// history ownership: it lets the mixed-predecessor tests vary the exact buffers capture sees.
    #[derive(Resource, Default)]
    struct PendingCapturePredecessors(Option<(Option<Position>, Option<Rotation>)>);

    fn seed_capture_predecessors(
        timeline: Res<LocalTimeline>,
        mut pending: ResMut<PendingCapturePredecessors>,
        mut roots: Query<(
            &Position,
            &Rotation,
            &mut PredictionHistory<Position>,
            &mut PredictionHistory<Rotation>,
        )>,
    ) {
        let Some((position_predecessor, rotation_predecessor)) = pending.0.take() else {
            return;
        };
        let tick = timeline.tick();
        for (position, rotation, mut position_history, mut rotation_history) in &mut roots {
            position_history.clear();
            if let Some(previous) = position_predecessor {
                position_history.add_predicted(tick - 1, Some(previous));
            }
            position_history.add_predicted(tick, Some(*position));

            rotation_history.clear();
            if let Some(previous) = rotation_predecessor {
                rotation_history.add_predicted(tick - 1, Some(previous));
            }
            rotation_history.add_predicted(tick, Some(*rotation));
        }
    }

    #[derive(Clone, Copy)]
    struct CapturedRoot {
        entity: Entity,
        translation: Vec3,
        rotation: Quat,
    }

    #[derive(Resource, Default)]
    struct CaptureObservation {
        overstep: f32,
        roots: Vec<CapturedRoot>,
    }

    #[derive(Resource, Default)]
    struct PreviousVisualObservation {
        roots: Vec<(Entity, Option<Position>, Option<Rotation>)>,
    }

    fn observe_previous_visuals(
        roots: Query<(
            Entity,
            Option<&PreviousVisual<Position>>,
            Option<&PreviousVisual<Rotation>>,
        )>,
        mut observation: ResMut<PreviousVisualObservation>,
    ) {
        observation.roots = roots
            .iter()
            .map(|(entity, position, rotation)| {
                (
                    entity,
                    position.map(|value| value.0),
                    rotation.map(|value| value.0),
                )
            })
            .collect();
    }

    fn observe_capture(
        time: Res<Time<Fixed>>,
        roots: Query<(Entity, &RenderErrorOffset)>,
        mut observation: ResMut<CaptureObservation>,
    ) {
        observation.overstep = time.overstep_fraction();
        observation.roots = roots
            .iter()
            .map(|(entity, offset)| CapturedRoot {
                entity,
                translation: offset.translation,
                rotation: offset.rotation,
            })
            .collect();
    }

    #[derive(Resource, Default)]
    struct PendingForwardTravel(Option<Vec3>);

    #[derive(Resource, Default)]
    struct PendingForwardRotation(Option<Quat>);

    fn switch_to_forward_motion(
        mut pending_travel: ResMut<PendingForwardTravel>,
        mut pending_rotation: ResMut<PendingForwardRotation>,
        mut travel: ResMut<ReplayTravel>,
        mut rotation: ResMut<ReplayRotation>,
    ) {
        if let Some(next) = pending_travel.0.take() {
            travel.0 = next;
        }
        if let Some(next) = pending_rotation.0.take() {
            rotation.0 = next;
        }
    }

    #[derive(Resource, Default)]
    struct DisplayObservation {
        overstep: f32,
        roots: Vec<(Entity, Vec3, Quat)>,
    }

    fn observe_display(
        time: Res<Time<Fixed>>,
        roots: Query<(Entity, &Position, &Rotation), With<RenderErrorOffset>>,
        mut observation: ResMut<DisplayObservation>,
    ) {
        observation.overstep = time.overstep_fraction();
        observation.roots = roots
            .iter()
            .map(|(entity, position, rotation)| (entity, position.0, rotation.0))
            .collect();
    }

    /// A real lightyear client on the PRODUCTION registration, with this module's plugin mounted.
    ///
    /// The registration matters: `prepare_rollback` stores `PreviousVisual<C>` only when
    /// `PredictionRegistry::has_correction::<C>()`, and `update_frame_interpolation_post_rollback`
    /// runs only for components registered through `add_correction_fn`. Both come from
    /// `net::protocol`'s `.add_linear_correction_fn()`, so a fixture that registered its own
    /// components would be testing a seam the game does not have.
    fn client_app_at(target: Tick) -> App {
        // THE CLIENT'S PHYSICS COMPOSITION, not Avian's default. Avian's `PhysicsTransformPlugin`
        // syncs `Transform` back into `Position` inside `FixedPostUpdate`, which `run_rollback`
        // executes on every replayed tick — it would undo the restore this whole fixture is about.
        let mut app = crate::net::test_harness::net_physics_app();
        app.add_plugins(ClientPlugins {
            tick_duration: crate::net::test_harness::TICK,
        });
        crate::state::sim_plugin(&mut app);
        crate::net::protocol::plugin(&mut app);
        crate::net::grip::install_client(&mut app);
        crate::net::rig::client_smoothing_plugin(&mut app);
        app.insert_state(crate::state::AppState::Playing);
        app.add_plugins(plugin);
        app.init_resource::<PendingRollback>();
        app.init_resource::<PendingSharp>();
        app.init_resource::<ReplayTravel>();
        app.init_resource::<ReplayRotation>();
        app.init_resource::<PendingCapturePredecessors>();
        app.init_resource::<CaptureObservation>();
        app.init_resource::<PreviousVisualObservation>();
        app.init_resource::<PendingForwardTravel>();
        app.init_resource::<PendingForwardRotation>();
        app.init_resource::<DisplayObservation>();
        app.init_resource::<HistoriesBeforeCapture>();
        app.init_resource::<HistoriesAfterCapture>();
        app.init_resource::<bevy::transform::StaticTransformOptimizations>();
        app.add_systems(PreUpdate, order_the_rollback.before(RollbackSystems::Check));
        // WHERE `confirm_forced_rollback` SITS: after the restore is established, before the replay.
        app.add_systems(
            PreUpdate,
            emit_sharp_occurrences
                .after(RollbackSystems::Prepare)
                .before(RollbackSystems::Rollback),
        );
        app.add_systems(
            PreUpdate,
            seed_capture_predecessors
                .after(RollbackSystems::Rollback)
                .before(snapshot_histories_before_capture),
        );
        app.add_systems(
            PreUpdate,
            snapshot_histories_before_capture
                .after(RollbackSystems::Rollback)
                .before(capture_render_error),
        );
        app.add_systems(
            PreUpdate,
            observe_previous_visuals
                .after(snapshot_histories_before_capture)
                .before(capture_render_error),
        );
        app.add_systems(
            PreUpdate,
            (
                (observe_capture, snapshot_histories_after_capture).after(capture_render_error),
                switch_to_forward_motion.after(observe_capture),
            )
                .before(RollbackSystems::EndRollback),
        );
        app.add_systems(
            PostUpdate,
            observe_display
                .after(FrameInterpolationSystems::Interpolate)
                .before(apply_render_error),
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
        advance_to(&mut app, target);
        let checkpoint = RepliconTick::new(1);
        let mut checkpoints = app.world_mut().resource_mut::<ReplicationCheckpointMap>();
        checkpoints.record(checkpoint, target);
        checkpoints.record_last_confirmed_tick(checkpoint);
        app
    }

    fn client_app() -> App {
        client_app_at(CURRENT_TICK)
    }

    /// A predicted root as `net::rig` leaves one: rollback-eligible, frame-interpolated, and armed.
    fn spawn_armed_root(app: &mut App) -> Entity {
        let mut predicted_position = PredictionHistory::<Position>::default();
        predicted_position.add_predicted(ROLLBACK_TICK, Some(Position(PREDICTED_POSITION)));
        let mut predicted_rotation_history = PredictionHistory::<Rotation>::default();
        predicted_rotation_history
            .add_predicted(ROLLBACK_TICK, Some(Rotation(predicted_rotation())));
        let grip = TrackGripElements::for_links(1);
        let mut predicted_grip = PredictionHistory::<TrackGripElements>::default();
        predicted_grip.add_predicted(ROLLBACK_TICK, Some(grip.clone()));
        predicted_grip.add_predicted(CURRENT_TICK, Some(grip.clone()));
        let root = app
            .world_mut()
            .spawn((
                Predicted,
                Position(PREDICTED_POSITION),
                Rotation(predicted_rotation()),
                predicted_position,
                predicted_rotation_history,
                grip,
                predicted_grip,
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

    fn spawn_root_through_shipping_arming(app: &mut App) -> Entity {
        let root = spawn_armed_root(app);
        app.world_mut()
            .entity_mut(root)
            .remove::<(
                FrameInterpolate<Position>,
                FrameInterpolate<Rotation>,
                RenderErrorOffset,
            )>()
            .insert((NetTank, crate::tank::Tank, GlobalTransform::default()));
        app.world_mut().flush();

        // `net::rig` arms interpolation first; render-error's stricter predicate observes those
        // components on the following Update. This is the same two-system composition as shipping.
        app.world_mut().run_schedule(Update);
        app.world_mut().run_schedule(Update);
        assert!(
            app.world().get::<RenderErrorOffset>(root).is_some(),
            "the shipping arming path must install the offset",
        );
        root
    }

    fn stage_age_zero_grip_checkpoint(app: &mut App, root: Entity) {
        let mut corrected = TrackGripElements::for_links(1);
        corrected.sides[0].strain[0] = Vec3::new(0.001, 0.0, 0.0);
        crate::net::grip::stage_checkpoint_for_test(
            app.world_mut(),
            root,
            crate::CombatantId(7),
            1,
            CURRENT_TICK + 1,
            corrected,
        );
    }

    fn set_half_overstep_visual_pose(app: &mut App, root: Entity) -> (Vec3, Quat, Vec3, Quat) {
        let common_previous_position = PREDICTED_POSITION - Vec3::new(0.1, 0.0, 0.0);
        let common_previous_rotation = Quat::IDENTITY;
        let previous_displayed_position = common_previous_position.lerp(PREDICTED_POSITION, 0.5);
        let previous_displayed_rotation = common_previous_rotation.slerp(predicted_rotation(), 0.5);
        {
            let mut interpolate = app
                .world_mut()
                .get_mut::<FrameInterpolate<Position>>(root)
                .unwrap();
            interpolate.previous_value = Some(Position(common_previous_position));
            interpolate.current_value = Some(Position(PREDICTED_POSITION));
        }
        {
            let mut interpolate = app
                .world_mut()
                .get_mut::<FrameInterpolate<Rotation>>(root)
                .unwrap();
            interpolate.previous_value = Some(Rotation(common_previous_rotation));
            interpolate.current_value = Some(Rotation(predicted_rotation()));
        }
        app.world_mut().get_mut::<Position>(root).unwrap().0 = previous_displayed_position;
        app.world_mut().get_mut::<Rotation>(root).unwrap().0 = previous_displayed_rotation;
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(TICK / 2);
        (
            common_previous_position,
            common_previous_rotation,
            previous_displayed_position,
            previous_displayed_rotation,
        )
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
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(TICK / 2);
        app.world_mut().get_mut::<Position>(root).unwrap().0 = PREDICTED_POSITION + TRAVEL * 0.5;
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
            expected_rotation_offset(),
            "the rotation offset must be the body-local delta that carries the corrected pose back \
             to the previously displayed one",
        );
    }

    /// POSITIVE CONTROL FOR THE SAMPLE INSTANT. The previous display lies halfway through a moving
    /// tick. The rollback replays that same travel around a shifted authority baseline, then the
    /// shipping `RunFixedMainLoop` advances a different, orthogonal travel before PostUpdate.
    ///
    /// The correction is the baseline shift alone. Sampling the corrected raw endpoint also adds
    /// half of `PREVIOUS_TRAVEL`; sampling this frame's PostUpdate display adds that plus its actual
    /// overstep fraction of `THIS_FRAME_TRAVEL`. Distinct axes make either mistake visible.
    #[test]
    fn a_moving_rollback_captures_only_the_correction_at_the_previous_display_instant() {
        const PREVIOUS_TRAVEL: Vec3 = Vec3::new(0.4, 0.0, 0.0);
        const THIS_FRAME_TRAVEL: Vec3 = Vec3::new(0.0, 0.0, 0.3);
        const CORRECTION: Vec3 = Vec3::new(0.0, 0.2, 0.0);
        let baseline = PREDICTED_POSITION - PREVIOUS_TRAVEL;

        let mut app = client_app();
        prime_full_frame_clock(&mut app);
        app.world_mut().resource_mut::<ReplayTravel>().0 = PREVIOUS_TRAVEL;
        app.world_mut().resource_mut::<PendingForwardTravel>().0 = Some(THIS_FRAME_TRAVEL);
        let root = spawn_armed_root(&mut app);
        {
            let mut history = app
                .world_mut()
                .get_mut::<PredictionHistory<Position>>(root)
                .expect("position history");
            history.clear();
            history.add_predicted(ROLLBACK_TICK, Some(Position(baseline)));
        }
        let previous_display = stage_previous_display(
            &mut app,
            root,
            Some(Position(baseline)),
            Some(Rotation(predicted_rotation())),
        )
        .0;
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            baseline + CORRECTION,
            predicted_rotation(),
        );
        order_rollback(&mut app, ROLLBACK_TICK);

        app.update();

        let captured = captured_of(&app, root);
        assert!((app.world().resource::<CaptureObservation>().overstep - 0.5).abs() < 1e-6);
        assert_near(
            captured.translation,
            -CORRECTION,
            "both timelines travel halfway through the old tick, so their same-instant difference \
             is only the authority baseline shift",
        );
        let corrected_raw_endpoint = PREDICTED_POSITION + CORRECTION;
        assert_ne!(
            captured.translation,
            previous_display - corrected_raw_endpoint,
            "diffing against the corrected raw endpoint would wrongly include previous-frame travel",
        );

        let display = app.world().resource::<DisplayObservation>();
        let (_, displayed_position, _) = display
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .copied()
            .expect("the display observer saw the root");
        assert_near(
            displayed_position,
            corrected_raw_endpoint + THIS_FRAME_TRAVEL * display.overstep,
            "the full shipping schedule must visibly advance this frame's orthogonal travel",
        );
        let interpolation = app
            .world()
            .get::<FrameInterpolate<Position>>(root)
            .expect("position interpolation remains armed");
        assert_eq!(
            interpolation.previous_value,
            Some(Position(corrected_raw_endpoint)),
            "RunFixedMainLoop must shift the corrected endpoint into the previous sample",
        );
        assert_eq!(
            interpolation.current_value,
            Some(Position(corrected_raw_endpoint + THIS_FRAME_TRAVEL)),
            "RunFixedMainLoop must record this frame's distinct travel as the new endpoint",
        );
        assert_ne!(
            captured.translation,
            previous_display - displayed_position,
            "diffing against this frame's display would wrongly include both frames' travel",
        );
    }

    /// ONE-SHOT. Capture removes `PreviousVisual` before lightyear can turn it into
    /// `VisualCorrection`, and clears either stale correction component as well. A later frame with
    /// no rollback therefore has no input that could add the correction again.
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
            "lightyear must not retain a duplicate correction for the next frame",
        );
        assert!(
            app.world().get::<PreviousVisual<Position>>(root).is_none()
                && app.world().get::<PreviousVisual<Rotation>>(root).is_none(),
            "the one-shot PreviousVisual inputs must be consumed in the rollback transaction",
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
    /// re-counted: the second frame adds its own same-instant error to it and nothing else.
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
        assert!(
            app.world().get::<PreviousVisual<Position>>(root).is_none(),
            "the sharp verdict must consume its rollback input too",
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

    /// TWO MESSAGE BUFFERS, ONE READER PASS. The first occurrence is rotated into the older Bevy
    /// message buffer before the second is written; capture does not run between them. This is the
    /// queued-occurrence case distinct from two writes in one frame.
    #[test]
    fn occurrences_spanning_adjacent_message_buffers_are_both_drained_by_one_capture() {
        let mut app = client_app();
        let first = spawn_armed_root(&mut app);
        let second = spawn_armed_root(&mut app);
        for root in [first, second] {
            confirm_pose(
                &mut app,
                root,
                ROLLBACK_TICK,
                AUTHORITY_POSITION,
                authority_rotation(),
            );
        }

        app.world_mut()
            .resource_mut::<Messages<SharpCorrection>>()
            .write(SharpCorrection {
                entity: first,
                restored_from: ROLLBACK_TICK,
            });
        app.world_mut()
            .resource_mut::<Messages<SharpCorrection>>()
            .update();
        app.world_mut()
            .resource_mut::<Messages<SharpCorrection>>()
            .write(SharpCorrection {
                entity: second,
                restored_from: ROLLBACK_TICK,
            });

        order_rollback(&mut app, ROLLBACK_TICK);
        run_pre_update(&mut app);

        assert_eq!(offset_of(&app, first).0, Vec3::ZERO);
        assert_eq!(offset_of(&app, second).0, Vec3::ZERO);
        assert!(
            app.world()
                .resource::<Messages<SharpCorrection>>()
                .is_empty(),
            "one capture must drain both the older and newer message buffers",
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

    fn captured_of(app: &App, root: Entity) -> CapturedRoot {
        app.world()
            .resource::<CaptureObservation>()
            .roots
            .iter()
            .find(|capture| capture.entity == root)
            .copied()
            .expect("the capture observer saw the armed root")
    }

    fn previous_visuals_of(app: &App, root: Entity) -> (Option<Position>, Option<Rotation>) {
        app.world()
            .resource::<PreviousVisualObservation>()
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .map(|(_, position, rotation)| (*position, *rotation))
            .expect("the pre-capture observer saw the armed root")
    }

    /// Bevy's first `update()` establishes the manual clock origin and has zero virtual delta. Prime
    /// that frame before a test whose observed update must expend a fixed tick.
    fn prime_full_frame_clock(app: &mut App) {
        let tick = app.world().resource::<LocalTimeline>().tick();
        app.update();
        assert_eq!(
            app.world().resource::<LocalTimeline>().tick(),
            tick,
            "the clock-origin frame must not advance the fixed timeline",
        );
    }

    fn stage_previous_display(
        app: &mut App,
        root: Entity,
        position_predecessor: Option<Position>,
        rotation_predecessor: Option<Rotation>,
    ) -> (Vec3, Quat) {
        let displayed_position = position_predecessor.map_or(PREDICTED_POSITION, |previous| {
            previous.0.lerp(PREDICTED_POSITION, 0.5)
        });
        let displayed_rotation = rotation_predecessor.map_or_else(predicted_rotation, |previous| {
            previous.0.slerp(predicted_rotation(), 0.5)
        });
        {
            let mut interpolate = app
                .world_mut()
                .get_mut::<FrameInterpolate<Position>>(root)
                .expect("position interpolation is armed");
            interpolate.previous_value = position_predecessor;
            interpolate.current_value = Some(Position(PREDICTED_POSITION));
        }
        {
            let mut interpolate = app
                .world_mut()
                .get_mut::<FrameInterpolate<Rotation>>(root)
                .expect("rotation interpolation is armed");
            interpolate.previous_value = rotation_predecessor;
            interpolate.current_value = Some(Rotation(predicted_rotation()));
        }
        app.world_mut().get_mut::<Position>(root).unwrap().0 = displayed_position;
        app.world_mut().get_mut::<Rotation>(root).unwrap().0 = displayed_rotation;
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(TICK / 2);
        (displayed_position, displayed_rotation)
    }

    fn run_age_zero_rollback(
        history_predecessors: (Option<Position>, Option<Rotation>),
        display_predecessors: (Option<Position>, Option<Rotation>),
    ) -> (App, Entity, Vec3, Quat) {
        let mut app = client_app();
        prime_full_frame_clock(&mut app);
        let root = spawn_armed_root(&mut app);
        let (previous_displayed_position, previous_displayed_rotation) = stage_previous_display(
            &mut app,
            root,
            display_predecessors.0,
            display_predecessors.1,
        );

        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );
        stage_age_zero_grip_checkpoint(&mut app, root);
        app.world_mut()
            .resource_mut::<PendingCapturePredecessors>()
            .0 = Some(history_predecessors);

        app.update();

        (
            app,
            root,
            previous_displayed_position,
            previous_displayed_rotation,
        )
    }

    /// NO PREDECESSOR means Lightyear displays the raw endpoint; it does not invent an interpolated
    /// timeline. This drives the production registration, shipping arming composition, the prior
    /// PostUpdate display, the depth-zero rollback, the following fixed tick, and presentation
    /// application. Distinct translation and rotation travel after capture prove the offset hides
    /// only the rollback discontinuity.
    #[test]
    fn an_age_zero_rollback_without_any_t_minus_one_sample_smooths_the_full_raw_display_correction()
    {
        const FORWARD_TRAVEL: Vec3 = Vec3::new(0.0, 0.0, 0.3);
        let forward_turn = Quat::from_rotation_z(0.08);

        let mut app = client_app();
        prime_full_frame_clock(&mut app);
        let root = spawn_root_through_shipping_arming(&mut app);
        let (previous_displayed_position, previous_displayed_rotation) =
            stage_previous_display(&mut app, root, None, None);

        // Positive control for the premise: without `previous_value`, Lightyear's real PostUpdate
        // interpolation system skips and both the component and final Transform stay at raw P.
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(previous_displayed_position, PREDICTED_POSITION);
        assert_eq!(previous_displayed_rotation, predicted_rotation());
        let prior_display = app.world().resource::<DisplayObservation>();
        let (_, prior_position, prior_rotation) = prior_display
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .copied()
            .expect("the prior PostUpdate display observer saw the armed root");
        assert_near(
            prior_position,
            PREDICTED_POSITION,
            "Lightyear must leave the raw predicted position displayed when no predecessor exists",
        );
        assert_near_rotation(
            prior_rotation,
            predicted_rotation(),
            "Lightyear must leave the raw predicted rotation displayed when no predecessor exists",
        );
        let prior_transform = app.world().get::<Transform>(root).expect("rendered root");
        assert_near(
            prior_transform.translation,
            PREDICTED_POSITION,
            "the prior frame must actually render raw P, not merely stage it in interpolation state",
        );
        assert_near_rotation(
            prior_transform.rotation,
            predicted_rotation(),
            "the prior frame must actually render raw Pq",
        );

        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );
        stage_age_zero_grip_checkpoint(&mut app, root);
        app.world_mut()
            .resource_mut::<PendingCapturePredecessors>()
            .0 = Some((None, None));
        app.world_mut().resource_mut::<PendingForwardTravel>().0 = Some(FORWARD_TRAVEL);
        app.world_mut().resource_mut::<PendingForwardRotation>().0 = Some(forward_turn);

        app.update();

        assert_near(
            app.world().get::<Position>(root).expect("live position").0,
            AUTHORITY_POSITION + FORWARD_TRAVEL * 0.5,
            "PostUpdate must display the independent forward travel at the half-tick overstep",
        );
        assert_near_rotation(
            app.world().get::<Rotation>(root).expect("live rotation").0,
            Quat::IDENTITY.slerp(forward_turn, 0.5) * authority_rotation(),
            "PostUpdate must display the independent forward rotation at the half-tick overstep",
        );
        assert_eq!(
            app.world()
                .get::<FrameInterpolate<Position>>(root)
                .expect("position interpolation")
                .current_value,
            Some(Position(AUTHORITY_POSITION + FORWARD_TRAVEL)),
            "the fixed-tick endpoint must retain the full independent forward travel",
        );
        assert_eq!(
            app.world()
                .get::<FrameInterpolate<Rotation>>(root)
                .expect("rotation interpolation")
                .current_value,
            Some(Rotation(forward_turn * authority_rotation())),
            "the fixed-tick endpoint must retain the full independent forward rotation",
        );
        assert_eq!(
            previous_visuals_of(&app, root),
            (
                Some(Position(PREDICTED_POSITION)),
                Some(Rotation(predicted_rotation()))
            ),
            "prepare_rollback must capture the raw P/Pq that the prior PostUpdate displayed",
        );
        let captured = captured_of(&app, root);
        assert!((app.world().resource::<CaptureObservation>().overstep - 0.5).abs() < 1e-6);
        assert_near(
            captured.translation,
            PREDICTED_POSITION - AUTHORITY_POSITION,
            "the skipped-interpolation path must capture the full raw endpoint correction P - C",
        );
        assert_near_rotation(
            captured.rotation,
            expected_rotation_offset(),
            "the skipped-interpolation path must capture the exact raw body-local offset Cq^-1 * Pq",
        );
        assert!(
            app.world().get::<PreviousVisual<Position>>(root).is_none()
                && app.world().get::<PreviousVisual<Rotation>>(root).is_none(),
            "both one-shot rollback inputs must be consumed after their full corrections are used",
        );

        let display = app.world().resource::<DisplayObservation>();
        assert!((display.overstep - 0.5).abs() < 1e-6);
        let (_, corrected_display_position, corrected_display_rotation) = display
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .copied()
            .expect("the current PostUpdate display observer saw the armed root");
        let displayed_travel = FORWARD_TRAVEL * display.overstep;
        let displayed_turn = Quat::IDENTITY.slerp(forward_turn, display.overstep);
        assert_near(
            corrected_display_position,
            AUTHORITY_POSITION + displayed_travel,
            "Lightyear's current display must contain the independent forward travel",
        );
        assert_near_rotation(
            corrected_display_rotation,
            displayed_turn * authority_rotation(),
            "Lightyear's current display must contain the independent forward rotation",
        );

        let live_offset = offset_of(&app, root);
        let mut expected_decayed_rotation = captured.rotation;
        decay_rotation(
            &mut expected_decayed_rotation,
            app.world().resource::<Time<Real>>().delta_secs(),
        );
        assert_near_rotation(
            live_offset.1,
            expected_decayed_rotation,
            "the live rotation offset must be exactly the captured body-local offset after normal \
             one-frame decay",
        );
        let rendered = app.world().get::<Transform>(root).expect("rendered root");
        assert_near(
            rendered.translation,
            corrected_display_position + live_offset.0,
            "presentation must apply only the normally decayed captured offset to the current pose",
        );
        assert_near_rotation(
            rendered.rotation,
            corrected_display_rotation * live_offset.1,
            "presentation must right-apply only the normally decayed body-local rotation offset",
        );
        let undiscounted_target = PREDICTED_POSITION + displayed_travel;
        assert_near(
            rendered.translation,
            undiscounted_target + (live_offset.0 - captured.translation),
            "the rendered pose must be P plus current-frame travel, subject only to normal decay",
        );
        assert!(
            rendered.translation.distance(undiscounted_target)
                <= CAP_TRANSLATION_MPS * app.world().resource::<Time<Real>>().delta_secs() + 1e-5,
            "normal decay may reveal only its capped one-frame share of the correction",
        );
        let avian_current_rotation = displayed_turn * authority_rotation();
        assert_near_rotation(
            corrected_display_rotation,
            avian_current_rotation,
            "positive control: the fixture must model Avian's current = F * C world-left \
             composition",
        );
        assert!(
            (authority_rotation() * displayed_turn).angle_between(avian_current_rotation) > 1e-3,
            "the noncommuting fixture must distinguish Avian's world-left composition from the old \
             body-right helper convention",
        );
        let undiscounted_rotation = displayed_turn * predicted_rotation();
        assert_near_rotation(
            corrected_display_rotation * captured.rotation,
            undiscounted_rotation,
            "the full captured body-local offset must hide the rollback while preserving Avian's \
             world-left current-frame rotation",
        );
        assert!(
            rendered.rotation.angle_between(undiscounted_rotation)
                <= CAP_ROTATION_DPS.to_radians()
                    * app.world().resource::<Time<Real>>().delta_secs()
                    + 1e-5,
            "normal decay may reveal only its capped one-frame share of the rotation correction",
        );
    }

    /// `EndRollback` reconstructs frame interpolation even when no fixed tick ran. At depth zero
    /// its history lookup can produce `previous_value = None`, so the raw-display window can reopen
    /// after any such rollback; it is not a one-time arming state.
    #[test]
    fn a_depth_zero_rollback_reopens_the_raw_display_window_and_a_second_correction_is_smoothed() {
        let mut app = client_app();
        let root = spawn_root_through_shipping_arming(&mut app);
        let first_predecessor_position = Position(PREDICTED_POSITION - Vec3::new(0.2, 0.0, 0.0));
        let first_predecessor_rotation = Rotation(Quat::from_rotation_z(-0.08));
        let (first_displayed_position, first_displayed_rotation) = stage_previous_display(
            &mut app,
            root,
            Some(first_predecessor_position),
            Some(first_predecessor_rotation),
        );

        assert_eq!(
            (
                app.world()
                    .get::<FrameInterpolate<Position>>(root)
                    .expect("position interpolation")
                    .previous_value,
                app.world()
                    .get::<FrameInterpolate<Rotation>>(root)
                    .expect("rotation interpolation")
                    .previous_value,
            ),
            (
                Some(first_predecessor_position),
                Some(first_predecessor_rotation)
            ),
            "the raw-display window must begin closed by real predecessor samples",
        );
        app.world_mut().run_schedule(PostUpdate);
        let initial_display = app.world().resource::<DisplayObservation>();
        let (_, initial_position, initial_rotation) = initial_display
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .copied()
            .expect("the initial PostUpdate observer saw the root");
        assert_near(
            initial_position,
            first_displayed_position,
            "the first real PostUpdate must display the interpolated position",
        );
        assert_near_rotation(
            initial_rotation,
            first_displayed_rotation,
            "the first real PostUpdate must display the interpolated rotation",
        );

        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );
        order_rollback(&mut app, CURRENT_TICK);
        run_pre_update(&mut app);

        let first = captured_of(&app, root);
        let first_corrected_display = first_predecessor_position.0.lerp(AUTHORITY_POSITION, 0.5);
        let first_corrected_rotation = first_predecessor_rotation
            .0
            .slerp(authority_rotation(), 0.5);
        assert_near(
            first.translation,
            first_displayed_position - first_corrected_display,
            "the first correction must compare the two interpolated displays at the same instant",
        );
        assert_near_rotation(
            first.rotation,
            first_corrected_rotation.inverse() * first_displayed_rotation,
            "the first correction must be captured in the corrected display's body frame",
        );
        let position_interpolate = app
            .world()
            .get::<FrameInterpolate<Position>>(root)
            .expect("position interpolation remains armed");
        let rotation_interpolate = app
            .world()
            .get::<FrameInterpolate<Rotation>>(root)
            .expect("rotation interpolation remains armed");
        assert_eq!(
            (
                position_interpolate.previous_value,
                rotation_interpolate.previous_value
            ),
            (None, None),
            "Lightyear EndRollback must transition Some to None and reopen the raw-display window",
        );

        // With the reopened window, Lightyear's real interpolation system skips. Its display
        // observer therefore sees the raw corrected endpoint; the render offset is applied only
        // afterwards to Transform.
        app.world_mut().run_schedule(PostUpdate);
        let reopened_display = app.world().resource::<DisplayObservation>();
        let (_, reopened_position, reopened_rotation) = reopened_display
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .copied()
            .expect("the reopened-window PostUpdate observer saw the root");
        assert_near(
            reopened_position,
            AUTHORITY_POSITION,
            "without a predecessor, the intervening PostUpdate must display the raw position \
             endpoint",
        );
        assert_near_rotation(
            reopened_rotation,
            authority_rotation(),
            "without a predecessor, the intervening PostUpdate must display the raw rotation \
             endpoint",
        );
        let retained = offset_of(&app, root);

        let second_corrected_position = Vec3::new(10.25, 2.05, -2.95);
        let second_corrected_rotation = Quat::from_rotation_x(0.09);
        advance_to(&mut app, CURRENT_TICK + 1);
        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK + 1,
            second_corrected_position,
            second_corrected_rotation,
        );
        order_rollback(&mut app, CURRENT_TICK + 1);
        run_pre_update(&mut app);

        assert_eq!(
            previous_visuals_of(&app, root),
            (
                Some(Position(AUTHORITY_POSITION)),
                Some(Rotation(authority_rotation()))
            ),
            "the second rollback must capture the raw endpoint established by the intervening real \
             PostUpdate",
        );
        let accumulated = captured_of(&app, root);
        assert_near(
            accumulated.translation,
            retained.0 + AUTHORITY_POSITION - second_corrected_position,
            "the second raw endpoint correction must accumulate onto the retained translation \
             offset",
        );
        assert_near_rotation(
            accumulated.rotation,
            second_corrected_rotation.inverse() * authority_rotation() * retained.1,
            "the second body-local correction must fold the retained offset into \
             C2^-1 * P2 * O_old",
        );
        assert_eq!(
            (
                app.world()
                    .get::<FrameInterpolate<Position>>(root)
                    .expect("position interpolation")
                    .previous_value,
                app.world()
                    .get::<FrameInterpolate<Rotation>>(root)
                    .expect("rotation interpolation")
                    .previous_value,
            ),
            (None, None),
            "the second depth-zero EndRollback must reopen the same window again",
        );
    }

    #[test]
    fn age_zero_capture_uses_position_history_and_frame_rotation_when_only_position_history_has_t_minus_one()
     {
        let history_position = Position(PREDICTED_POSITION - Vec3::new(0.4, 0.0, 0.0));
        let frame_position = Position(PREDICTED_POSITION - Vec3::new(0.1, 0.0, 0.0));
        let frame_rotation = Rotation(Quat::from_rotation_z(-0.08));
        let (app, root, previous_position, previous_rotation) = run_age_zero_rollback(
            (Some(history_position), None),
            (Some(frame_position), Some(frame_rotation)),
        );
        let captured = captured_of(&app, root);
        assert_near(
            captured.translation,
            previous_position - history_position.0.lerp(AUTHORITY_POSITION, 0.5),
            "the present position-history predecessor must be the position input capture reads",
        );
        assert_near_rotation(
            captured.rotation,
            frame_rotation.0.slerp(authority_rotation(), 0.5).inverse() * previous_rotation,
            "the absent rotation-history predecessor must fall back independently to the retained \
             pre-rollback frame sample",
        );
        let before = app
            .world()
            .resource::<HistoriesBeforeCapture>()
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .expect("history snapshot contains the root");
        assert_eq!(before.1.len(), 2);
        assert_eq!(before.2.len(), 1);
    }

    #[test]
    fn age_zero_capture_uses_frame_position_and_rotation_history_when_only_rotation_history_has_t_minus_one()
     {
        let frame_position = Position(PREDICTED_POSITION - Vec3::new(0.1, 0.0, 0.0));
        let frame_rotation = Rotation(Quat::from_rotation_z(-0.08));
        let history_rotation = Rotation(Quat::from_rotation_y(-0.16));
        let (app, root, previous_position, previous_rotation) = run_age_zero_rollback(
            (None, Some(history_rotation)),
            (Some(frame_position), Some(frame_rotation)),
        );
        let captured = captured_of(&app, root);
        assert_near(
            captured.translation,
            previous_position - frame_position.0.lerp(AUTHORITY_POSITION, 0.5),
            "the absent position-history predecessor must fall back independently to the retained \
             pre-rollback frame sample",
        );
        assert_near_rotation(
            captured.rotation,
            history_rotation
                .0
                .slerp(authority_rotation(), 0.5)
                .inverse()
                * previous_rotation,
            "the present rotation-history predecessor must be the rotation input capture reads",
        );
        let before = app
            .world()
            .resource::<HistoriesBeforeCapture>()
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .expect("history snapshot contains the root");
        assert_eq!(before.1.len(), 1);
        assert_eq!(before.2.len(), 2);
    }

    #[test]
    fn an_age_zero_sharp_occurrence_is_consumed_without_accumulating_the_pose_correction() {
        let mut app = client_app();
        let root = spawn_armed_root(&mut app);
        let _ = set_half_overstep_visual_pose(&mut app, root);
        confirm_pose(
            &mut app,
            root,
            CURRENT_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );
        stage_age_zero_grip_checkpoint(&mut app, root);
        order_sharp(&mut app, root);

        run_pre_update(&mut app);

        assert_near(
            app.world().get::<Position>(root).expect("live position").0,
            AUTHORITY_POSITION,
            "the age-zero grip rollback must restore the differing authority pose",
        );
        assert_eq!(
            offset_of(&app, root),
            (Vec3::ZERO, Quat::IDENTITY),
            "the age-zero occurrence consumed at capture must leave that correction sharp",
        );
    }

    #[test]
    fn the_presentation_layer_never_writes_the_rollback_state_it_offsets() {
        let predecessor_position = Position(PREDICTED_POSITION - Vec3::new(0.1, 0.0, 0.0));
        let predecessor_rotation = Rotation(Quat::IDENTITY);
        let (app, root, _, _) = run_age_zero_rollback(
            (Some(predecessor_position), Some(predecessor_rotation)),
            (Some(predecessor_position), Some(predecessor_rotation)),
        );
        let before = app
            .world()
            .resource::<HistoriesBeforeCapture>()
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .cloned()
            .expect("the seam snapshot includes the corrected root");
        let after = app
            .world()
            .resource::<HistoriesAfterCapture>()
            .roots
            .iter()
            .find(|(entity, ..)| *entity == root)
            .cloned()
            .expect("the post-capture seam snapshot includes the corrected root");
        assert_eq!(
            after, before,
            "capture may write presentation state only; every tick, removal marker, and raw float \
             bit in Lightyear's rollback histories must remain exactly as its Rollback system left \
             them",
        );
    }

    #[test]
    fn a_second_rollback_to_t_minus_one_cannot_restore_a_predecessor_from_presentation_capture() {
        let abandoned_local_predecessor = Position(PREDICTED_POSITION - Vec3::new(0.1, 0.0, 0.0));
        let (mut app, root, _, _) = run_age_zero_rollback(
            (None, None),
            (
                Some(abandoned_local_predecessor),
                Some(Rotation(Quat::IDENTITY)),
            ),
        );
        assert_eq!(
            app.world()
                .get::<PredictionHistory<Position>>(root)
                .expect("position history")
                .get(CURRENT_TICK - 1),
            None,
            "Lightyear left no T-1 baseline after the age-zero rollback",
        );

        // With no ConfirmedHistory, Lightyear's state-rollback restore reads PredictionHistory.
        // Presentation capture must not have invented a T-1 value for this branch to consume.
        app.world_mut()
            .entity_mut(root)
            .remove::<ConfirmedHistory<Position>>();
        app.world_mut().flush();
        order_rollback(&mut app, CURRENT_TICK - 1);
        run_pre_update(&mut app);

        assert_near(
            app.world()
                .get::<Position>(root)
                .expect("the absent baseline leaves the current component present")
                .0,
            AUTHORITY_POSITION,
            "the immediate second restore must leave the corrected pose alone, not move its \
             baseline to frame interpolation's abandoned local predecessor",
        );
        assert_ne!(
            app.world().get::<Position>(root).unwrap().0,
            abandoned_local_predecessor.0,
            "the presentation sample is not rollback truth",
        );
    }

    #[test]
    fn rollback_capture_flows_through_shipping_arming_and_apply_into_frame_trace_telemetry() {
        let path = std::env::temp_dir().join(format!(
            "overmatch-render-error-trace-integration-{}-{}.jsonl",
            std::process::id(),
            Entity::from_raw_u32(1).unwrap().to_bits(),
        ));
        let _ = std::fs::remove_file(&path);

        let mut app = client_app();
        crate::trace::install_test_frame_trace(&mut app, &path);
        let root = spawn_root_through_shipping_arming(&mut app);
        confirm_pose(
            &mut app,
            root,
            ROLLBACK_TICK,
            AUTHORITY_POSITION,
            authority_rotation(),
        );
        order_rollback(&mut app, ROLLBACK_TICK);
        run_pre_update(&mut app);
        assert_ne!(
            offset_of(&app, root),
            (Vec3::ZERO, Quat::IDENTITY),
            "a real rollback must create the telemetry source; this is not a serialization-only \
             fixture",
        );

        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(TICK);
        app.world_mut().run_schedule(PostUpdate);
        let live_offset = offset_of(&app, root);
        crate::trace::close_test_frame_trace(&mut app);

        let contents = std::fs::read_to_string(&path).expect("frame row was flushed");
        let row: serde_json::Value = serde_json::from_str(
            contents
                .lines()
                .last()
                .expect("PostUpdate must write one frame row"),
        )
        .expect("frame row is valid JSON");
        let decoded_vec3 = |field: &str| {
            let values = row[field].as_array().expect("vector field is an array");
            Vec3::new(
                values[0].as_f64().unwrap() as f32,
                values[1].as_f64().unwrap() as f32,
                values[2].as_f64().unwrap() as f32,
            )
        };
        let decoded_quat = |field: &str| {
            let values = row[field].as_array().expect("quaternion field is an array");
            Quat::from_xyzw(
                values[0].as_f64().unwrap() as f32,
                values[1].as_f64().unwrap() as f32,
                values[2].as_f64().unwrap() as f32,
                values[3].as_f64().unwrap() as f32,
            )
        };
        assert_near(
            decoded_vec3("cp"),
            live_offset.0,
            "frame telemetry must carry capture's live, post-decay translation offset",
        );
        assert_near_rotation(
            decoded_quat("cq"),
            live_offset.1,
            "frame telemetry must carry capture's live, post-decay rotation offset",
        );
        assert_eq!(row.get("vo"), row.get("cp"));
        assert_eq!(row.get("voq"), row.get("cq"));

        let _ = std::fs::remove_file(path);
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

    /// The rotation half, the required frame conversion, and the upstream asymmetry it must not be
    /// routed through.
    ///
    /// `Diffable::diff` for `Rotation` is `new * self.inverse()` — a LEFT delta — while the same
    /// impl's `apply_diff` is `self.0 *= delta.0`, a RIGHT multiplication. Right multiplication is
    /// the side this module needs for transport across later Avian motion, but the raw left delta is
    /// not the body-local value. Capture derives `corrected^-1 * previous` explicitly; it does not
    /// pass `previous * corrected^-1` to `apply_diff`.
    #[test]
    fn the_body_local_rotation_offset_is_derived_explicitly_not_applied_as_diffables_left_delta() {
        let corrected = Rotation(authority_rotation());
        let previous = Rotation(predicted_rotation());
        let world_left_error = corrected.diff(&previous);

        assert_near_rotation(
            world_left_error.0 * corrected.0,
            previous.0,
            "Diffable::diff must produce the world-left delta P * C^-1",
        );
        assert!(
            (corrected.0 * world_left_error.0).angle_between(previous.0) > 1e-3,
            "right-applying Diffable's unconverted world-left delta must miss the previous display, \
             or this noncommuting fixture proves nothing",
        );
        let body_local_offset = corrected.0.inverse() * previous.0;
        assert_near_rotation(
            corrected.0 * body_local_offset,
            previous.0,
            "right-applying the explicitly derived body-local offset must reproduce the previous \
             display",
        );
        assert_near_rotation(
            body_local_offset,
            expected_rotation_offset(),
            "the explicit derivation must be the same quantity the schedule fixtures capture",
        );
    }

    /// ARMING IS A SUBSET OF FRAME INTERPOLATION'S. Capture may need its retained T-1 baseline, and
    /// apply needs the current sub-tick pose it produces. The two arming predicates are written in
    /// different modules and are not ordered against each other, so this one is made the narrower of
    /// the two rather than merely documented.
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

    #[test]
    fn a_parented_root_is_not_armed_and_a_late_parent_transition_disables_the_offset_without_panic()
    {
        let mut world = World::new();
        let parent = world.spawn_empty().id();
        let initially_parented = world
            .spawn((
                Predicted,
                NetTank,
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
                ChildOf(parent),
            ))
            .id();
        world
            .run_system_once(arm_render_error)
            .expect("the arming system runs");
        assert!(
            world.get::<RenderErrorOffset>(initially_parented).is_none(),
            "the top-level-root invariant must be part of arming",
        );

        let (mut world, armed) = presentation_world(Vec3::ZERO, Quat::IDENTITY, TICK);
        let parent = world.spawn_empty().id();
        world.entity_mut(armed).insert(ChildOf(parent));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            world
                .run_system_once(apply_render_error)
                .expect("system validation succeeds before the invariant check")
        }));
        assert!(
            result.is_ok(),
            "a cosmetic offset must never crash the client after a hierarchy transition",
        );
        assert!(
            world.get::<RenderErrorOffset>(armed).is_none(),
            "the late-parent fallback disables the offset so it cannot retain stale top-level \
             presentation state",
        );
        assert_eq!(
            world.get::<Transform>(armed).unwrap().translation,
            AUTHORITY_POSITION,
            "the fallback accepts the raw pose already written by the hierarchy pipeline",
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
            "the DERIVED near-bracket retention is roughly 95%, not a freeze — held {held} of 0.2 m",
        );
        assert!(held < 0.2, "and strictly less than all of it");
        assert_eq!(
            world.get::<Transform>(root).unwrap().translation,
            AUTHORITY_POSITION + Vec3::new(held, 0.0, 0.0),
            "the presented pose is the corrected pose plus the surviving offset",
        );

        // FAR regime: 1.0 m. The adaptive retain would remove 0.141 m, so the 3 m/s speed cap binds
        // above its DERIVED ~0.553 m crossover and the frame holds MORE than the retain factor
        // alone would suggest.
        let (mut world, root) = presentation_world(Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY, TICK);
        world.run_system_once(apply_render_error).unwrap();
        let held = world.get::<RenderErrorOffset>(root).unwrap().translation.x;
        assert!(
            ((1.0 - held) - CAP_TRANSLATION_MPS * dt).abs() < 1e-5,
            "above the ~0.553 m crossover the correction VELOCITY cap decides the step, not the \
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

    #[test]
    fn translation_decay_boundaries_are_derived_from_the_constants() {
        let dt = TICK.as_secs_f32();
        let adaptive_retain = |mag: f32| {
            let t = ((mag - DECAY_LERP_LO_M) / (DECAY_LERP_HI_M - DECAY_LERP_LO_M)).clamp(0.0, 1.0);
            (DECAY_RETAIN_NEAR + (DECAY_RETAIN_FAR - DECAY_RETAIN_NEAR) * t).powf(dt * 60.0)
        };
        let uncapped_reduction = |mag: f32| mag * (1.0 - adaptive_retain(mag));

        let near_retain = DECAY_RETAIN_NEAR.powf(dt * 60.0);
        let at_near = decay_magnitude(
            DECAY_LERP_LO_M,
            DECAY_LERP_LO_M,
            DECAY_LERP_HI_M,
            CAP_TRANSLATION_MPS,
            SNAP_TRANSLATION_M,
            dt,
        );
        assert!(
            (at_near / DECAY_LERP_LO_M - near_retain).abs() < 1e-6,
            "95.305% retention applies at the 0.25 m near boundary",
        );
        let half_metre_retain = adaptive_retain(0.5);
        assert!(
            (half_metre_retain - 0.9217).abs() < 1e-4,
            "0.5 m is already in adaptive decay, retaining {half_metre_retain}",
        );

        let cap_step = CAP_TRANSLATION_MPS * dt;
        let mut below = DECAY_LERP_LO_M;
        let mut above = DECAY_LERP_HI_M;
        for _ in 0..32 {
            let middle = (below + above) * 0.5;
            if uncapped_reduction(middle) < cap_step {
                below = middle;
            } else {
                above = middle;
            }
        }
        let crossover = (below + above) * 0.5;
        assert!(
            (0.552..0.554).contains(&crossover),
            "the constants put the 3 m/s crossover at {crossover} m",
        );
        assert!(uncapped_reduction(crossover - 1e-4) < cap_step);
        assert!(uncapped_reduction(crossover + 1e-4) > cap_step);

        let at_snap = decay_magnitude(
            SNAP_TRANSLATION_M,
            DECAY_LERP_LO_M,
            DECAY_LERP_HI_M,
            CAP_TRANSLATION_MPS,
            SNAP_TRANSLATION_M,
            dt,
        );
        assert!(
            (at_snap - (SNAP_TRANSLATION_M - cap_step)).abs() < 1e-6,
            "exactly 2 m is capped and smoothed, not snapped",
        );
        let just_over_snap = f32::from_bits(SNAP_TRANSLATION_M.to_bits() + 1);
        assert_eq!(
            decay_magnitude(
                just_over_snap,
                DECAY_LERP_LO_M,
                DECAY_LERP_HI_M,
                CAP_TRANSLATION_MPS,
                SNAP_TRANSLATION_M,
                dt,
            ),
            0.0,
            "the first representable value above 2 m snaps",
        );
    }

    /// THE SIM IS NEVER FROZEN. This layer writes `Transform` and nothing else; `Position` and
    /// `Rotation` — the rollback state the fixed loop, the replay and replication all read — come
    /// out of the frame exactly as the corrected sim left them.
    #[test]
    fn apply_render_error_changes_only_transform_and_its_owned_offset() {
        let (mut world, root) =
            presentation_world(Vec3::new(0.2, 0.0, 0.0), Quat::from_rotation_z(0.2), TICK);
        let before_transform = *world.get::<Transform>(root).unwrap();
        let before_position = *world.get::<Position>(root).unwrap();
        let before_rotation = *world.get::<Rotation>(root).unwrap();
        let before_offset = *world.get::<RenderErrorOffset>(root).unwrap();
        let before_components = world.entity(root).archetype().components().to_vec();

        let mut expected_offset = before_offset;
        decay_translation(&mut expected_offset.translation, TICK.as_secs_f32());
        decay_rotation(&mut expected_offset.rotation, TICK.as_secs_f32());
        let expected_transform = Transform {
            translation: before_position.f32() + expected_offset.translation,
            rotation: (before_rotation.f32() * expected_offset.rotation).normalize(),
            scale: before_transform.scale,
        };

        world.run_system_once(apply_render_error).unwrap();

        assert_eq!(
            *world.get::<Transform>(root).unwrap(),
            expected_transform,
            "apply owns Transform and must write the exact corrected pose plus the decayed offset",
        );
        assert_eq!(
            *world.get::<RenderErrorOffset>(root).unwrap(),
            expected_offset,
            "apply owns RenderErrorOffset and must perform the exact translation and rotation decay",
        );
        assert_eq!(
            *world.get::<Position>(root).unwrap(),
            before_position,
            "the rollback position is not presentation-owned",
        );
        assert_eq!(
            *world.get::<Rotation>(root).unwrap(),
            before_rotation,
            "the rollback rotation is not presentation-owned",
        );
        assert_eq!(
            world.entity(root).archetype().components().to_vec(),
            before_components,
            "apply must neither add nor remove any component on an unparented root",
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

    /// A SPENT OFFSET DOES NOT DIRTY THE ROOT. The tank root has DERIVED ~194 link children, so a
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
