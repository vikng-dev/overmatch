//! The sandbox's RUNNING-GEAR RENDER LAYER (`F9`): the glb's own wheel nodes, driven by the belt.
//!
//! The rig's wheel entities ([`super::RigWheel`] + [`super::Suspension`]) carry no mesh — the Tiger
//! glb draws the visible running gear, and its wheel/sprocket/idler nodes are a SEPARATE set of
//! entities the sandbox never touched. So the suspension articulated a number nothing rendered and
//! the wheels sat frozen while the shoes scrolled past them: the tank looked like it was skating.
//! This module closes that loop. It binds the glb nodes by name, remembers each one's authored
//! REST transform, and rewrites their local transforms every frame from the two belt facts the rest
//! of the sandbox already computes — [`super::Suspension::dy`] (travel) and
//! [`super::model4::BeltPhase`] (rotation).
//!
//! # Why the phase, and never a separately-integrated angle
//!
//! [`super::link_view`] places the shoes from `BeltPhase`; so does the belt line; so must the
//! wheels. Reading the SAME accumulator is what makes the two agree by construction instead of by
//! tuning: there is no second integrator to drift, no reset to miss, and a paused sim freezes the
//! wheels and the shoes on the same value. It is also drift-free over a long session — the phase is
//! `f64` metres of travel, wrapped per wheel revolution before the `f32` cast, exactly as the game's
//! `track::view::spin_angle` does it.
//!
//! # Three rolling radii, because there are three contacts
//!
//! The one number that is NOT shared is the radius, and getting it from "the wheel's radius" is how
//! a running gear ends up visibly slipping:
//!
//!   * **road wheels** roll on the FLAT INNER FACE of the shoes along the belly, so their no-slip
//!     radius is the measured tread ([`super::model::DerivedModel::wheel_tread`]) — hub to that
//!     face. This is the one an observer can actually check: on flat ground the belly shoes are
//!     stationary against the ground, so a road wheel that spins at anything but `belt / tread`
//!     visibly scrubs.
//!   * **the idler** is toothless but WRAPPED, which is the opposite kinematic case from a road
//!     wheel: the belt segment around it rotates about the hub, and the inextensible PIN LINE
//!     sets that wrap's angular rate — so the idler turns at `travel / R_pin` (the pin-line
//!     circle [`super::rig_geom::RigGeom::rest`] hands the route builder), the sprocket case
//!     minus teeth. Rolling it at the inner-face radius over-rotates it by pin→inner/R ≈ 7%
//!     (2026-07-25 review; the shipped `track::view` was corrected to agree).
//!   * **the sprocket is not a radius at all** — see below.
//!
//! # The sprocket is TOOTH-LOCKED, not speed-derived
//!
//! A sprocket does not roll: its teeth sit between consecutive pins, so one link of belt travel is
//! *exactly* one tooth of rotation — `Δθ = τ / teeth` per `pitch` of travel, by definition, with no
//! radius in the statement at all. Writing it that way is the whole point of the layer: the teeth
//! stay seated in the same pin gaps forever, so a meshing error you SEE is a real geometry error
//! rather than an artefact of two clocks running apart.
//!
//! # ...and it is PHASE-locked as well, which the rate alone does not give you
//!
//! A rate lock says the teeth never drift. It says nothing about WHERE they sit, and for a long time
//! nothing did: the sprocket's absolute angle was whatever fell out of the mesh's authored
//! orientation plus wherever `BeltPhase` happened to be zero, which on the shipped Tiger left every
//! tooth a CONSTANT 5.99° — a third of a tooth, 44 mm of arc at the tip — off its pin gap. Perfectly
//! stable, permanently wrong, and invisible to a rate test.
//!
//! The rule (Yan, 2026-07-23) is the one a real sprocket obeys: **a tooth TIP bisects each adjacent
//! pin pair**, so pins sit at ±½ tooth from every tip and seat in the gullets. That is the same
//! geometry the chord-exact pitch radius `pitch / (2 sin(π/teeth))` already states — it is BY
//! DEFINITION the circle on which the chord between adjacent pins is one pitch, i.e. pins τ/teeth
//! apart — so the phase lock adds no new assumption, only the missing constant.
//!
//! [`tooth_angle`] is therefore built from three facts, none of them typed in:
//!
//!   1. where zero phase puts the first pin — [`RigGeom::belt_origin_angle`], the sprocket-side
//!      upper tangent point that both belt views start their arc length from;
//!   2. the rule — half a tooth further round is where a tip must be;
//!   3. where this sprocket's teeth actually ARE — measured off its own mesh at bind
//!      ([`measure_tooth_tip_angle`]), never asserted. The shipped Tiger authors tooth 0 pointing
//!      straight up to within 0.094°, and both sides agree, but a re-export that turns the sprocket
//!      by half a tooth must move the calibration with it rather than silently un-mesh the track.
//!
//! Then the node's spin is simply "put the mesh's tip where the rule wants a tip", and the rate lock
//! is untouched: the required angle advances exactly τ/teeth per pitch, so the difference does too.
//!
//! ## The one residual, and whose it is
//!
//! Measured end to end on the shipped Tiger — the pins `ConformedBelts` actually places the shoes on
//! against the rotation this module actually writes — the nearest tip sits within **0.05°–0.28°** of
//! the exact half tooth, on BOTH sides, in both drive directions, and it stays there over a hundred
//! metres of driving. That is under the ROUTE CHAIN (`V`, the view that shipped as the game's
//! `track::view`), whose nodes are held one material `pitch` apart by a real length constraint.
//!
//! Under the kinematic wrap it is exact at first and then DRIFTS — measured at one tooth per ~195 m
//! (0.07 %/link), and faster still after a few `'` presses. The cause is not here: that view
//! resamples the loop at `polyline_len / link_count` (`model4::conform_belts_field`), which is the
//! material pitch only up to how well a polyline approximates the arcs and the sag it is drawn from.
//! The teeth are locked to the material pitch — the honest number — so a belt whose drawn stations
//! are 0.07 % apart from it walks out from under them at exactly that rate. Fixing it means the
//! wrap view resampling at `RigGeom::pitch`; calibrating the teeth to the stretched spacing instead
//! would be locking the sprocket to a drawing error.
//!
//! It is worth being precise about why the obvious `phase / pitch_radius` is wrong here, because
//! the radius is right there in [`super::rig_geom::RigGeom::rest`] and it is off by only 0.4 %. That
//! stored radius is the CHORD-exact pitch radius `pitch / (2 sin(π/teeth))` — the circle the pins
//! actually sit on, which is what the route must wrap. But the pins are joined by straight CHORDS of
//! length `pitch`, and a chord is shorter than the arc it subtends: the belt travel per tooth is
//! `pitch`, while the arc per tooth is `2π·R_chord/teeth`. Dividing travel by `R_chord` therefore
//! under-rotates by `2·teeth·sin(π/teeth) / τ` — 0.41 % at 20 teeth, which is one whole tooth of
//! drift every ~244 links (~32 m of driving) and a sprocket visibly chewing its own track within a
//! lap of the course. The tooth statement has no such error term because it never divides a length
//! by a radius.
//!
//! # Binding, and why it composes the whole chain
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
//! about the hull's lateral axis (the rotation PRE-multiplies, so it is applied in parent space, not
//! in the node's own possibly-flipped frame), and the authored scale survives.

