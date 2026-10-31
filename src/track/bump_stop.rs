//! Rigid track contacts in the game's tank driving physics (game state only).
//!
//! [`TrackColliders`] caches body-local shapes from the measured running gear and authored bump
//! travel. [`spawn_track_colliders`] attaches them synchronously with every game or sandbox body:
//! a full-width compression stop per side on [`Layer::Vehicle`], and thin slices at the true shoe
//! faces on [`Layer::TrackSide`]. The slices extend the stop to the rest belt's outer surface;
//! their thickness lies entirely inside the shoe width. Their vertical sides meet bodies through
//! ordinary solver contacts, including the structure contact reader. Terrain meets only the stop:
//! side slices never carry the belt's ground support.

use avian3d::prelude::{Collider, CollisionLayers, Friction, LayerMask, SimpleCollider};
use bevy::prelude::*;

use super::derive::SuspensionParams;
use super::rig_geom::RigGeom;
use super::route::build_route;
use super::side::{PerSide, Side};
use crate::Layer;

/// One side's bump-stop collider, a child of its body.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct BumpStop;

/// One shoe edge's side-contact slice, a child of its body. Never collides with terrain.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct SidePlate;

/// One tank kind's body-local stops and side plates, with the friction of the track's substance
/// ([`crate::spec::TrackSpec::friction`]).
#[derive(Resource, Clone)]
pub(crate) struct TrackColliders {
    colliders: PerSide<Collider>,
    /// Inboard then outboard, as in [`RigGeom::grip_columns`].
    side_plates: PerSide<[Collider; 2]>,
    friction: Friction,
}

impl TrackColliders {
    /// Cut the compression prisms and shoe-edge slices from `geom`, carrying `friction`, the
    /// track's own ([`crate::spec::TrackSpec::friction`]).
    pub(crate) fn new(geom: &RigGeom, params: &SuspensionParams, friction: Friction) -> Self {
        let colliders = PerSide::new(
            hull(prism(geom, Side::Left, params), Side::Left),
            hull(prism(geom, Side::Right, params), Side::Right),
        );
        let side_plates = PerSide::new(
            plates(geom, Side::Left, params),
            plates(geom, Side::Right, params),
        );
        Self {
            colliders,
            side_plates,
            friction,
        }
    }

    /// DERIVED root-centred sphere enclosing every prepared running-gear shape, in metres.
    /// A sphere also bounds horizontal occupancy at any body orientation.
    pub(crate) fn contact_radius(&self) -> f32 {
        self.colliders
            .values()
            .chain(self.side_plates.values().flat_map(|pair| pair.iter()))
            .map(|collider| {
                let bounds = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
                bounds.min.abs().max(bounds.max.abs()).length()
            })
            .fold(0.0, f32::max)
    }

    /// A tank kind's shapes at its authored travel, prepared during startup extraction of the sim
    /// artifact at `glb`. Body spawning reads only these cached shapes.
    pub(crate) fn of_blueprint(
        blueprint: &crate::bake::TankBlueprint,
        glb: &std::path::Path,
    ) -> Self {
        let track = &blueprint.spec.track;
        let params = track.suspension.params();
        let geom = RigGeom::build(
            blueprint,
            glb,
            track.sprocket.teeth,
            track.link_count,
            &params,
        );
        Self::new(&geom, &params, track.friction())
    }
}

/// Compression wrap extruded across the shoe's true lateral faces, in body coordinates.
fn prism(geom: &RigGeom, side: Side, params: &SuspensionParams) -> Vec<Vec3> {
    let cols = geom.grip_columns(side);
    let (x_in, x_out) = (cols[0].0, cols[2].0);
    extrude(&geom.hard_stop_polyline(side, params), x_in, x_out)
}

/// Thin side slices cover the compression stop and extend to the rest belt's ground face. They
/// overlap the stop on the same rigid body. Slice depth is DERIVED from the shoe's radial thickness, capped at
/// half its width so the two slices cannot extend past one another. The exposed x face is exactly
/// the measured edge, never `plane_x ± width/2`.
fn plates(geom: &RigGeom, side: Side, params: &SuspensionParams) -> [Collider; 2] {
    let depth = geom.thickness.min(geom.model.width / 2.0);
    assert!(
        depth.is_finite() && depth > 0.0,
        "Link_Box must have positive finite radial thickness and width; fix its shoe volume"
    );
    let outer: Vec<_> = geom
        .rest
        .get(side)
        .iter()
        .map(|&(c, r)| (c, r + geom.model.pin_to_outer))
        .collect();
    let mut profile = geom.hard_stop_polyline(side, params);
    profile.extend(build_route(&outer, 0.0).pts);
    let cols = geom.grip_columns(side);
    [(cols[0].0, side.sign()), (cols[2].0, -side.sign())]
        .map(|(edge, inward)| hull(extrude(&profile, edge, edge + inward * depth), side))
}

/// Side-plane `(z, y)` to body frame, with both lateral faces kept explicit.
fn extrude(profile: &[Vec2], x0: f32, x1: f32) -> Vec<Vec3> {
    profile
        .iter()
        .flat_map(|p| [Vec3::new(x0, p.y, p.x), Vec3::new(x1, p.y, p.x)])
        .collect()
}

fn hull(points: Vec<Vec3>, side: Side) -> Collider {
    Collider::convex_hull(points).unwrap_or_else(|| {
        panic!(
            "the {side:?} track collider is degenerate: a running gear with a sprocket and an \
             idler encloses area, and Link_Box must supply positive finite shoe dimensions"
        )
    })
}

fn side_plate_layers() -> CollisionLayers {
    CollisionLayers::new(
        [Layer::TrackSide],
        [
            Layer::Default,
            Layer::Vehicle,
            Layer::Debris,
            // Standing structures also belong to Armor. Tank armour volumes have empty filters.
            Layer::Armor,
            Layer::TrackSide,
        ],
    )
}

/// Hang the cached shapes on `body` at identity: their vertices are already body-local, shielded
/// as authored attachments (ADR-0015). This is the constructor for game tanks and sandbox retunes.
pub(crate) fn spawn_track_colliders(
    commands: &mut Commands,
    body: Entity,
    shapes: &TrackColliders,
) {
    for side in Side::ALL {
        commands.spawn((
            BumpStop,
            shapes.colliders.get(side).clone(),
            crate::tank::authored_attachment(Transform::IDENTITY),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            shapes.friction,
            ChildOf(body),
        ));
        for plate in shapes.side_plates.get(side) {
            commands.spawn((
                SidePlate,
                plate.clone(),
                crate::tank::authored_attachment(Transform::IDENTITY),
                side_plate_layers(),
                shapes.friction,
                ChildOf(body),
            ));
        }
    }
}

#[cfg(test)]
mod tests;
