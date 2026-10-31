//! The bump stop, in the game's tank driving physics (game state only): the rigid shape each
//! track's suspension bottoms on. One convex prism per side, extruded from
//! [`RigGeom::hard_stop_polyline`] (the compression-pose wrap at the wheel rims) across the shoe's
//! measured faces, hung on the tank's body as an ordinary collider on [`Layer::Vehicle`]: it meets
//! the ground and loose pieces alike, and its contacts reach every contact reader as any
//! collider's do.
//!
//! The belt's buoyancy carries the hull; the stop carries nothing until the springs give out. At
//! rest on flat ground it clears the ground by the full plate plus the belly's rise (the invariant
//! [`RigGeom::hard_stop_polyline`] names), so it never stands in for the belt.
//!
//! Built from data at spawn: [`BumpStops`] is cut once per tank kind from its measured running gear
//! and its `track.suspension.bump_stop` travel, and [`spawn_bump_stops`] is the one constructor every
//! body uses, the game's tanks and the track sandbox's rig alike.

use avian3d::prelude::{Collider, CollisionLayers, Friction, LayerMask};
use bevy::prelude::*;

use super::derive::SuspensionParams;
use super::rig_geom::RigGeom;
use super::side::{PerSide, Side};
use crate::Layer;

/// The `materials.ron` row the stop is of: the track shoe steel it stands in for. A named material
/// in logic until the track's own substance is tank data (the pair-friction lane of #238).
pub(crate) const SUBSTANCE: &str = "TrackSteel";

/// One side's bump-stop collider, a child of its body.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct BumpStop;

/// One tank kind's bump-stop prisms in its body frame, with the friction of the steel they stand
/// in for.
#[derive(Resource, Clone)]
pub(crate) struct BumpStops {
    colliders: PerSide<Collider>,
    friction: Friction,
}

impl BumpStops {
    /// Cut both sides' prisms from `geom` at `params`' compression travel.
    pub(crate) fn new(geom: &RigGeom, params: &SuspensionParams) -> Self {
        let colliders = PerSide::new(
            hull(prism(geom, Side::Left, params), Side::Left),
            hull(prism(geom, Side::Right, params), Side::Right),
        );
        let friction = crate::substances::SubstanceRegistry::shipped_static()
            .contact_friction(SUBSTANCE)
            .unwrap_or_else(|err| panic!("the bump stop's substance `{SUBSTANCE}`: {err}"));
        Self {
            colliders,
            friction,
        }
    }

    /// A tank kind's stops at its authored travel, cut from the running gear measured off its sim
    /// artifact at `glb`.
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
        Self::new(&geom, &params)
    }
}

/// `side`'s compression wrap extruded across the shoe's TRUE lateral faces: the edge grip columns
/// ([`RigGeom::grip_columns`] 0 and 2), not `plane_x ± width/2`, because the shoe sits off its pin
/// plane. The side-plane `(z, y)` maps to the body frame as the wheel spawns map it.
fn prism(geom: &RigGeom, side: Side, params: &SuspensionParams) -> Vec<Vec3> {
    let cols = geom.grip_columns(side);
    let (x_in, x_out) = (cols[0].0, cols[2].0);
    geom.hard_stop_polyline(side, params)
        .into_iter()
        .flat_map(|p| [Vec3::new(x_in, p.y, p.x), Vec3::new(x_out, p.y, p.x)])
        .collect()
}

fn hull(points: Vec<Vec3>, side: Side) -> Collider {
    Collider::convex_hull(points).unwrap_or_else(|| {
        panic!(
            "the {side:?} bump stop is degenerate: the compression wrap of a running gear with a \
             sprocket and an idler always encloses area, and the shoe has width"
        )
    })
}

/// Hang `stops`' two prisms on `body`. The points are already in the body frame, so each child sits
/// at identity, shielded as an authored attachment (ADR-0015).
pub(crate) fn spawn_bump_stops(commands: &mut Commands, body: Entity, stops: &BumpStops) {
    for side in Side::ALL {
        commands.spawn((
            BumpStop,
            stops.colliders.get(side).clone(),
            crate::tank::authored_attachment(Transform::IDENTITY),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            stops.friction,
            ChildOf(body),
        ));
    }
}
