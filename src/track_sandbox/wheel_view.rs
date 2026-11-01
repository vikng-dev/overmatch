//! The sandbox's RUNNING-GEAR RENDER LAYER: the glb's own wheel nodes, driven by the belt.
//!
//! The rig's wheel entities ([`super::RigWheel`] + [`super::Suspension`]) carry no mesh — the Tiger
//! glb draws the visible running gear, and its wheel/sprocket/idler nodes are a SEPARATE set of
//! entities. Left untouched, the suspension articulates a number nothing renders and the wheels sit
//! frozen while the shoes scroll past them: the tank looks like it is skating.
//! This module closes that loop. It binds the glb nodes by name, remembers each one's authored
//! REST transform, and rewrites their local transforms every frame from the two belt facts the rest
//! of the sandbox already computes — [`super::Suspension::dy`] (travel) and
//! [`super::belt::BeltPhase`] (rotation).
//!
//! # What is here, and what is not
//!
//! The PHASE LAW is not here. Which angle a road wheel, an idler or a sprocket must carry for a
//! given belt travel — the three rolling radii, the sprocket's tooth lock, the tooth-tip phase
//! measured off the mesh, and the pose composition that survives a baked flip — all lives in
//! [`crate::track::gear_phase`], shared verbatim with the game's `track::view`. Read that module for
//! the derivation and its receipts; what follows is only what BINDING the sandbox's scene needs.
//!
//! # Why the phase, and never a separately-integrated angle
//!
//! [`super::link_view`] places the shoes from `BeltPhase`; so does the belt line; so must the
//! wheels. Reading the SAME accumulator is what makes the three agree by construction instead of by
//! tuning: there is no second integrator to drift, no reset to miss, and a paused sim freezes the
//! wheels and the shoes on the same value.
//!
//! The one number that is NOT shared between the roles is the radius, and getting it from "the
//! wheel's radius" is how a running gear ends up visibly slipping. Road wheels roll on the measured
//! tread ([`crate::track::marker_model::DerivedModel::wheel_tread`]); the idler is toothless but
//! WRAPPED, so it turns at the PIN-LINE radius the route wraps it with
//! ([`idler_pin_radius`]); the sprocket is not a radius at all but a tooth count. See
//! [`crate::track::gear_phase`].
//!
//! ## The drift that used to sit beside the tooth lock
//!
//! Measured end to end on the shipped Tiger — the pins [`super::ConformedBelts`] actually places the
//! shoes on, against the rotation this module actually writes — the nearest tip sits within a
//! fraction of a degree of the exact half tooth, on BOTH sides, in both drive directions, and it
//! STAYS there: the wrap spaces its pins at the MATERIAL pitch, not the drawn one
//! ([`crate::track::wrap::station_params`]). Resampling at the naive `polyline_len / link_count`
//! instead spaced them at the DRAWN pitch — ~0.07 % off on this rig — and the belt walked out from
//! under the teeth at exactly that rate: one tooth per ~160–195 m. `belt`'s
//! `the_wrap_view_keeps_its_pins_seated_over_three_hundred_metres` pins the seated wrap.
//!
//! # Binding, and why it composes the whole transform chain
//!
//! The spec's `roadwheels`, `track.sprocket.nodes` and `track.idler.nodes` address the nodes.
//! [`bind_gear_nodes`] composes every transform between the hull body and the
//! node's parent, expresses the hull's own up/lateral axes in that parent space, and keeps the
//! node's authored local `T·R·S` verbatim as the pose everything is written ON TOP of. A baked 180°
//! Y flip or a non-unit scale therefore changes nothing: travel is still hull-up, spin is still
//! about the hull's lateral axis, and the authored scale survives.

use bevy::prelude::*;

use crate::track::gear_phase::{GearFrame, hull_from_parent, spin_angle, tooth_angle};

use super::belt::BeltPhase;
use super::rig_geom::RigGeom;
use super::{Hull, Side, StationAddress, Suspension, VizLayers};

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            bind_gear_nodes,
            drive_running_gear
                .after(bind_gear_nodes)
                // The travel it reads is written there, and the shoes are placed from the same
                // phase right after — landing between them keeps wheel, belt line and shoe on one
                // generation of the state.
                .after(super::belt::articulate_wheels_field),
        )
            .run_if(resource_exists::<RigGeom>),
    );
}

// ---------------------------------------------------------------------------------------------
// The bound nodes
// ---------------------------------------------------------------------------------------------

/// What a bound glb node is, which decides how its rotation is derived.
///
/// Shared with mesh layering so the authored running gear is excluded from static hull layers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum GearRole {
    /// A sprung road wheel: rises and falls with its suspension station, rolls on the shoe faces.
    Road,
    /// The drive sprocket: hull-fixed, TOOTH-LOCKED to the belt (see the module doc).
    Sprocket,
    /// The rear idler: hull-fixed, wrapped, so it turns at the pin-line radius.
    Idler,
}

