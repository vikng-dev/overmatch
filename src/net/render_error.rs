//! The render-space error layer — "the sim snaps, the view never does".
//!
//! Client-only, predicted-tank-only. Rollback reconciliation is tamed in frequency and depth
//! (`protocol`, `client`) but each remaining replay through chaotic contact/friction still lands the
//! corrected present far from the old present. With `CorrectionPolicy::instant_correction()` set on
//! the client `PredictionManager` (`client.rs`), lightyear now CONSUMES that error in a single frame
//! — its own `VisualCorrection` decays to nothing on the frame the rollback lands, so the
//! lightyear-visible pose (`Position`/`Rotation`, hence the post-writeback root `Transform`) SNAPS
//! straight to the corrected present. This layer catches that snap and hides it: it accumulates the
//! per-rollback jump as a render-space offset on the root `Transform`, then decays that offset
//! smoothly (Fiedler's adaptive exponential + a capped correction velocity) so the camera and hull
//! ease from the old pose to the new one instead of lurching. The sim is untouched — the offset
//! lives only on the root's post-writeback `Transform`, which avian re-derives from `Position` every
//! frame, so it can never feed back into the simulation (see the leak analysis on [`apply_render_error`]).
//!
//! Verified instant-correction flow (vendored lightyear 0.28, `lightyear_prediction/src/correction.rs`):
//!   - On rollback end (PreUpdate, `RollbackSystems::EndRollback`),
//!     `update_frame_interpolation_post_rollback` refreshes `FrameInterpolate<C>`'s current/previous
//!     values from the corrected history and — if `PreviousVisual<C>` is present — inserts
//!     `VisualCorrection<C> { error }`. So lightyear's own frame-interpolation state stays coherent.
//!   - In PostUpdate (`RollbackSystems::VisualCorrection`, ordered after
//!     `FrameInterpolationSystems::Interpolate`), `add_visual_correction` scales the error by
//!     `correction_policy.lerp_ratio(dt)`. `instant_correction()` sets `decay_period = 1 ms`,
//!     `decay_ratio = 1e-7` — so for any real frame `dt` the ratio underflows to ~0, the error is
//!     multiplied to ~0 and applied to the component as a ~zero offset, and the next frame the
//!     residual is below the rollback threshold so `VisualCorrection` is removed. Net effect: the
//!     component (`Position`/`Rotation`) equals the corrected present within one frame — the snap we
//!     then hide here. `PreviousVisual`/`VisualCorrection` are still created/consumed; we simply let
//!     them collapse in one frame and do ALL the visible smoothing in this layer.

use avian3d::prelude::PhysicsSystems;
use bevy::prelude::*;
use lightyear::prelude::{Predicted, PredictionMetrics};

use super::protocol::NetTank;

/// The render-space error offset carried on the predicted tank root. Composes across overlapping
/// rollbacks, decays to zero between them. `translation` is a world-space displacement added to the
/// root `Transform`; `rotation` is a world-space pre-rotation (identity = no error). Both are
/// consumed (applied then decayed) every frame in [`apply_render_error`].
#[derive(Component, Default)]
pub struct RenderErrorOffset {
    pub translation: Vec3,
    pub rotation: Quat,
    /// The root's post-writeback, PRE-offset rendered pose captured last frame — the "old present" a
    /// rollback snaps away from. `None` until the first frame is seen (guards a spurious frame-1 delta).
    prev_rendered: Option<(Vec3, Quat)>,
    /// Last-seen cumulative `PredictionMetrics::rollbacks`. A change means a rollback consumed
    /// frame(s) since we last looked, so the lightyear-visible pose has snapped — the capture hook.
    last_rollbacks: u32,
}