use bevy::math::Affine3A;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;

use super::model4::BeltPhase;
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
                .after(super::model4::articulate_wheels_field),
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
    /// rest pose, reduced to one tooth pitch ([`measure_tooth_tip_angle`]). Zero and unused for the
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
    // `Wheel_L_0_Visual` / `Wheel_L_0_Ballistic` are sibling volumes, not the wheel: only a purely
    // numeric tail is a station (the same rule `bake::roadwheel_side` enforces).
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
                local,
                hull_from_parent,
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

/// Belt travel → axle angle for a wheel the belt ROLLS on, wrapped per revolution in `f64` before
/// the `f32` cast so a long session's accumulated travel never erodes the angle's precision.
///
/// The sign is the one place this module's rotation convention lives, and it is forced: positive
/// phase scrolls the belly toward `+z` (the loop is ordered sprocket → belly → idler), so a wheel's
/// contact point must travel `+z`, and a positive rotation about `+x` moves the bottom of a wheel
/// toward `−z`. Hence negative — the same flip the game's `track::view::spin_angle` carries.
fn spin_angle(travel: f64, radius: f32) -> f32 {
    if radius <= 1e-6 {
        return 0.0;
    }
    let circumference = f64::from(radius) * std::f64::consts::TAU;
    -(travel.rem_euclid(circumference) / f64::from(radius)) as f32
}