/// One glb running-gear node, bound to the belt.
///
/// `pub(super)` only so `mod.rs`'s mesh-visibility mirror can find these entities — everything else
/// about them is this module's business.
#[derive(Component)]
pub(super) struct GearNode {
    side: Side,
    role: GearRole,
    /// The checked authored parent-space pose shared with the game's track view.
    frame: GearFrame,
    /// Road wheels only: the suspension station whose `dy` this node rides.
    station: Option<Entity>,
    /// Sprockets only: the side-plane angle at which this mesh carries a tooth TIP in its authored
    /// rest pose, from the tank sheet. Unused by the rolling roles.
    tooth_tip: f32,
    /// A roadwheel's authored rolling radius; unused for hull-fixed roles.
    rolling_radius: f32,
}

/// The running-gear role explicitly addressed by the spec, shared with mesh layering.
pub(super) fn authored_gear(spec: &crate::spec::TankSpec, name: &str) -> Option<(Side, GearRole)> {
    if let Some(wheel) = spec.roadwheels.iter().find(|wheel| wheel.node == name) {
        let side = match wheel.side {
            crate::tank::TrackSide::Left => Side::Left,
            crate::tank::TrackSide::Right => Side::Right,
        };
        return Some((side, GearRole::Road));
    }
    for (nodes, role) in [
        (&spec.track.sprocket.nodes, GearRole::Sprocket),
        (&spec.track.idler.nodes, GearRole::Idler),
    ] {
        if let Some(side) = nodes.side_of(name) {
            return Some((Side::ALL[side], role));
        }
    }
    None
}

/// Bind every unbound glb running-gear node.
///
/// A full scan of NAMED-but-unbound entities rather than an `Added<Name>` filter, deliberately: the
/// bind needs the rig's suspension stations to exist, and `Added` gets exactly ONE chance — if the
/// scene ever instantiates on a frame this system cannot complete, the wheels are dead for the whole
/// session with nothing on screen to say so. A few hundred string-prefix compares a frame is not a
/// price worth a race, and the scan re-binds a hot-reloaded scene for free.
#[expect(
    clippy::too_many_arguments,
    reason = "binds glb gear nodes: unbound nodes, hierarchy, transforms, meshes, stations, hull, rig and viz flags"
)]
fn bind_gear_nodes(
    mut commands: Commands,
    unbound: Query<(Entity, &Name, &Transform), Without<GearNode>>,
    parents: Query<&ChildOf>,
    transforms: Query<&Transform>,
    stations: Query<(Entity, &StationAddress, &Suspension)>,
    blueprint: Res<crate::bake::TankBlueprint>,
    hull: Query<Entity, With<Hull>>,
    viz: Res<VizLayers>,
) {
    let hull = hull
        .single()
        .expect("RigGeom and exactly one sandbox hull are built in the same command flush");
    for (entity, name, local) in &unbound {
        let Some((side, role)) = authored_gear(&blueprint.spec, name.as_str()) else {
            continue;
        };
        // The node must live under the hull body — the rig's own frame. A node found anywhere else
        // is not part of this tank (a stray template, another scene), and writing hull-local travel
        // into it would fling it across the world.
        let Some(hull_from_parent) = hull_from_parent(entity, hull, &parents, &transforms) else {
            continue;
        };
        let frame = GearFrame::new(*local, hull_from_parent).unwrap_or_else(|fault| {
            panic!("{}: running-gear node {name}: {fault}", blueprint.file)
        });
        let station = if role == GearRole::Road {
            let mut matches = stations
                .iter()
                .filter(|(_, address, _)| address.0 == name.as_str());
            let (station, _, suspension) = matches.next().expect(
                "every spec roadwheel spawns its addressed suspension carrier with the hull",
            );
            assert!(
                matches.next().is_none(),
                "one suspension carrier is spawned per spec roadwheel address"
            );
            // ASSUMED numerical tolerance: a millimetre of slack for the scene/bake f32 transform round trip.
            let residual = suspension
                .pivot_local
                .distance(hull_from_parent.transform_point3(local.translation));
            assert!(
                residual <= 1e-3,
                "running gear: {name} scene/bake station residual {residual} m exceeds the transform round-trip tolerance"
            );
            Some(station)
        } else {
            None // Hull-fixed sprockets and idlers have no suspension travel.
        };
        let tooth_tip = blueprint.spec.track.sprocket.tooth_tip_rad.get(side);
        let rolling_radius = if role == GearRole::Road {
            blueprint
                .spec
                .roadwheels
                .iter()
                .find(|wheel| wheel.node == name.as_str())
                .expect("authored_gear resolved this declared roadwheel")
                .tread
                .radius
        } else {
            0.0
        }; // Hull-fixed roles use their own pin/tooth law.
        commands.entity(entity).insert((
            GearNode {
                side,
                role,
                frame,
                station,
                tooth_tip,
                rolling_radius,
            },
            // EXPLICIT, never `Inherited`: the `2` layer is the running gear's own switch, and it
            // is seeded here as well as in `apply_mesh_visibility` so a freshly-bound node is correct
            // on the very frame it appears — before that every-frame mirror next runs and takes over
            // re-asserting it. `Visible` rather than `Inherited` is the same override the shoes take
            // (`link_view`):
            // now that the wheels really are separate scene nodes, "model off, running gear on" is
            // a state the layers can express.
            if viz.wheels {
                Visibility::Visible
            } else {
                Visibility::Hidden
            },
        ));
    }
}