// --- Feel dials (all in ONE place) -----------------------------------------------------------
// Fiedler's adaptive exponential decay: the per-60Hz-frame RETAINED fraction lerps from NEAR (a lot
// retained → slow, gentle decay of small errors the eye won't notice) to FAR (little retained → fast
// decay of big errors that would otherwise linger as a visible lag). Frame-rate-normalized by
// `powf(factor, dt * 60)`, so the felt decay is identical at any framerate.
const DECAY_RETAIN_NEAR: f32 = 0.95;
const DECAY_RETAIN_FAR: f32 = 0.85;
/// Translation error (m) at/below which decay is NEAR-slow, and at/above which it is FAR-fast.
const DECAY_LERP_LO_M: f32 = 0.25;
const DECAY_LERP_HI_M: f32 = 1.0;
/// Rotation error (rad) bracket for the same law — 0.25 rad ≈ 14°, 1.0 rad ≈ 57° (just under the snap
/// threshold, so a near-teleport rotation decays at full speed before it snaps).
const DECAY_LERP_LO_RAD: f32 = 0.25;
const DECAY_LERP_HI_RAD: f32 = 1.0;
/// Correction-velocity cap: the offset may SHRINK by at most this much per second, i.e. the view can
/// never move faster than this purely because of a correction — the anti-lurch bound. The
/// exponential decay sets the reduction; this clamps it from being too fast.
const CAP_TRANSLATION_MPS: f32 = 3.0;
const CAP_ROTATION_DPS: f32 = 120.0;
/// Snap threshold: past this the desync is teleport-class — smoothing it would read as a long,
/// obviously-wrong glide, so consume the whole offset at once (let the view snap with the sim).
const SNAP_TRANSLATION_M: f32 = 2.0;
const SNAP_ROTATION_DEG: f32 = 60.0;
/// Below this the offset is treated as spent and zeroed, so it never lingers as denormal dust.
const ZERO_EPS_M: f32 = 1e-4;
const ZERO_EPS_RAD: f32 = 1e-4;

/// The apply system's set: runs AFTER avian's writeback (so the root `Transform` already holds the
/// snapped lightyear-visible pose) and BEFORE the orbit camera reads it and BEFORE transform
/// propagation — so the camera, the hull, and every child render through the offset pose as one.
/// `camera::orbit_camera` is ordered `.after(this)` (see `camera.rs`).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RenderErrorApplied;

/// Client-only: arm the predicted root with a [`RenderErrorOffset`] and run the apply/decay pass in
/// PostUpdate, wedged between avian's writeback and transform propagation. Mounted from `client.rs`
/// alongside `client_smoothing_plugin`; absent entirely from the server (no predicted view to smooth).
pub fn plugin(app: &mut App) {
    app.add_systems(Update, arm_render_error);
    app.add_systems(
        PostUpdate,
        apply_render_error
            .in_set(RenderErrorApplied)
            .after(PhysicsSystems::Writeback)
            .before(TransformSystems::Propagate),
    );
    // The orbit camera reads the root `Transform` it will render through; it must see the offset pose,
    // so wedge our apply before it. Both already sit between `Writeback` and `Propagate`; without this
    // edge the executor could order the camera first and it would orbit the pre-offset pose (the whole
    // world would lurch opposite the hull). Vacuous on a headless client (no `OrbitCameraSet` members).
    app.configure_sets(
        PostUpdate,
        crate::camera::OrbitCameraSet.after(RenderErrorApplied),
    );
}

/// Decorate the predicted tank root with [`RenderErrorOffset`] once `Predicted` is present. A polling
/// system, not an `Add` observer, for the same reason as `arm_predicted_smoothing`: the prediction
/// markers ride replication and arrive in no guaranteed order. Root only — the offset lives on the
/// rollback-participating root; children render through it via propagation.
fn arm_render_error(
    tanks: Query<Entity, (With<Predicted>, With<NetTank>, Without<RenderErrorOffset>)>,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!("net: {entity} predicted root armed with render-space error offset");
        commands.entity(entity).insert(RenderErrorOffset::default());
    }
}