/// Where a tooth TIP must sit at belt travel `travel`: the side-plane angle (rad) of the tip that
/// bisects the pin pair straddling the loop's arc-length origin.
///
/// This is THE rule, and it is two terms and no fitting:
///
///   * `origin` = [`RigGeom::belt_origin_angle`] — the angle zero phase puts pin 0 at;
///   * `+ ½ tooth` — the rule itself: a tip bisects adjacent pins, which on the chord-exact pitch
///     circle (where consecutive pins are exactly `τ/teeth` apart) is exactly half a tooth on;
///   * `+ turn` — one tooth per pitch of travel, the proven rate lock, wrapped per full REVOLUTION
///     of the sprocket (`teeth × pitch` metres of belt) in `f64` so a long session's accumulated
///     travel never erodes the angle. No radius appears, which is the point — see the module doc
///     for why the pitch radius sitting next door would drift a whole tooth every ~244 links.
///
/// Increasing, because positive travel scrolls the belt forward around the sprocket's front, i.e.
/// counter-clockwise in the `(z, y)` side plane the angle is measured in.
pub(super) fn tooth_tip_angle(travel: f64, pitch: f32, teeth: u32, origin: f32) -> f32 {
    let tooth = std::f32::consts::TAU / teeth as f32;
    let per_revolution = f64::from(pitch) * f64::from(teeth);
    let turn = (travel.rem_euclid(per_revolution) / per_revolution * std::f64::consts::TAU) as f32;
    origin + tooth / 2.0 + turn
}

/// Belt travel → sprocket node spin about the hull's lateral axis (rad): **put this mesh's own tooth
/// tip where the rule wants a tip**.
///
/// `mesh_tip` is where the mesh carries a tip at zero spin ([`measure_tooth_tip_angle`]); the target
/// is [`tooth_tip_angle`]. A positive spin about the hull's `+x` DECREASES a side-plane angle (it
/// rolls `+y` toward `+z`), so the difference is taken tip-minus-target rather than the other way
/// round — the same flip [`spin_angle`] carries, and the reason positive travel still turns the
/// sprocket negatively.
///
/// The tooth-per-pitch RATE is untouched by the calibration: only the target moves with travel, and
/// it moves by exactly one tooth per pitch, so the spin does too. Any representative of `mesh_tip`
/// modulo one tooth is as good as any other — the teeth are `τ/teeth`-periodic, which is precisely
/// what makes a single measured constant enough to seat all of them.
fn tooth_angle(travel: f64, pitch: f32, teeth: u32, origin: f32, mesh_tip: f32) -> f32 {
    // A rig that could not fill in a pitch or a tooth count parks the sprocket rather than
    // producing a NaN transform that would propagate into every child of the hull.
    if f64::from(pitch) * f64::from(teeth) <= 1e-6 {
        return 0.0;
    }
    mesh_tip - tooth_tip_angle(travel, pitch, teeth, origin)
}

/// Reduce an angle to the signed representative nearest zero within one `period` — "how far off is
/// it, and which way", for readouts and for assertions about a periodic quantity.
fn fold(angle: f32, period: f32) -> f32 {
    (angle + period / 2.0).rem_euclid(period) - period / 2.0
}

// ---------------------------------------------------------------------------------------------
// Reading the teeth off the sprocket mesh
// ---------------------------------------------------------------------------------------------

/// Fraction of the rim radius a vertex must reach to count as tooth-TIP land. Two per cent of the
/// Tiger's 0.4328 m rim is 8.7 mm, which takes the whole tip land (its four corners span 0.4323 to
/// 0.4328 m) and nothing else: the next feature inboard is the gullet floor, 39 mm down.
const TIP_BAND: f32 = 0.98;
/// Quantile of the in-plane radii that anchors "this is the rim", and how far past it a vertex may
/// still sit and count. Same construction (and same reason) as `super::model`'s disc radius: a rim
/// is a RING of hundreds of vertices so it always clears the quantile, while a stray from a boolean
/// or a loose greeble never does — and a raw `max` is the one statistic a single stray destroys.
const RIM_QUANTILE: f32 = 0.95;
const RIM_BAND: f32 = 1.01;
/// How sharply the tip band must actually cluster on a `teeth`-fold grid before the measurement is
/// believed (the mean resultant length of the fitted harmonic, 0 = no structure, 1 = a delta). The
/// Tiger scores 0.587 — the tip is a LAND, not a point, so its own angular width caps the score
/// well below 1; anything that fails this is not a sprocket with this many teeth.
const TOOTH_CONCENTRATION: f32 = 0.25;

