//! The game's damage model: the bore's forward exit from the source's physics shapes.

use avian3d::prelude::{Collider, SimpleCollider};
use bevy::prelude::*;

/// The game's damage model: the first open point beyond the source's physics envelope along
/// the bore. A reverse ray starts beyond every shape, so its nearest hit is the forward union
/// boundary even when the muzzle itself is inside a shape. The ordinary march owns everything
/// from this point on, including the barrel's remaining length.
pub(crate) fn outside_source(muzzle: Vec3, bore: Dir3, shapes: &[(Vec3, Quat, &Collider)]) -> Vec3 {
    assert!(
        !shapes.is_empty(),
        "a mounted weapon's tank has physics collision proxies"
    );
    let reach = shapes
        .iter()
        .map(|&(at, turn, shape)| {
            let bounds = shape.aabb(at, turn);
            (bounds.min - muzzle)
                .abs()
                .max((bounds.max - muzzle).abs())
                .length()
        })
        .fold(0.0, f32::max);
    let reach = (reach + crate::ballistics::MARCH_EPS).next_up();
    let beyond = muzzle + bore * reach;
    let distance = shapes
        .iter()
        .filter_map(|&(at, turn, shape)| {
            shape
                .cast_ray(at, turn, beyond, -*bore, reach * 2.0, false)
                .map(|(distance, _)| distance)
        })
        .min_by(f32::total_cmp)
        .expect("a mounted bore intersects its source's physics envelope");
    beyond - bore * distance + bore * crate::ballistics::MARCH_EPS
}
