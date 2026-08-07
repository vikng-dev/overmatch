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
//! under the teeth at exactly that rate: one tooth per ~160–195 m. Both halves are pinned by tests
//! in `belt` (`the_wrap_view_keeps_its_pins_seated_over_three_hundred_metres` and its
//! `the_naive_drawn_pitch_would_drift_a_pin_onto_a_tooth` receipt).
//!
//! # Binding, and why it composes the whole transform chain
//!
//! The nodes are `Wheel_L_0`..`Wheel_R_7`, `Sprocket_L/R`, `Idler_L/R`. Today they are TOP-LEVEL
//! scene nodes with identity rotation and scale, and their origins have been hand-corrected onto the
//! axles — so rotating about the node origin is rotating about the true axle. None of that is
//! assumed: the export has already moved these nodes under `Hull` / `Track_L` / `Track_R` once and
//! renamed them once, and a wheel that silently stops moving is the worst possible failure for a
//! diagnostic tool. So [`bind_gear_nodes`] composes every transform between the hull body and the
//! node's parent, expresses the hull's own up/lateral axes in that parent space, and keeps the
//! node's authored local `T·R·S` verbatim as the pose everything is written ON TOP of. A baked 180°
//! Y flip or a non-unit scale therefore changes nothing: travel is still hull-up, spin is still
//! about the hull's lateral axis, and the authored scale survives.

use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::track::gear_phase::{fold, gear_transform, spin_angle, sprocket_tooth_tip, tooth_angle};

use super::belt::BeltPhase;
use super::rig_geom::RigGeom;
use super::{Hull, RigWheel, Side, Suspension, VizLayers};

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
/// `pub(super)` because [`gear_slot`] is (so [`super::mesh_layers`] can share the parser); the role
/// itself is only meaningful inside this module.
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
    /// The node's AUTHORED local transform, captured once at bind. Every frame's pose is built from
    /// this, never from the previous frame's — so the layer has no state to drift and switching it
    /// off restores the model's own pose exactly.
    rest: Transform,
    /// The hull's up axis, expressed in this node's PARENT space and scaled by that space's metric:
    /// adding `up * dy` moves the node exactly `dy` metres of HULL space, whatever sits in between.
    up: Vec3,
    /// The hull's lateral axis in the same parent space — the axle the wheel spins about. Unit
    /// length, because it is a rotation axis and not a displacement.
    axle: Vec3,
    /// Road wheels only: the suspension station whose `dy` this node rides.
    station: Option<Entity>,
    /// Sprockets only: the side-plane angle at which this mesh carries a tooth TIP in its authored
    /// rest pose, reduced to one tooth pitch ([`crate::track::gear_phase::measure_tooth_tip_angle`]). Zero and unused for the
    /// rolling roles. Measured once, because a mesh cannot change under a running sandbox — the
    /// calibration built from it is redone every frame, since the rig's tooth count and pitch can.
    tooth_tip: f32,
}

/// Split a glb node name into the running-gear slot it names, or `None` for everything else.
///
/// Deliberately its own parser rather than a reuse of `bake`'s: this is the sandbox's read of the
/// SCENE, and the day the export renames a node it must be one obvious place that goes quiet.
///
/// `pub(super)` so [`super::mesh_layers`] can ask "is this node running gear?" with the same parser —
/// one source of truth for what the sandbox treats as a driven wheel/sprocket/idler.
pub(super) fn gear_slot(name: &str) -> Option<(Side, GearRole)> {
    // Only a purely numeric tail is a station: a `Wheel_L_0_*` sibling (a decor mesh, a proxy) names
    // something parented to the wheel, never the wheel itself. The GAME no longer parses this at all
    // — it reads the explicit `roadwheels` list in `<tank>.tank.ron` (see [`crate::spec::RoadwheelSpec`],
    // §12's identity rule: names address, they never classify). This parser is the SANDBOX's own read
    // of the scene, so it stays a pattern; it just no longer mirrors a bake-side scan that exists.
    for (prefix, side) in [("Wheel_L_", Side::Left), ("Wheel_R_", Side::Right)] {
        if let Some(tail) = name.strip_prefix(prefix)
            && !tail.is_empty()
            && tail.bytes().all(|b| b.is_ascii_digit())
        {
            return Some((side, GearRole::Road));
        }
    }
    match name {
        "Sprocket_L" => Some((Side::Left, GearRole::Sprocket)),
        "Sprocket_R" => Some((Side::Right, GearRole::Sprocket)),
        "Idler_L" => Some((Side::Left, GearRole::Idler)),
        "Idler_R" => Some((Side::Right, GearRole::Idler)),
        _ => None,
    }
}

