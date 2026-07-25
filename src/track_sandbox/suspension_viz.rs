//! The sandbox's SUSPENSION OVERLAY — the suspension editor's derived-geometry gizmo layers, ported
//! onto a hull that **drives**.
//!
//! `bin/suspension_editor` proved the marker-driven track model by drawing it: the pin-line running
//! gear, the three cast poses (rest / max droop / max compression), the sprocket tooth ring, the
//! buoyant box. But it parked the tank at the model origin, so every gizmo it drew was a
//! model-space point pushed straight into world space — the identity transform hid the hardest part.
//! Here the hull is a dynamic body that translates, pitches and rolls, so the SAME geometry has to be
//! lifted twice:
//!
//!   1. side plane `(z, y)` -> hull-local `Vec3` at the track's lateral median plane (`plane_x`),
//!   2. hull-local -> world through the hull's live [`GlobalTransform`].
//!
//! Step 2 is the whole adaptation: [`world`] is the only place a gizmo point crosses into world
//! space, and every layer goes through it, so a drooping overlay stays welded to the hull it
//! describes instead of sliding off it. Circle and box layers additionally carry the hull's
//! ROTATION (a circle's plane is hull-local, not world-XY), which is why they take the isometry
//! rather than a bare translation.
//!
//! Everything drawn is DERIVED live from [`RigGeom`] + [`RigSuspension`] through
//! [`super::derive`]'s universal laws — nudge the ride frequency and the green droop envelope moves
//! this frame. That live re-derivation is what made the editor an *editor*; it survives the port.
//!
//! What this module is NOT: it never writes a transform, spawns a body, or touches physics — it
//! reads the rig contract and draws. Nor does it own any control input any more: which layers draw
//! (the [`SuspensionViz`] switches) and the suspension knobs ([`RigSuspension`]) are set by the egui
//! [`super::panel`] now, not by keys. This module is pure gizmo output over resources the panel
//! writes; the whole text readout it used to own (a status line + a paged detail panel) is gone,
//! folded into the panel's collapsing sections.

use bevy::prelude::*;

use super::derive;
use super::model4::BeltPhase;
use super::rig_geom::{Pose, RigGeom};
use super::wheel_view;
// `mod.rs`'s own sandbox state (a child module may read its parent's private items).
use super::{Hull, RigSuspension};
use crate::track::route::{build_route, resample};
use crate::track::side::Side;

pub(crate) fn plugin(app: &mut App) {
    app
        // `RigSuspension` is `mod.rs`'s (it feeds the rig build and the link-count clamp band too);
        // the panel writes it, this overlay only reads it. `SuspensionViz` is the panel's Layers
        // state; init it here so the draw system has it whether or not the panel is compiled in.
        .init_resource::<SuspensionViz>()
        // The rig is built at spawn (asset-gated), so the draw waits for it.
        .add_systems(Update, draw_suspension.run_if(resource_exists::<RigGeom>));
}

// ---------------------------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------------------------

/// How densely the grip-column sampler is drawn. A tap-loop rather than a bool because the density
/// genuinely matters: `Columns` is three clean lines you can read while driving, `Stations` adds the
/// ~6 probe stubs per link that show where the contact model actually casts — informative parked,
/// unreadable at speed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum GripDetail {
    Off,
    /// The three lateral column lines running the loop.
    #[default]
    Columns,
    /// ...plus each column's longitudinal collocation stations, as outward cast stubs.
    Stations,
}

impl GripDetail {
    /// Advance the tap-loop (the pen sandbox's `MeshState::next()` pattern). Used by the
    /// `dev_ui` panel's grip-detail cycle button and by the unit test; `allow(dead_code)` covers the
    /// default (no-`dev_ui`) build where only the always-compiled draw code references this type.
    #[allow(dead_code)]
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Off => Self::Columns,
            Self::Columns => Self::Stations,
            Self::Stations => Self::Off,
        }
    }

    /// The panel's label for this detail level. See [`Self::next`] for the `allow`.
    #[allow(dead_code)]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Columns => "columns",
            Self::Stations => "stations",
        }
    }
}