/// Measure the tooth-tip phase of a bound sprocket node from its own mesh, in the HULL's side plane.
///
/// The mesh hangs on PRIMITIVE children of the node (`bevy_gltf` spawns one child per primitive), so
/// the walk composes hull ← parent ← node ← primitive and takes every position through it. Doing the
/// whole thing in hull space rather than in mesh space is what makes it survive the export: a baked
/// 180° Y flip, a non-unit scale or a re-parenting all change where the teeth are in the node's own
/// frame, and none of them change where they are on the tank — which is the only frame the answer is
/// used in ([`gear_transform`] spins about the hull's lateral axis, in parent space).
///
/// `None` if the mesh cannot be read yet or does not look like a `teeth`-fold star; the caller
/// leaves the node unbound and tries again next frame.
#[allow(clippy::too_many_arguments)]
fn sprocket_tooth_tip(
    entity: Entity,
    node_local: &Transform,
    hull_from_parent: Affine3A,
    children: &Query<&Children>,
    transforms: &Query<&Transform>,
    primitives: &Query<&Mesh3d>,
    meshes: &Assets<Mesh>,
    teeth: u32,
) -> Option<f32> {
    let hull_from_node = hull_from_parent * node_local.compute_affine();
    // The node ORIGIN is the axle (hand-corrected onto it, and `super::model` guards that), so it is
    // the centre every tooth angle is measured about — never a vertex statistic, which on a toothed
    // rim is not a circle in the first place.
    let axle = hull_from_node.transform_point3(Vec3::ZERO);
    let mut polar: Vec<(f32, f32)> = Vec::new();
    for child in children.get(entity).ok()?.iter() {
        let Ok(primitive) = primitives.get(child) else {
            continue;
        };
        let mesh = meshes.get(&primitive.0)?;
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        let hull_from_mesh = hull_from_node
            * transforms
                .get(child)
                .map_or(Affine3A::IDENTITY, Transform::compute_affine);
        polar.extend(positions.iter().map(|p| {
            let v = hull_from_mesh.transform_point3(Vec3::from(*p)) - axle;
            // The side plane, by definition and not by inference: every axle in a tank's running
            // gear is lateral, so the tooth ring lives in `(z, y)` — the same plane the route,
            // `belt_origin_angle` and the spin axis are all expressed in, and `atan2(y, z)` is the
            // angle all three of them mean.
            (Vec2::new(v.z, v.y).length(), v.y.atan2(v.z))
        }));
    }
    measure_tooth_tip_angle(&polar, teeth)
}