/// Bind every unbound glb running-gear node.
///
/// A full scan of NAMED-but-unbound entities rather than an `Added<Name>` filter, deliberately: the
/// bind needs the rig's suspension stations to exist, and `Added` gets exactly ONE chance — if the
/// scene ever instantiates on a frame this system cannot complete, the wheels are dead for the whole
/// session with nothing on screen to say so. A few hundred string-prefix compares a frame is not a
/// price worth a race, and the scan re-binds a hot-reloaded scene for free.
fn bind_gear_nodes(
    mut commands: Commands,
    unbound: Query<(Entity, &Name, &Transform), Without<GearNode>>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    transforms: Query<&Transform>,
    primitives: Query<&Mesh3d>,
    meshes: Res<Assets<Mesh>>,
    stations: Query<(Entity, &RigWheel, &Suspension)>,
    hull: Query<Entity, With<Hull>>,
    geom: Res<RigGeom>,
    viz: Res<VizLayers>,
) {
    let Ok(hull) = hull.single() else {
        return;
    };
    for (entity, name, local) in &unbound {
        let Some((side, role)) = gear_slot(name.as_str()) else {
            continue;
        };
        // The node must live under the hull body — the rig's own frame. A node found anywhere else
        // is not part of this tank (a stray template, another scene), and writing hull-local travel
        // into it would fling it across the world.
        let Some(hull_from_parent) = hull_from_parent(entity, hull, &parents, &transforms) else {
            continue;
        };
        let parent_from_hull = hull_from_parent.inverse();
        let station = (role == GearRole::Road)
            .then(|| {
                nearest_station(
                    hull_from_parent.transform_point3(local.translation),
                    side,
                    &stations,
                )
            })
            .flatten();
        if role == GearRole::Road && station.is_none() {
            warn!(
                "running gear: {name} has no suspension station on {side:?} - it will not travel"
            );
        }
        // The sprocket's phase lock stands on its own mesh, so it cannot bind before that mesh is
        // readable. Skipping is the whole recovery: this system re-scans unbound nodes every frame
        // (see above), so a sprocket whose primitive or mesh asset lands a frame late binds a frame
        // late — where a `unwrap_or(0.0)` fallback would bind it permanently mis-meshed instead.
        let tooth_tip = if role == GearRole::Sprocket {
            let Some(measured) = sprocket_tooth_tip(
                entity,
                // The node's full hull-from-node affine: the sandbox composes the whole parent
                // chain (the export has re-parented these nodes before), the game hands its
                // captured rest transform straight in.
                hull_from_parent * local.compute_affine(),
                &children,
                &transforms,
                &primitives,
                &meshes,
                geom.teeth,
            ) else {
                continue;
            };
            let tooth = std::f32::consts::TAU / geom.teeth.max(1) as f32;
            info!(
                "running gear: {name} tooth tips measured at {:.3}° + k·{:.2}° in the hull side \
                 plane — the tip nearest vertical is {:+.3}° off straight up; seating the pins \
                 costs a {:+.3}° phase correction",
                measured.to_degrees(),
                tooth.to_degrees(),
                fold(measured - std::f32::consts::FRAC_PI_2, tooth).to_degrees(),
                fold(
                    tooth_angle(
                        0.0,
                        geom.pitch,
                        geom.teeth,
                        geom.belt_origin_angle(side),
                        measured,
                    ),
                    tooth,
                )
                .to_degrees(),
            );
            measured
        } else {
            0.0
        };
        commands.entity(entity).insert((
            GearNode {
                side,
                role,
                rest: *local,
                up: parent_from_hull.transform_vector3(Vec3::Y),
                axle: parent_from_hull
                    .transform_vector3(Vec3::X)
                    .normalize_or(Vec3::X),
                station: station.map(|(entity, _)| entity),
                tooth_tip,
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

/// The suspension station nearest `rest` on `side`, with its residual (m).
///
/// A POSITION match, not an index match, because both numbers come from the same glb node: the
/// station's `pivot_local` is that node's own root-relative translation as `bake` read it, so the
/// residual is a check that the sandbox and the scene are describing the same wheel — not a fit. The
/// stations are ~0.53 m apart, so a residual anywhere near that means the two reads have diverged
/// and the pairing is worth shouting about.
fn nearest_station(
    rest: Vec3,
    side: Side,
    stations: &Query<(Entity, &RigWheel, &Suspension)>,
) -> Option<(Entity, f32)> {
    let best = stations
        .iter()
        .filter(|(_, wheel, _)| wheel.side == side)
        .map(|(entity, _, susp)| (entity, susp.pivot_local.distance(rest)))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    if best.1 > STATION_MATCH_TOLERANCE {
        warn!(
            "running gear: nearest suspension station on {side:?} is {:.4} m from the glb node - \
             the scene and the bake disagree about where this wheel is",
            best.1,
        );
    }
    Some(best)
}

/// How far a glb wheel node may sit from its suspension station before the pairing is suspect (m).
/// Both are the SAME node read twice, so the honest expectation is zero; a millimetre is slack for
/// the `f32` round trip through the bake, and is two orders of magnitude below the wheel spacing.
const STATION_MATCH_TOLERANCE: f32 = 1e-3;

/// Compose every transform strictly between `hull` and `node` — i.e. the space `node`'s own
/// `Transform` is expressed in, as seen from the hull body.
///
/// `None` if `node` is not a descendant of `hull`. The walk is up the `ChildOf` chain rather than a
/// `GlobalTransform` difference on purpose: global transforms are propagated at the END of the
/// frame, so on the frame a scene instantiates they are all still identity, and a bind that trusted
/// them would silently record the wrong axes.
fn hull_from_parent(
    node: Entity,
    hull: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&Transform>,
) -> Option<Affine3A> {
    let mut chain = Vec::new();
    let mut current = parents.get(node).ok()?.parent();
    while current != hull {
        chain.push(current);
        current = parents.get(current).ok()?.parent();
    }
    // Root-most first: the composition reads hull → … → parent, left to right.
    Some(chain.iter().rev().fold(Affine3A::IDENTITY, |m, &entity| {
        m * transforms
            .get(entity)
            .map_or(Affine3A::IDENTITY, Transform::compute_affine)
    }))
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
            transform.set_if_neq(node.rest);
            continue;
        }
        let travel = phase.get(node.side);
        let (dy, angle) = match node.role {
            GearRole::Road => (
                node.station
                    .and_then(|station| stations.get(station).ok())
                    .map_or(0.0, |susp| susp.dy),
                spin_angle(travel, geom.model.wheel_tread),
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
        transform.set_if_neq(gear_transform(&node.rest, node.up, node.axle, dy, angle));
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
        .map_or(0.0, |&(_, radius)| radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names the export ships map to the slots this module binds — the ONE thing that is this
    /// module's own (the phase law and the tooth-tip measurement it drives them with live in
    /// [`crate::track::gear_phase`], and are pinned by that module's tests against the shipped
    /// Tiger).
    #[test]
    fn the_names_the_export_ships_map_to_slots() {
        assert_eq!(gear_slot("Wheel_L_0"), Some((Side::Left, GearRole::Road)));
        assert_eq!(gear_slot("Wheel_R_7"), Some((Side::Right, GearRole::Road)));
        assert_eq!(
            gear_slot("Sprocket_L"),
            Some((Side::Left, GearRole::Sprocket))
        );
        assert_eq!(gear_slot("Idler_R"), Some((Side::Right, GearRole::Idler)));
        // Sibling volumes are NOT stations — the numeric-tail rule, same as `bake`'s.
        assert_eq!(gear_slot("Wheel_L_0_Visual"), None);
        assert_eq!(gear_slot("Wheel_L_0_Ballistic"), None);
        // The renamed forms the game's binder still looks for must not match either: this module
        // binds what the CURRENT export ships, and a silent double-bind would be worse than a gap.
        assert_eq!(gear_slot("Sprocket_L_Visual"), None);
        assert_eq!(gear_slot("Hull_Visual"), None);
        assert_eq!(gear_slot("Wheel_L_"), None);
    }
}