/// Which overlay layers draw, as the panel's Layers-section "Debug overlays" state. Bools where a
/// layer has exactly one useful reading; a tap-loop where it does not ([`GripDetail`]).
///
/// BOOT DEFAULTS are deliberately QUIET: the rest route and the wheel circles are the two layers that
/// say "here is the running gear the model derives", and everything else — the droop/compression
/// envelopes, the grip sampler, the tooth ring — is a diagnostic you ask for. Every layer is
/// independently toggleable in the panel.
///
/// `Copy + PartialEq` so the [`super::panel`] edits a LOCAL copy behind its checkboxes and writes
/// the resource back only on a real change (write-on-change discipline).
#[derive(Resource, Clone, Copy, PartialEq)]
pub(crate) struct SuspensionViz {
    /// The at-rest route: the taut wrap at the pose Blender authored (orange). ON at boot.
    pub rest_route: bool,
    /// The max-droop route: rest circles lowered by the static deflection (green).
    pub droop_route: bool,
    /// The max-compression route: rest circles raised by the bump stop (red).
    pub compression_route: bool,
    /// The 3-column contact sampler, drawn on the droop route.
    pub grip: GripDetail,
    /// The sprocket pitch circle + tooth ring with tooth-0 highlighted.
    pub sprocket: bool,
    /// The rest-pose pin-line circles of the whole running gear (cyan). ON at boot.
    pub wheels: bool,
}