// ---------------------------------------------------------------------------------------------
// The drive
// ---------------------------------------------------------------------------------------------

/// Pose every bound node from this frame's suspension travel and belt phase.
///
/// Written in the node's own LOCAL transform, so the hull's physics-interpolated pose carries the
/// whole running gear: a wheel can no more lag the tank it is bolted to than a shoe can.
fn drive_running_gear(
    viz: Res<VizLayers>,
    geom: Res<RigGeom>,
    phase: Res<BeltPhase>,
    stations: Query<&Suspension>,
    mut gear: Query<(&GearNode, &mut Transform)>,
) {
    for (node, mut transform) in &mut gear {
        if !viz.running_gear {
            // The authored pose, not the last driven one: the layer's OFF state is the model as
            // Blender ships it, which is exactly the A/B you want against the derived rest gear.
            transform.set_if_neq(node.frame.rest());
            continue;
        }
        let travel = phase.get(node.side);
        let (dy, angle) = match node.role {
            GearRole::Road => (
                stations
                    .get(
                        node.station
                            .expect("a roadwheel binds its authored suspension station"),
                    )
                    .expect("the sandbox's bound suspension carrier lives with its hull")
                    .dy,
                spin_angle(travel, node.rolling_radius),
            ),
            GearRole::Sprocket => (
                0.0,
                tooth_angle(
                    travel,
                    geom.pitch,
                    geom.teeth,
                    // Re-derived every frame, never cached at bind: the sandbox's `n`/`m` knob
                    // retunes the tooth count live, which moves the sprocket's pitch circle and
                    // with it the tangent point zero phase seats pin 0 on.
                    geom.belt_origin_angle(node.side),
                    node.tooth_tip,
                ),
            ),
            // The toothless idler is WRAPPED, not rolled-over: the belt segment around it
            // rotates about the hub at the PIN-LINE rate (the inextensible pin polygon sets
            // wrap rotation — the sprocket case minus teeth), so it spins at travel/R_pin.
            // Only a wheel whose hub TRANSLATES over the stationary belly run rolls at the
            // inner-face radius (2026-07-25 review; the game view agrees).
            GearRole::Idler => (0.0, spin_angle(travel, idler_pin_radius(&geom, node.side))),
        };
        transform.set_if_neq(node.frame.pose(dy, angle));
    }
}

/// The idler's PIN-LINE radius on a side — the last circle of the rest running gear, i.e. the exact
/// circle the route wraps it with. Taken from the assembled geometry rather than re-derived so it
/// and the belt line come from one source; the drive spins the idler at exactly this pin-line
/// radius (see the module doc's "three rolling radii").
fn idler_pin_radius(geom: &RigGeom, side: Side) -> f32 {
    geom.rest
        .get(side)
        .last()
        .map(|&(_, radius)| radius)
        .expect("a rig side's measured rest list ends with its idler")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sandbox and its mesh layers must follow arbitrary authored addresses, including side
    /// membership. FIXTURE DATA: renamed seed-spec entries; none of the names encodes a role.
    /// A mutant retaining the export's name patterns binds none of these stations.
    #[test]
    fn running_gear_roles_follow_the_authored_addresses() {
        let mut spec = crate::track::rig_geom::tiger_spec();
        let old = spec.roadwheels[0].node.clone();
        spec.roadwheels[0].node = "axle-a".into();
        spec.roadwheels[0].side = crate::tank::TrackSide::Right;
        spec.track.sprocket.nodes.left = "drive-a".into();
        spec.track.idler.nodes.right = "tension-a".into();
        assert_eq!(
            authored_gear(&spec, "axle-a"),
            Some((Side::Right, GearRole::Road))
        );
        assert_eq!(
            authored_gear(&spec, "drive-a"),
            Some((Side::Left, GearRole::Sprocket))
        );
        assert_eq!(
            authored_gear(&spec, "tension-a"),
            Some((Side::Right, GearRole::Idler))
        );
        assert_eq!(authored_gear(&spec, &old), None);
        assert_eq!(authored_gear(&spec, "decoration"), None);
    }
}
