//! Rigid track contacts in the game's tank driving physics (game state only).
//!
//! [`TrackColliders`] caches body-local shapes from the measured running gear and authored bump
//! travel. [`spawn_track_colliders`] attaches them synchronously with every simulated game or sandbox body:
//! a full-width compression stop per side on [`Layer::Vehicle`], and thin slices at the true shoe
//! faces on [`Layer::TrackSide`]. The slices extend the stop to the rest belt's outer surface;
//! their thickness lies entirely inside the shoe width. Only their lateral planes meet bodies;
//! the contact hook removes their radial contacts. Each side also hangs its soft run (`soft_run`)
//! on [`Layer::TrackBelt`]: loose pieces meet it and the stop, never a side plate. Every face
//! carries the belt's surface speed, and the solver's friction impulses return to the transmission
//! through `stop_contacts`. Replicas use [`spawn_replica_track_colliders`], retaining the queried
//! stops and side plates.

use avian3d::prelude::{Collider, CollisionLayers, Friction, LayerMask, SimpleCollider};
use bevy::prelude::*;

use super::derive::SuspensionParams;
use super::rig_geom::RigGeom;
use super::route::build_route;
use super::side::{PerSide, Side};
use super::soft_run::RunShape;
use super::stop_contacts::{Bearing, BeltFace};
use crate::Layer;

/// One side's bump-stop collider, a child of its body.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct BumpStop;

/// One shoe edge's side-contact slice, a child of its body. Never collides with terrain.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct SidePlate;

/// One tank kind's body-local stops, side plates and soft runs, with the friction of the track's
/// substance ([`crate::spec::TrackSpec::friction`]).
#[derive(Clone)]
pub(crate) struct TrackColliders {
    colliders: PerSide<Collider>,
    /// Inboard then outboard, as in [`RigGeom::grip_columns`].
    side_plates: PerSide<[Collider; 2]>,
    friction: Friction,
    /// Each side's stop face, then its side plates' faces, in `side_plates` order.
    faces: PerSide<(BeltFace, [BeltFace; 2])>,
    runs: PerSide<RunShape>,
}

impl TrackColliders {
    /// Cut the compression prisms, shoe-edge slices and soft runs from `geom`, carrying `friction`,
    /// the track's own ([`crate::spec::TrackSpec::friction`]).
    pub(crate) fn new(geom: &RigGeom, params: &SuspensionParams, friction: Friction) -> Self {
        let colliders = PerSide(
            Side::ALL.map(|side| across_shoe(geom, side, &geom.hard_stop_polyline(side, params))),
        );
        let side_plates = PerSide::new(
            plates(geom, Side::Left, params),
            plates(geom, Side::Right, params),
        );
        let faces = PerSide(Side::ALL.map(|side| {
            let rest: Vec<_> = geom
                .rest
                .get(side)
                .iter()
                .map(|&(centre, radius)| (centre, radius + geom.model.pin_to_outer))
                .collect();
            // The inboard plate's shoe face looks toward the hull's centre, the outboard's away.
            let plate = |out: f32| {
                BeltFace::new(
                    side,
                    build_route(&rest, 0.0).pts,
                    Bearing::Exposed(out * side.sign()),
                )
            };
            (
                BeltFace::new(side, geom.hard_stop_polyline(side, params), Bearing::Whole),
                [plate(-1.0), plate(1.0)],
            )
        }));
        Self {
            colliders,
            side_plates,
            friction,
            faces,
            runs: PerSide(Side::ALL.map(|side| RunShape::build(geom, side))),
        }
    }

    /// DERIVED root-centred sphere enclosing every prepared running-gear shape, in metres.
    /// A sphere also bounds horizontal occupancy at any body orientation.
    pub(crate) fn contact_radius(&self) -> f32 {
        self.bound_corners().map(Vec3::length).fold(0.0, f32::max)
    }

    /// The body-local corners of every prepared shape's bounds: their hull encloses the stops, the
    /// side plates (which reach the rest belt's ground face) and the soft runs, at any pose.
    pub(crate) fn bound_corners(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.colliders
            .values()
            .chain(self.side_plates.values().flat_map(|pair| pair.iter()))
            .chain(self.runs.values().map(RunShape::collider))
            .flat_map(|collider| {
                let bounds = collider.aabb(Vec3::ZERO, Quat::IDENTITY, 0.0);
                (0..8).map(move |k| {
                    Vec3::select(
                        BVec3::new(k & 1 != 0, k & 2 != 0, k & 4 != 0),
                        bounds.max,
                        bounds.min,
                    )
                })
            })
    }

    /// A tank kind's shapes at its authored travel, prepared during startup extraction of the sim
    /// artifact at `glb`. Body spawning reads only these cached shapes.
    pub(crate) fn of_blueprint(
        blueprint: &crate::bake::TankBlueprint,
        glb: &std::path::Path,
    ) -> Self {
        let track = &blueprint.spec.track;
        let params = track.suspension.params();
        let (geom, _) = RigGeom::build(
            blueprint,
            glb,
            track.sprocket.teeth,
            track.link_count,
            &params,
        );
        Self::new(&geom, &params, track.friction())
    }
}

/// Side-plane `profile` extruded across `side`'s shoe between its true lateral faces, as one convex
/// collider in body coordinates: the stop's compression wrap, or a soft run's outline.
pub(super) fn across_shoe(geom: &RigGeom, side: Side, profile: &[Vec2]) -> Collider {
    let cols = geom.grip_columns(side);
    hull(extrude(profile, cols[0].0, cols[2].0), side)
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
    Collider::convex_hull(&points).unwrap_or_else(|| {
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
            // Standing structures also belong to Armor; tank armour volumes are no colliders.
            Layer::Armor,
            Layer::TrackSide,
        ],
    )
}

/// Hang the cached shapes on a simulated `body` at identity: their vertices are body-local,
/// shielded as authored attachments (ADR-0015). Game simulation and sandbox retunes share this.
pub(crate) fn spawn_track_colliders(
    commands: &mut Commands,
    body: Entity,
    shapes: &TrackColliders,
) {
    for side in Side::ALL {
        spawn_side_surfaces(commands, body, shapes, side);
        commands.spawn((
            shapes.runs.get(side).bundle(),
            crate::tank::authored_attachment(Transform::IDENTITY),
            shapes.friction,
            ChildOf(body),
        ));
    }
}

/// Replica queries read the rigid surfaces; the soft runs serve only the authority's solver.
pub(crate) fn spawn_replica_track_colliders(
    commands: &mut Commands,
    body: Entity,
    shapes: &TrackColliders,
) {
    for side in Side::ALL {
        spawn_side_surfaces(commands, body, shapes, side);
    }
}

fn spawn_side_surfaces(commands: &mut Commands, body: Entity, shapes: &TrackColliders, side: Side) {
    commands.spawn((
        BumpStop,
        shapes.faces.get(side).0.clone(),
        shapes.colliders.get(side).clone(),
        crate::tank::authored_attachment(Transform::IDENTITY),
        CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
        shapes.friction,
        ChildOf(body),
    ));
    for (plate, face) in shapes
        .side_plates
        .get(side)
        .iter()
        .zip(&shapes.faces.get(side).1)
    {
        commands.spawn((
            SidePlate,
            face.clone(),
            plate.clone(),
            crate::tank::authored_attachment(Transform::IDENTITY),
            side_plate_layers(),
            shapes.friction,
            ChildOf(body),
        ));
    }
}

#[cfg(test)]
mod tests;