impl Default for SuspensionViz {
    fn default() -> Self {
        // The curated boot view: the rest AND droop routes on (the suspension story), everything else
        // off — read through the x-ray hull the [`super::VizLayers`] default opens with.
        Self {
            rest_route: true,
            droop_route: true,
            compression_route: false,
            grip: GripDetail::Off,
            sprocket: false,
            wheels: false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The coordinate lift — the one place a side-plane point becomes a world point
// ---------------------------------------------------------------------------------------------

const ORANGE: Color = Color::srgb(1.0, 0.6, 0.1);
const GREEN: Color = Color::srgb(0.25, 0.9, 0.4);
const RED: Color = Color::srgb(0.95, 0.28, 0.22);
const CYAN: Color = Color::srgb(0.35, 0.8, 1.0);
const YELLOW: Color = Color::srgb(1.0, 0.9, 0.35);
const MAGENTA: Color = Color::srgb(1.0, 0.3, 0.85);
const WHITE: Color = Color::srgb(0.95, 0.96, 1.0);

/// Lift a side-plane point `(z, y)` on `side` into WORLD space through the hull's live pose.
///
/// The editor's version stopped at hull-local because its tank never moved; this one composes the
/// hull transform, and it is deliberately the ONLY path from side-plane to world in this module —
/// if a layer ever detaches from the hull, there is exactly one function to look at.
fn world(hull: &GlobalTransform, side: Side, p: Vec2, plane_x: f32) -> Vec3 {
    hull.transform_point(Vec3::new(side.sign() * plane_x, p.y, p.x))
}

/// Lift a hull-local point into world space (the lateral-offset layers build their own `x`).
fn world_local(hull: &GlobalTransform, p: Vec3) -> Vec3 {
    hull.transform_point(p)
}

/// The rotation that puts a gizmo circle in the hull's side plane (normal along the hull's local X),
/// composed with the hull's own rotation so the circle rolls and pitches with the tank.
fn side_plane_rotation(hull: &GlobalTransform) -> Quat {
    hull.rotation() * Quat::from_rotation_arc(Vec3::Z, Vec3::X)
}

// ---------------------------------------------------------------------------------------------
// The draw
// ---------------------------------------------------------------------------------------------

/// Draw every enabled layer, both sides, at the hull's current pose.
///
/// The three routes are rebuilt each frame rather than cached: the knobs move them, the whole point
/// of the overlay is that they move, and a ~100-point polyline per pose per side is nothing next to
/// the belt model already running in this sandbox.
fn draw_suspension(
    mut gizmos: Gizmos,
    geom: Res<RigGeom>,
    knobs: Res<RigSuspension>,
    viz: Res<SuspensionViz>,
    // The SAME accumulator the shoes and the running gear are placed from — the tooth ring is only
    // a check on the meshing if all three read one number (see [`draw_sprocket`]).
    phase: Res<BeltPhase>,
    hull: Single<&GlobalTransform, With<Hull>>,
) {
    let hull = hull.into_inner();
    let p = &knobs.0;
    let plane_x = geom.plane_x;
    let belt_len = geom.belt_len();

    for side in Side::ALL {
        // --- Rest-pose pin-line circles (the running gear the route wraps) ---
        if viz.wheels {
            let rot = side_plane_rotation(hull);
            for (c, r) in geom.circles(side, Pose::Rest, p) {
                gizmos.circle(Isometry3d::new(world(hull, side, c, plane_x), rot), r, CYAN);
            }
        }

        // --- At-rest route (orange): the taut wrap at the authored rest pose ---
        if viz.rest_route {
            let rest = build_route(&geom.circles(side, Pose::Rest, p), belt_len);
            draw_loop(&mut gizmos, hull, side, &rest.pts, plane_x, ORANGE);
        }

        // --- Max-droop route (green) — also the datum the grip columns and the box are read off ---
        let droop = build_route(&geom.circles(side, Pose::Droop, p), belt_len);
        if viz.droop_route {
            draw_loop(&mut gizmos, hull, side, &droop.pts, plane_x, GREEN);
        }
        if viz.grip != GripDetail::Off {
            draw_grip_columns(&mut gizmos, hull, side, &droop.pts, &geom, viz.grip);
        }
        // --- Max-compression / bump-stop route (red) ---
        if viz.compression_route {
            let comp = build_route(&geom.circles(side, Pose::Compression, p), belt_len);
            draw_loop(&mut gizmos, hull, side, &comp.pts, plane_x, RED);
        }

        // --- Sprocket tooth ring (live: it turns with the belt) ---
        if viz.sprocket {
            draw_sprocket(&mut gizmos, hull, side, &geom, phase.get(side), plane_x);
        }
    }
}

/// A closed side-plane polyline, lifted onto a side of the moving hull.
fn draw_loop(
    gizmos: &mut Gizmos,
    hull: &GlobalTransform,
    side: Side,
    pts: &[Vec2],
    plane_x: f32,
    color: Color,
) {
    gizmos.linestrip(pts.iter().map(|&p| world(hull, side, p, plane_x)), color);
}

/// The sprocket pitch circle + its live tooth ring: where a tooth TIP has to be, at this frame's
/// belt phase.
///
/// The ticks are TIPS, not gullets — that is what "teeth mesh BETWEEN pins" means, and it is why
/// they straddle the pitch circle the pin line rides rather than sitting on it. They used to be
/// drawn at `k·τ/teeth` from the hull's `+z`, which is neither: a ring bolted to the hull, at an
/// arbitrary phase, that did not turn when the track moved. An overlay whose job is to check a
/// meshing angle must not be half a tooth out and static while the thing it checks turns — so this
/// draws the RULE, live:
///
///   * white ticks at [`wheel_view::tooth_tip_angle`] + `k·τ/teeth` — the angles a tooth tip MUST
///     occupy for a pin to land in a gullet, derived from the belt alone (loop origin + phase),
///     with no reference to the sprocket mesh;
///   * the leading tooth in RED, so the ring reads as rotating and its direction is unambiguous;
///   * a yellow tick on the pitch circle at [`RigGeom::belt_origin_angle`] — where zero phase seats
///     pin 0, i.e. the datum the whole calibration hangs off.
///
/// That makes it a genuine A/B rather than a copy of the wheel layer: turn the running gear on
/// (`2`) and the glb's own teeth should sit ON the white ticks, and the drawn shoes' pin joints
/// halfway between them. If a tooth is off a tick, the mesh measurement is wrong; if a pin is off
/// the yellow tick at zero phase, the loop's origin is not where this thinks it is.
fn draw_sprocket(
    gizmos: &mut Gizmos,
    hull: &GlobalTransform,
    side: Side,
    geom: &RigGeom,
    travel: f64,
    plane_x: f32,
) {
    let center = geom.model.sprocket_center;
    let r = derive::sprocket_pitch_radius(geom.pitch, geom.teeth);
    gizmos.circle(
        Isometry3d::new(
            world(hull, side, center, plane_x),
            side_plane_rotation(hull),
        ),
        r,
        WHITE,
    );
    let mut tick = |angle: f32, inner: f32, outer: f32, color| {
        let dir = Vec2::from_angle(angle);
        gizmos.line(
            world(hull, side, center + dir * inner, plane_x),
            world(hull, side, center + dir * outer, plane_x),
            color,
        );
    };
    let origin = geom.belt_origin_angle(side);
    let lead = wheel_view::tooth_tip_angle(travel, geom.pitch, geom.teeth, origin);
    for t in 0..geom.teeth {
        let angle = lead + std::f32::consts::TAU * t as f32 / geom.teeth as f32;
        tick(angle, r - 0.05, r + 0.05, if t == 0 { RED } else { WHITE });
    }
    // Short, and INSIDE the pitch circle, so it cannot be mistaken for a tooth: it marks a pin seat.
    tick(origin, r - 0.06, r, YELLOW);
}

// --- Grip columns: the contact sampler's own geometry ------------------------------------------

/// Length (m) of a collocation-station cast stub — a readable stand-in for the probe reach, drawn
/// along the link's own outward normal because that is the direction the oracle is queried in.
const STATION_STUB: f32 = 0.06;

/// Draw the contact sampler as it is actually structured: `N` link stations along the loop, each
/// carrying 3 lateral columns, each column carrying 3 longitudinal collocation stations (start /
/// mid / end of its link).
///
/// It is drawn on the MAX-DROOP route on purpose — droop is the soft datum the support penalty is
/// measured against, so this is where the sampler lives when the suspension is doing its work.
///
/// Three details worth stating, because all three are easy to get backwards:
///   * the columns come from [`RigGeom::grip_columns`] — the shoe's TRUE lateral faces and its own
///     centre — not from `plane_x ± half·width`. The Tiger's shoe is authored ~17 mm outboard of the
///     pin plane, so the symmetric construction this used to build put BOTH edge lines 17 mm off:
///     it drew contact inboard where there is no shoe and missed the overhang that actually catches
///     a rut lip. This layer exists to make that overhang visible, so it must not erase it;
///   * the stations sit on the OUTER FACE, offset from the pin line by the measured `pin_to_outer`
///     — that is the surface the oracle is queried from (the sim's `forces.rs` still uses the
///     mid-plate `thickness/2`; the measured offset is the truthful one and is what we draw);
///   * "outward" is derived from the loop's own winding (signed area), not assumed, so the stubs
///     point away from the hull on both sides and in either route orientation.
fn draw_grip_columns(
    gizmos: &mut Gizmos,
    hull: &GlobalTransform,
    side: Side,
    pts: &[Vec2],
    geom: &RigGeom,
    detail: GripDetail,
) {
    // Link stations: the route resampled at the link pitch — the same walk the belt model does, so
    // the drawn stations are the stations.
    let mut stations = resample(pts, geom.pitch, 0.0);
    if stations.len() < 2 {
        return;
    }
    stations.push(stations[0]); // close the loop; the last segment carries the pitch residual
    let winding = loop_winding(pts);
    let face = geom.model.pin_to_outer;

    // Inboard → centre → outboard, already mirrored for this side and already carrying the shoe's
    // outboard bias: the hull-local `x` is the answer, not an offset to reconstruct.
    for (ci, (x, _share)) in geom.grip_columns(side).into_iter().enumerate() {
        // Centre column is the weighted majority of the load (2/3 vs 1/6 per edge) — draw it as the
        // dominant line, the edges as the rim pair.
        let color = if ci == 1 { YELLOW } else { MAGENTA };
        let lift = |p: Vec2, out: Vec2| {
            world_local(hull, Vec3::new(x, p.y + out.y * face, p.x + out.x * face))
        };

        // The column line: every link station pushed out onto the outer face at this offset.
        gizmos.linestrip(
            stations.windows(2).map(|w| {
                let out = outward_normal(w[0], w[1], winding);
                lift(w[0], out)
            }),
            color,
        );

        if detail != GripDetail::Stations {
            continue;
        }
        // The collocation stations: start / mid / end of each link, as outward cast stubs. The end
        // station is the next link's start, so drawing start+mid per link covers all three without
        // doubling every shared vertex.
        for w in stations.windows(2) {
            let out = outward_normal(w[0], w[1], winding);
            // A DIRECTION, so it takes the hull's rotation only — never its translation.
            let dir = hull.rotation() * Vec3::new(0.0, out.y, out.x);
            for p in [w[0], (w[0] + w[1]) * 0.5] {
                let base = lift(p, out);
                gizmos.line(base, base + dir * STATION_STUB, color);
            }
        }
    }
}

/// Sign of the loop's winding from its signed area: `+1` counter-clockwise in the side plane,
/// `-1` clockwise. Needed because "outward" is a property of the loop, not of the segment.
fn loop_winding(pts: &[Vec2]) -> f32 {
    let twice_area: f32 = pts
        .windows(2)
        .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
        .sum();
    if twice_area >= 0.0 { 1.0 } else { -1.0 }
}

/// Unit outward normal of the segment `a -> b` for a loop of the given winding. For a CCW loop the
/// outward normal is the right-hand normal `(t.y, -t.x)`; a CW loop flips it.
fn outward_normal(a: Vec2, b: Vec2, winding: f32) -> Vec2 {
    let t = (b - a).normalize_or_zero();
    Vec2::new(t.y, -t.x) * winding
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed CCW unit square in the side plane.
    fn square_ccw() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(0.0, 0.0),
        ]
    }

    /// The property every sampler layer rests on: the stubs must point AWAY from the loop, in either
    /// winding. Get this backwards and the grip columns draw inside the belt, where nothing is.
    #[test]
    fn outward_normal_points_out_of_the_loop_in_both_windings() {
        let ccw = square_ccw();
        let mut cw = ccw.clone();
        cw.reverse();
        for pts in [ccw, cw] {
            let winding = loop_winding(&pts);
            let centre = Vec2::new(0.5, 0.5);
            for w in pts.windows(2) {
                let out = outward_normal(w[0], w[1], winding);
                let mid = (w[0] + w[1]) * 0.5;
                assert!(
                    out.dot(mid - centre) > 0.0,
                    "normal {out} on segment {}->{} points inward",
                    w[0],
                    w[1],
                );
            }
        }
    }

    #[test]
    fn winding_sign_follows_the_traversal() {
        let ccw = square_ccw();
        let mut cw = ccw.clone();
        cw.reverse();
        assert_eq!(loop_winding(&ccw), 1.0);
        assert_eq!(loop_winding(&cw), -1.0);
    }

    /// The lift is what the whole port turns on: a side-plane point must land at the hull's pose,
    /// on the correct side, with `(z, y)` unswapped — and it must MOVE with the hull, which is the
    /// exact thing the editor's identity transform could never catch.
    #[test]
    fn the_lift_follows_the_hull_pose() {
        let plane_x = 1.2;
        let p = Vec2::new(3.0, 0.5); // (z, y)

        let identity = GlobalTransform::IDENTITY;
        let right = world(&identity, Side::Right, p, plane_x);
        assert_eq!(right, Vec3::new(1.2, 0.5, 3.0));
        let left = world(&identity, Side::Left, p, plane_x);
        assert_eq!(left, Vec3::new(-1.2, 0.5, 3.0));

        // Translated hull: the point translates with it.
        let moved = GlobalTransform::from(Transform::from_xyz(10.0, 2.0, -5.0));
        assert_eq!(
            world(&moved, Side::Right, p, plane_x),
            Vec3::new(11.2, 2.5, -2.0)
        );

        // Yawed 90 deg about Y: hull-local +z maps to world -x (right-handed), so the lifted point
        // must rotate too rather than staying on the world z axis.
        let yawed = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_y(
            std::f32::consts::FRAC_PI_2,
        )));
        let w = world(&yawed, Side::Right, p, plane_x);
        assert!(
            (w - Vec3::new(3.0, 0.5, -1.2)).length() < 1e-5,
            "yawed lift landed at {w}"
        );
    }

    /// The tap-loop is a loop: three taps come home. (The layer bools are their own inverses.)
    #[test]
    fn grip_detail_taps_around() {
        let start = GripDetail::Off;
        assert_eq!(start.next().next().next(), start);
        assert_eq!(GripDetail::default(), GripDetail::Columns);
    }
    /// The boot state is the curated "suspension story through a translucent tank" view: the rest AND
    /// droop routes on, every other gizmo layer off (they stay panel-toggleable). Read together with
    /// the [`super::super::VizLayers`] default (x-ray hull, running gear + shoes on, belt line off).
    #[test]
    fn boot_defaults_are_quiet() {
        let viz = SuspensionViz::default();
        assert!(viz.rest_route, "rest route on");
        assert!(viz.droop_route, "droop route on");
        assert!(!viz.compression_route && !viz.sprocket && !viz.wheels);
        assert_eq!(viz.grip, GripDetail::Off, "grip columns off");
    }
}
