//! Client-side smoothing for rollback corrections.
//!
//! The offset is presentation-only: this module writes the predicted root's `Transform`, never its
//! rollback state (`Position`/`Rotation`). `apply_render_error` runs after Avian writeback and before
//! transform propagation, so the root, children, and camera share one rendered pose.

use avian3d::prelude::PhysicsSystems;
use bevy::prelude::*;
use lightyear::prelude::{Predicted, PredictionMetrics};

use super::protocol::NetTank;

/// Presentation-only correction accumulated on the predicted root.
#[derive(Component, Default)]
pub struct RenderErrorOffset {
    pub translation: Vec3,
    pub rotation: Quat,
    /// Previous clean rendered pose; absent until the first frame.
    prev_rendered: Option<(Vec3, Quat)>,
    /// Cumulative rollback count used to detect a correction discontinuity.
    last_rollbacks: u32,
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

/// Arm a predicted root once both replication markers are visible.
fn arm_render_error(
    tanks: Query<Entity, (With<Predicted>, With<NetTank>, Without<RenderErrorOffset>)>,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!("net: {entity} predicted root armed with render-space error offset");
        commands.entity(entity).insert(RenderErrorOffset::default());
    }
}

/// Capture a rollback discontinuity, decay it, and apply it to presentation only.
fn apply_render_error(
    metrics: Option<Res<PredictionMetrics>>,
    time: Res<Time<Real>>,
    mut roots: Query<(&mut Transform, &mut RenderErrorOffset)>,
) {
    let dt = time.delta_secs();

    for (mut transform, mut offset) in &mut roots {
        let cur_t = transform.translation;
        let cur_r = transform.rotation;

        // A changed cumulative count captures the post-rollback pose discontinuity.
        if let Some(rollbacks) = metrics.as_deref().map(|m| m.rollbacks)
            && rollbacks != offset.last_rollbacks
        {
            if let Some((prev_t, prev_r)) = offset.prev_rendered {
                offset.translation += prev_t - cur_t;
                let delta = prev_r * cur_r.inverse();
                offset.rotation = (offset.rotation * delta).normalize();
            }
            offset.last_rollbacks = rollbacks;
        }

        decay_translation(&mut offset.translation, dt);
        decay_rotation(&mut offset.rotation, dt);

        // Store the clean pose before applying the offset.
        offset.prev_rendered = Some((cur_t, cur_r));

        transform.translation += offset.translation;
        transform.rotation = (offset.rotation * transform.rotation).normalize();
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
    use super::*;

    /// The track view detects discontinuities LOCALLY (pose delta per frame) because this
    /// module publishes no signal. That only works while its thresholds sit strictly below the
    /// snap thresholds here: a correction consumed unsmoothed (>= these bounds) must always
    /// exceed the track's trip point, or a snapped hull keeps its old chain state and the
    /// tracks tear. Changing either side's constants must confront this bracket.
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