/// Capture-decay-apply, once per render frame, on the predicted root. Ordered after
/// `PhysicsSystems::Writeback` (root `Transform` = the snapped lightyear-visible pose) and before
/// `TransformSystems::Propagate` (so the offset reaches every `GlobalTransform`) and before
/// `camera::orbit_camera` (so the camera orbits the offset pose, not the pre-offset one).
///
/// THE LEAK GATE (verified, vendored lightyear_avian3d 0.28 `AvianReplicationMode::Position` +
/// avian3d 0.7 `transform_to_position`): we mutate ONLY the root `Transform` here, never
/// `Position`/`Rotation`. Two independent guarantees keep the offset out of the sim:
///   1. avian re-derives the root `Transform` from `Position` every frame in `Writeback`
///      (`position_to_transform` fully OVERWRITES translation+rotation), so last frame's offset is
///      gone before we read `Transform` — the capture reads a clean pose, and the offset never
///      compounds through `Transform`.
///   2. The reverse sync `transform_to_position` runs in `RunFixedMainLoop` BEFORE
///      `FrameInterpolationSystems::Restore`, and it only writes `Position` from `Transform` for a
///      body whose `Position` was NOT changed since the last physics tick. The visual pipeline
///      (`FrameInterpolation::Interpolate` + `VisualCorrection`) changes `Position` every frame, so
///      it early-outs for our root; and even if it didn't, `restore_from_visual_interpolation`
///      (armed by `FrameInterpolate<Position>`/`<Rotation>` in `rig.rs`) overwrites `Position` with
///      the real sim value before the next FixedUpdate. This is the same restore cycle that already
///      protects the shipped visual-interpolation `Transform` pollution; our offset rides it.
///
/// On flat cruise with no rollbacks the offset stays exactly zero, so the applied delta is `+ZERO` /
/// `IDENTITY *` — bit-exact-identity, which is why flat-reverse cruise stays bit-exact.
fn apply_render_error(
    metrics: Option<Res<PredictionMetrics>>,
    time: Res<Time<Real>>,
    // `Transform` and `RenderErrorOffset` share the predicted root — one query for both.
    mut roots: Query<(&mut Transform, &mut RenderErrorOffset)>,
) {
    let dt = time.delta_secs();

    for (mut transform, mut offset) in &mut roots {
        // The root's post-writeback, PRE-offset pose this frame — the current lightyear-visible pose.
        let cur_t = transform.translation;
        let cur_r = transform.rotation;

        // CAPTURE. A rollback since last frame means the lightyear-visible pose has snapped; add the
        // discontinuity (last frame's pre-offset pose ⊖ this frame's) INTO the accumulated offset, so
        // the applied pose (below) stays continuous across the snap. Detected by the cumulative
        // `PredictionMetrics::rollbacks` counter changing — a robust, self-contained hook: it is
        // incremented once per rollback in PreUpdate (before this PostUpdate system), monotonic, and
        // already reflects a coalesced N-rollbacks-this-frame as a single net snap; the alternative
        // `Rollback` marker is gone by PostUpdate (added/removed inside PreUpdate) and would need an
        // observer to latch. `Option` so an SP-composition net build (no metrics) simply never fires.
        //
        // Contamination noted (design-accepted for now): `prev_rendered` is one frame stale, so on a
        // rollback frame the delta also folds in ~one frame of legitimate motion (velocity·dt, ~10 cm
        // at cruise). If the harness shows it matters, subtract expected motion here.
        if let Some(rollbacks) = metrics.as_deref().map(|m| m.rollbacks)
            && rollbacks != offset.last_rollbacks
        {
            if let Some((prev_t, prev_r)) = offset.prev_rendered {
                offset.translation += prev_t - cur_t;
                // Right-compose the pre-offset rotational difference (prev_r ⊖ cur_r), so the
                // applied `offset.rotation * cur_r` reproduces last frame's `offset.rotation * prev_r`.
                let delta = prev_r * cur_r.inverse();
                offset.rotation = (offset.rotation * delta).normalize();
            }
            offset.last_rollbacks = rollbacks;
        }

        // DECAY. Fiedler adaptive exponential, then clamp the per-frame reduction to the velocity cap,
        // then a hard snap past the teleport threshold.
        decay_translation(&mut offset.translation, dt);
        decay_rotation(&mut offset.rotation, dt);

        // Remember this frame's PRE-offset pose for next frame's capture (the offset applied below
        // must NOT be folded in, or it would double-count).
        offset.prev_rendered = Some((cur_t, cur_r));

        // APPLY. Displace the root render pose by the offset; children and the camera pick it up
        // through propagation / the `.after(RenderErrorApplied)` camera edge. Zero offset ⇒ identity.
        transform.translation += offset.translation;
        transform.rotation = (offset.rotation * transform.rotation).normalize();
    }
}

/// Fiedler adaptive exponential decay of a scalar-magnitude error toward zero, framerate-normalized,
/// with the per-frame reduction clamped to `cap * dt` (the correction-velocity bound) and a hard snap
/// past `snap`. Shared shape for translation (metres) and rotation (radians).
fn decay_magnitude(mag: f32, lo: f32, hi: f32, cap: f32, snap: f32, dt: f32) -> f32 {
    if mag > snap {
        return 0.0; // teleport-class: consume entirely, let the view snap with the sim
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
    // Sign-normalize to the shortest-path representative (w >= 0), so the angle is in [0, π] and a
    // slerp from identity takes the short way.
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
    // Scale the offset to `new_angle` by slerping from identity by the retained fraction of the angle.
    *offset = Quat::IDENTITY.slerp(q, new_angle / angle).normalize();
}