/// The `teeth`-fold phase of the tip land in a sprocket's `(radius, side-plane angle)` cloud — i.e.
/// one angle at which the mesh carries a tooth tip, reduced to `[0, τ/teeth)`.
///
/// The estimator is the `teeth`-th circular harmonic of the tip band: sum `e^{i·teeth·α}` over the
/// band and take the argument. That is not a heuristic — it IS the definition of the phase of a
/// `teeth`-fold rotational symmetry, so it uses every vertex of every tooth rather than picking a
/// feature, it is exact for a symmetric tip land whatever the land's width, and it needs no
/// clustering, no bin size and no ordering. Its magnitude comes out for free as the confidence that
/// the thing measured really has that symmetry ([`TOOTH_CONCENTRATION`]).
fn measure_tooth_tip_angle(polar: &[(f32, f32)], teeth: u32) -> Option<f32> {
    if teeth == 0 || polar.len() < teeth as usize {
        return None;
    }
    let mut radii: Vec<f32> = polar.iter().map(|&(r, _)| r).collect();
    radii.sort_by(f32::total_cmp);
    let rim = radii[((RIM_QUANTILE * (radii.len() - 1) as f32) as usize).min(radii.len() - 1)];
    let tip = radii.iter().rev().find(|r| **r <= rim * RIM_BAND)?;
    let band = tip * TIP_BAND;

    // `f64` for the accumulation only: the sum runs over thousands of terms and its ARGUMENT is the
    // whole answer, so cancellation in the tail is not something to hand to an f32 accumulator.
    let (mut sx, mut sy, mut n) = (0.0_f64, 0.0_f64, 0_u32);
    for (_, angle) in polar.iter().filter(|&&(r, _)| r >= band) {
        let harmonic = f64::from(teeth) * f64::from(*angle);
        sx += harmonic.cos();
        sy += harmonic.sin();
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let concentration = sx.hypot(sy) / f64::from(n);
    if concentration < f64::from(TOOTH_CONCENTRATION) {
        warn!(
            "running gear: a sprocket's rim does not read as a {teeth}-fold tooth ring \
             (concentration {concentration:.3} over {n} rim vertices, need \
             {TOOTH_CONCENTRATION}). Either the mesh's tooth count is not the authored one or the \
             node's origin is off its axle - the track cannot be phase-locked to teeth that are \
             not there.",
        );
        return None;
    }
    let tooth = std::f32::consts::TAU / teeth as f32;
    Some(((sy.atan2(sx) / f64::from(teeth)) as f32).rem_euclid(tooth))
}

/// The node's local transform for `dy` metres of hull-up travel and `angle` of spin about the axle.
///
/// Two things are load-bearing. The spin PRE-multiplies the authored rotation — that applies it in
/// the node's PARENT space, so it is a rotation about the hull's lateral axis whatever the node's
/// own baked orientation is; post-multiplying would spin about the node's own `x`, which a baked
/// 180° Y flip silently reverses (one track's wheels turning backwards). And the translation is
/// untouched by the spin, which is what makes this a rotation about the NODE ORIGIN — now the
/// hand-corrected true axle — rather than about the model origin.
fn gear_transform(rest: &Transform, up: Vec3, axle: Vec3, dy: f32, angle: f32) -> Transform {
    Transform {
        translation: rest.translation + up * dy,
        rotation: Quat::from_axis_angle(axle, angle) * rest.rotation,
        // Carried, never assumed: the wheel nodes have shipped with a non-unit scale before.
        scale: rest.scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Tiger's authored counts, as the 2026-07-23 export carries them. INPUTS to the pure math
    /// below, not assertions about the file — a re-authoring that changes them must not turn this
    /// suite red.
    const TEETH: u32 = 20;
    const PITCH: f32 = 0.130;
    /// A belt origin and a mesh tooth phase that are both deliberately NOT round numbers, so a
    /// calibration term that silently dropped out of the rate math would show up here.
    const ORIGIN: f32 = 1.5199;
    const MESH_TIP: f32 = 0.0163;

    /// The spin this module would write for `travel`, at the fixture rig.
    fn spin(travel: f64) -> f64 {
        f64::from(tooth_angle(travel, PITCH, TEETH, ORIGIN, MESH_TIP))
    }

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

    /// THE sprocket property: one link of belt travel is exactly one tooth of rotation, forever.
    /// Checked after 5 000 links — a whole session of driving — because the failure mode this
    /// module exists to prevent is a slow drift, not a first-frame error.
    #[test]
    fn one_link_of_travel_is_exactly_one_tooth() {
        let tooth = std::f64::consts::TAU / f64::from(TEETH);
        let seated = spin(0.0);
        for links in [1_i32, 2, 19, 20, 21, 5_000] {
            let travel = f64::from(links) * f64::from(PITCH);
            // Measured against the SEATED zero-phase angle, not against zero: the phase lock adds a
            // constant, and a rate test that could not tell the two apart is the test that let a
            // 5.99° mis-mesh sit here for a month. Rotation is negative (see `spin_angle`), and only
            // defined modulo a full turn.
            let residual = (seated - spin(travel) - f64::from(links) * tooth)
                .rem_euclid(std::f64::consts::TAU);
            let err = residual.min(std::f64::consts::TAU - residual);
            assert!(
                err < 1e-5,
                "{links} links landed {err} rad off a whole tooth",
            );
        }
    }

    /// ...and why it is not `phase / pitch_radius`. The chord-exact pitch radius is the circle the
    /// pins sit on, so it is right for the ROUTE and wrong for the ROTATION: chords are shorter than
    /// their arcs, so dividing travel by it under-rotates, and the error accumulates into a full
    /// tooth within a lap of the course. This test is the receipt for that claim.
    #[test]
    fn the_chord_radius_would_drift_a_whole_tooth() {
        let chord_radius = PITCH / (2.0 * (std::f32::consts::PI / TEETH as f32).sin());
        let tooth = std::f64::consts::TAU / f64::from(TEETH);
        // One link in, the two agree to well under a tooth — which is exactly why the bug hides.
        let one_link = f64::from(PITCH);
        let by_radius = -f64::from(spin_angle(one_link, chord_radius));
        assert!((by_radius - tooth).abs() < 0.01 * tooth);
        // ...but the RATES differ, and that is what accumulates. Measured over one link, where
        // neither angle has wrapped yet, so the comparison is of two raw rotations and not of two
        // residues: the tooth lock turns further per link, by a constant fraction.
        let per_link = spin(0.0) - spin(one_link);
        assert!(
            per_link > by_radius,
            "the chord radius under-rotates - a chord is shorter than its arc",
        );
        let links_to_a_tooth = tooth / (per_link - by_radius);
        assert!(
            (links_to_a_tooth - 244.0).abs() < 15.0,
            "expected a whole tooth of drift within ~244 links, got {links_to_a_tooth}",
        );
        // The tooth lock has none of it, at any distance.
        for links in [244.0, 2_440.0] {
            let travel = links * f64::from(PITCH);
            let residual = (spin(0.0) - spin(travel) - links * tooth).rem_euclid(tooth);
            assert!(residual.min(tooth - residual) < 1e-4);
        }
    }

    /// The rolling wheels turn the way a wheel turns: driving forward scrolls the belly toward `+z`,
    /// so the wheel's CONTACT POINT must travel `+z` with it, and the arc it sweeps must equal the
    /// belt travel exactly — that is what "no visible slip on flat ground" means numerically.
    #[test]
    fn a_rolling_wheel_matches_the_ground_it_rolls_on() {
        let radius = 0.41;
        let travel = 0.37;
        let angle = spin_angle(travel as f64, radius);
        assert!(
            angle < 0.0,
            "positive travel must turn the wheel negatively"
        );
        assert!(
            ((-angle * radius) - travel).abs() < 1e-5,
            "the swept arc must equal the belt travel",
        );
        // The bottom of the wheel really does move toward +z under that rotation.
        let bottom = Quat::from_rotation_x(angle) * Vec3::new(0.0, -radius, 0.0);
        assert!(bottom.z > 0.0, "the contact point went the wrong way");
        // And travel that wraps many revolutions still lands on the same angle as the wrapped one.
        let laps = travel as f64 + 7.0 * f64::from(radius) * std::f64::consts::TAU;
        assert!((spin_angle(laps, radius) - angle).abs() < 1e-4);
    }

    /// The composition rule, against the two things the export has actually shipped: a baked 180° Y
    /// rotation and a non-unit scale. Travel must stay hull-vertical, spin must stay about the
    /// HULL's lateral axis (not the node's flipped one), the scale must survive, and the node ORIGIN
    /// — the hand-corrected axle — must not move when only the spin changes.
    #[test]
    fn a_flipped_scaled_node_still_travels_up_and_spins_about_the_axle() {
        let rest = Transform {
            translation: Vec3::new(1.67, 0.51, -1.88),
            rotation: Quat::from_rotation_y(std::f32::consts::PI),
            scale: Vec3::splat(0.8),
        };
        let angle = -0.7;
        let posed = gear_transform(&rest, Vec3::Y, Vec3::X, 0.045, angle);

        assert_eq!(posed.scale, rest.scale, "the authored scale must survive");
        assert_eq!(
            posed.translation,
            rest.translation + Vec3::Y * 0.045,
            "travel is hull-vertical and nothing else",
        );
        // The node origin (the axle) is exactly the translation — spin does not move it.
        let spun_only = gear_transform(&rest, Vec3::Y, Vec3::X, 0.045, angle * 3.0);
        assert_eq!(spun_only.translation, posed.translation);

        // The visible rotation is a pure `angle` about the hull's +x, composed onto the rest pose —
        // NOT about the node's own (flipped) x. Post-multiplying would give the opposite sense.
        let want = Quat::from_rotation_x(angle) * rest.rotation;
        assert!(posed.rotation.angle_between(want) < 1e-5);
        let wrong = rest.rotation * Quat::from_rotation_x(angle);
        assert!(
            posed.rotation.angle_between(wrong) > 1.0,
            "the flip makes the two conventions genuinely different - that is the point",
        );
        // Concretely: a point on the rim still sweeps the right way in hull space.
        let rim = |t: &Transform| t.transform_point(Vec3::new(0.0, -0.4, 0.0)) - t.translation;
        assert!(
            rim(&posed).z > rim(&rest).z,
            "the bottom of the wheel must move toward +z",
        );
    }

    /// A degenerate rig (a radius or a count the derivation could not fill in) parks the wheel
    /// instead of producing a NaN transform that would propagate into every child of the hull.
    #[test]
    fn a_degenerate_radius_parks_the_wheel() {
        assert_eq!(spin_angle(1.0, 0.0), 0.0);
        assert_eq!(tooth_angle(1.0, PITCH, 0, ORIGIN, MESH_TIP), 0.0);
        assert_eq!(tooth_angle(1.0, 0.0, TEETH, ORIGIN, MESH_TIP), 0.0);
        // ...and a mesh that is not a tooth ring is refused rather than averaged into a phase.
        assert_eq!(measure_tooth_tip_angle(&[], TEETH), None);
        let smooth_disc: Vec<(f32, f32)> = (0..720)
            .map(|i| (0.43, std::f32::consts::TAU * i as f32 / 720.0))
            .collect();
        assert_eq!(measure_tooth_tip_angle(&smooth_disc, TEETH), None);
    }

    // -----------------------------------------------------------------------------------------
    // The phase lock
    // -----------------------------------------------------------------------------------------

    /// A synthetic sprocket rim: `teeth` tip lands of `land` radians each, centred on
    /// `phase + k·τ/teeth`, plus a hub ring well inside them. Returned in the `(radius, angle)` form
    /// the measurement consumes, so the estimator is driven by a shape whose answer is KNOWN.
    fn synthetic_rim(teeth: u32, phase: f32, land: f32) -> Vec<(f32, f32)> {
        let tooth = std::f32::consts::TAU / teeth as f32;
        let mut polar = Vec::new();
        for k in 0..teeth {
            let centre = phase + tooth * k as f32;
            for j in 0..9 {
                let t = j as f32 / 8.0 - 0.5;
                polar.push((0.4328, centre + land * t));
            }
        }
        // The hub: a smooth ring at a radius the tip band must exclude, and four times as many
        // vertices as the teeth have, so anything that failed to band-limit would be swamped by it.
        for i in 0..(teeth * 4) {
            polar.push((0.3465, tooth * i as f32 / 4.0 + 0.37));
        }
        polar
    }

    /// The estimator recovers the phase of a known tooth ring — at any phase, any land width, and
    /// with a hub ring outvoting the teeth four to one.
    #[test]
    fn the_tooth_phase_estimator_recovers_a_known_ring() {
        let tooth = std::f32::consts::TAU / f32::from(20_u8);
        for phase in [0.0_f32, 0.0016, 0.1, tooth - 0.01] {
            for land in [0.001_f32, 0.05, 0.096] {
                let got = measure_tooth_tip_angle(&synthetic_rim(20, phase, land), 20)
                    .expect("a 20-fold ring measures");
                assert!(
                    fold(got - phase, tooth).abs() < 1e-4,
                    "phase {phase} land {land} measured as {got}",
                );
            }
        }
        // The answer is a REPRESENTATIVE modulo one tooth, so a ring authored a whole tooth round
        // reads identically — which is exactly why one measured constant seats all twenty teeth.
        let a = measure_tooth_tip_angle(&synthetic_rim(20, 0.1, 0.05), 20).unwrap();
        let b = measure_tooth_tip_angle(&synthetic_rim(20, 0.1 + tooth, 0.05), 20).unwrap();
        assert!((a - b).abs() < 1e-4);
    }

    /// **THE RULE**, on pure math: a tooth tip bisects every adjacent pin pair, so a pin lands in a
    /// gullet — at any phase, in either direction, forever.
    ///
    /// Pins are derived, not pinned: pin `k` sits at `origin + k·τ/teeth` when the belt has not
    /// moved (the chord-exact pitch circle is BY DEFINITION the one where consecutive pins are one
    /// tooth apart), and the whole set walks forward by `τ/teeth` per pitch of travel. The tooth the
    /// mesh carries is then rotated by [`tooth_angle`], and the assertion is that the two interleave
    /// at exactly half a tooth.
    #[test]
    fn a_pin_lands_in_a_gullet_at_every_phase() {
        let tooth = std::f32::consts::TAU / TEETH as f32;
        for links in [
            0.0_f64, 0.25, 0.5, 1.0, 1.5, 7.0, 19.5, 20.0, 41.0, -1.0, -13.75, -400.0, 5_000.0,
        ] {
            let travel = links * f64::from(PITCH);
            // Where the belt puts its pins on the sprocket, from the belt's own facts alone.
            let pin = ORIGIN + (links as f32) * tooth;
            // Where the mesh's teeth end up, from the spin this module writes. A spin of `θ` about
            // the hull's `+x` takes a side-plane angle to `angle − θ`.
            let tip = MESH_TIP - tooth_angle(travel, PITCH, TEETH, ORIGIN, MESH_TIP);
            let offset = fold(tip - pin, tooth);
            assert!(
                (offset.abs() - tooth / 2.0).abs() < 1e-4,
                "at {links} links the nearest tip sits {:.4}° from a pin - it must be exactly \
                 {:.4}° (half a tooth) for the pin to seat in a gullet",
                offset.to_degrees(),
                tooth.to_degrees() / 2.0,
            );
            // Said the other way round, because it is the way you check it on screen: the GULLET
            // (half a tooth off a tip) is where the pin is, to within a rounding.
            let gullet = tip + tooth / 2.0;
            assert!(fold(gullet - pin, tooth).abs() < 1e-4);
        }
    }

    /// The same rule on the SHIPPED Tiger, end to end: the sprocket mesh out of the glb, the belt
    /// origin out of the derived rig, and no constant in between. This is the test a re-export has
    /// to get past — turn the sprocket in Blender, or move the idler so the top run's tangent
    /// point shifts, and the calibration must follow rather than the track quietly un-meshing.
    #[test]
    fn the_shipped_tiger_seats_its_pins_in_its_gullets() {
        let rig = super::super::rig_geom::tiger_rig();
        let tooth = std::f32::consts::TAU / rig.teeth as f32;
        for side in [Side::Left, Side::Right] {
            let node = match side {
                Side::Left => "Sprocket_L",
                Side::Right => "Sprocket_R",
            };
            let mesh_tip = glb_sprocket_tip(node, rig.teeth);
            let origin = rig.belt_origin_angle(side);
            println!(
                "{node}: tooth tips at {:.4}° + k·{:.2}°, {:+.4}° off straight up; belt origin \
                 {:.4}°; seating correction {:+.4}°",
                mesh_tip.to_degrees(),
                tooth.to_degrees(),
                fold(mesh_tip - std::f32::consts::FRAC_PI_2, tooth).to_degrees(),
                origin.to_degrees(),
                fold(
                    tooth_angle(0.0, rig.pitch, rig.teeth, origin, mesh_tip),
                    tooth
                )
                .to_degrees(),
            );

            // The AUTHORING contract Yan states — "tooth 0 points straight up". Not assumed
            // anywhere in the derivation (the measured angle is what is actually used); asserted
            // here so that if a re-export stops honouring it, the report says so out loud instead
            // of the calibration silently absorbing it.
            assert!(
                fold(mesh_tip - std::f32::consts::FRAC_PI_2, tooth).abs() < 0.005,
                "{node}'s teeth are no longer authored with tooth 0 straight up",
            );

            for links in [0.0_f64, 0.5, 3.0, 9.25, 20.0, 137.0, -6.5] {
                let travel = links * f64::from(rig.pitch);
                let pin = origin + (links as f32) * tooth;
                let tip = mesh_tip - tooth_angle(travel, rig.pitch, rig.teeth, origin, mesh_tip);
                assert!(
                    (fold(tip - pin, tooth).abs() - tooth / 2.0).abs() < 1e-4,
                    "{node} at {links} links: the tips stopped bisecting the pins",
                );
            }
        }
    }

    /// The tooth-tip phase of one sprocket node of the SHIPPED glb, in the model's own side plane.
    ///
    /// A local glb walk rather than a reuse of `super::super::model`'s: that reader is a CONTRACT on
    /// the marker set (it aborts the process on a gap, and it measures rim radii, not phases), while
    /// this needs raw positions of one named node under its full transform chain. Both sprocket
    /// nodes are top-level in today's export, so "the node's world transform" and "the hull-local
    /// one" coincide — the hull origin IS the model origin (see `rig_geom`'s frame note).
    fn glb_sprocket_tip(node_name: &str, teeth: u32) -> f32 {
        use bevy::math::Mat4;

        let path = crate::assets::asset_root().join(crate::tank::TIGER_GLB_PATH);
        let gltf::Gltf { document, mut blob } =
            gltf::Gltf::open(&path).expect("the Tiger glb opens");
        let buffers = [blob.take().expect("the glb carries its binary chunk")];
        let scene = document.scenes().next().expect("the glb carries a scene");
        let mut stack: Vec<(gltf::Node, Mat4)> =
            scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
        while let Some((node, parent)) = stack.pop() {
            let local = match node.transform() {
                gltf::scene::Transform::Matrix { matrix } => Mat4::from_cols_array_2d(&matrix),
                gltf::scene::Transform::Decomposed {
                    translation,
                    rotation,
                    scale,
                } => Mat4::from_scale_rotation_translation(
                    Vec3::from(scale),
                    Quat::from_array(rotation),
                    Vec3::from(translation),
                ),
            };
            let world = parent * local;
            if node.name() == Some(node_name)
                && let Some(mesh) = node.mesh()
            {
                let axle = world.transform_point3(Vec3::ZERO);
                let mut polar = Vec::new();
                for primitive in mesh.primitives() {
                    let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                    for p in reader
                        .read_positions()
                        .expect("the sprocket carries positions")
                    {
                        let v = world.transform_point3(Vec3::from(p)) - axle;
                        polar.push((Vec2::new(v.z, v.y).length(), v.y.atan2(v.z)));
                    }
                }
                return measure_tooth_tip_angle(&polar, teeth)
                    .unwrap_or_else(|| panic!("{node_name} does not read as a tooth ring"));
            }
            for child in node.children() {
                stack.push((child, world));
            }
        }
        panic!("{node_name} is not in the shipped glb");
    }
}
