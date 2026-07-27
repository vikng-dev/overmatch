//! The sandbox's rig GEOMETRY CONTRACT — one derived description of a real tank's running gear,
//! replacing `mod.rs`'s hard-coded T-34 constants.
//!
//! Everything here comes from the sharp sources: the glb markers/rig meshes via [`super::marker_model`],
//! the universal laws via [`super::derive`], and the blueprint's own road-wheel nodes. Nothing is
//! authored twice. The sandbox spawns from it, the belt model wraps it, and the view draws it — so
//! "where the running gear is" has exactly one answer, in exactly one place.
//!
//! Frames (the thing to get right):
//!   * glTF/model space is `x = lateral, y = height, z = longitudinal`; side-plane points are
//!     `Vec2::new(z, y)` — the convention `track::route` already speaks.
//!   * the sandbox's HULL-LOCAL frame IS the glb model root frame (hull origin = model origin), so
//!     blueprint `root_position`s and [`DerivedModel`] centres are already hull-local. They are
//!     never re-offset here; the only vertical datum this module derives is [`RigGeom::hull_rest_y`],
//!     which says how high that origin floats above flat ground at rest.
//!   * the model is purely 2-D IN THAT SIDE PLANE. Circles, routes, poses, droop, perimeter and the
//!     link window are all functions of `(z, y)` alone — width never enters them, because a wider
//!     shoe does not change where the belt goes. Everything LATERAL goes through the measured shoe
//!     faces in exactly two places: the GRIP COLUMNS ([`RigGeom::grip_columns`]) and rendering
//!     ([`RigGeom::link_center_x`]; [`DerivedModel::width`] is the drawn link's SIZE). Keeping
//!     that seam sharp is what lets the whole rig be derived from a handful of side-plane numbers.
//!
//! The three CAST POSES ([`Pose`]) are the design's trapezoid: sprocket and idler are bolted to the
//! hull and never move, only the sprung road wheels swing, so the belt envelope pins at its ends and
//! breathes at the belly. That asymmetry is also why the link-count window ([`LinkWindow`]) exists —
//! see [`link_window`].

use std::path::Path;

use bevy::math::{Vec2, Vec3};
use bevy::prelude::Resource;

use crate::bake::TankBlueprint;
use crate::tank::TrackSide;
use crate::track::route::{build_route, external_tangent};
use crate::track::side::{PerSide, Side};

use super::derive::{self, SuspensionParams};
use super::marker_model::DerivedModel;

/// Load share of each EDGE grip column — the value that makes the edge pair reproduce a uniform
/// pressure strip's second moment exactly (see [`RigGeom::grip_columns`]). Width-independent.
pub(crate) const EDGE_COLUMN_SHARE: f32 = 1.0 / 6.0;
/// Load share of the CENTRE grip column: the remainder, so the three shares sum to exactly 1 and a
/// flat-ground total is identical to the single-column model's.
pub(crate) const CENTER_COLUMN_SHARE: f32 = 1.0 - 2.0 * EDGE_COLUMN_SHARE;

/// Which vertical offset the SPRUNG road wheels get before the belt envelope is built. Sprocket and
/// idler are hull-fixed in every pose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pose {
    /// The loaded rest pose Blender models — wheels where the tank's weight settles them.
    Rest,
    /// Fully extended: wheels lowered by the static deflection. The soft (droop) datum.
    Droop,
    /// Fully compressed: wheels raised by the bump-stop. The hard (bottoming) datum.
    Compression,
}

/// The droop stroke a rig can actually reach, split into what the SPRINGS want and what the CHAIN
/// allows. The spring wants the full static deflection; the inextensible loop may go taut first and
/// stop the belly short, so the pose the track can physically reach is the SMALLER of the two.
///
/// Both numbers are carried (not just the effective one) so the HUD can say *chain-limited* and show
/// how much droop the loop is eating. `spring` is `derive::static_deflection`; `effective` is what
/// [`RigGeom::circles`] lowers the belly by for [`Pose::Droop`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct DroopTravel {
    /// Spring static deflection `g/(2πf)²` (m) — the free-extension droop the springs alone reach.
    /// UNCLAMPED: this is what the loop is being measured against, not what it permits.
    pub spring: f32,
    /// The droop actually reachable (m) = `min(spring, chain-bind droop)`. Equals `spring` when the
    /// chain still has slack at full droop ([`DroopLimiter::Spring`]); strictly less when the chain
    /// goes taut mid-stroke ([`DroopLimiter::Chain`]); `0` when the loop is too short to reach even
    /// the loaded rest pose ([`DroopLimiter::Impossible`]).
    pub effective: f32,
}

impl DroopTravel {
    /// True when the inextensible loop bound the droop before the springs reached free extension —
    /// the belly is being held up by the track, not the springs.
    pub fn chain_limited(&self) -> bool {
        self.effective < self.spring
    }
}

/// One tank's derived running gear, built once at spawn and read by everything downstream.
#[derive(Resource, Clone)]
pub(crate) struct RigGeom {
    /// The marker-derived model this was assembled from (pitch, surface offsets, wheel/idler radii,
    /// centres). Always measured off the glb: a rig cannot exist on any other geometry, because a
    /// marker the export dropped aborts the build rather than substituting the RON's numbers.
    pub model: DerivedModel,
    /// Per-side REST-pose pin-line circles, front→rear: `[sprocket, road wheels…, idler]`.
    /// Hull-local side plane `(z, y)`; the radius is the PIN-LINE radius (what the route wraps),
    /// not the visible tread.
    pub rest: PerSide<Vec<(Vec2, f32)>>,
    /// Hull-local road-wheel centres per side, sorted front→rear (same order as the middle of
    /// [`Self::rest`]), full 3-D so wheel entities can be spawned straight off them. These keep the
    /// node's own lateral `x` — the wheel sits where the model says, not on the track's median plane.
    pub road_wheels: PerSide<Vec<Vec3>>,
    /// Height of the hull/model origin above flat ground at rest (m) = −(min y of the rest
    /// envelope). Spawn the body at this y and the rest track just kisses the ground.
    pub hull_rest_y: f32,
    /// Sprocket tooth count (authored) — sets the chord-exact pitch radius.
    pub teeth: u32,
    /// Links in the loop (authored/tuned) — with `pitch` this IS the material loop length.
    pub link_count: usize,
    /// Link pitch (m), measured off the `Pin_Start`/`Pin_End` markers.
    pub pitch: f32,
    /// Plate thickness (m) = `pin_to_inner + pin_to_outer`, measured, no mid-plate assumption.
    pub thickness: f32,
    /// The track's lateral median plane |x| (m); left sits at −`plane_x`, right at +`plane_x`.
    /// This is the PIN plane — where the 2-D route lives — not the shoe's centre.
    pub plane_x: f32,
    /// Where the authored `link_count` falls in the feasible window at build-time params — the
    /// answer to "can this loop even wrap this hull, and what stops the droop". Recomputed by
    /// [`Self::link_window`] when the suspension knobs move.
    pub window: LinkWindow,
}

impl RigGeom {
    /// Derive a side's whole running gear from the blueprint + its glb. `teeth` and `link_count` are
    /// the two authored counts the caller owns (the sandbox tunes `link_count` against
    /// [`LinkWindow`]); everything else is measured or derived. `params` is only needed for the
    /// build-time [`Self::window`] readout — the rest pose itself is knob-free by construction.
    pub(crate) fn build(
        blueprint: &TankBlueprint,
        glb_path: &Path,
        teeth: u32,
        link_count: usize,
        params: &SuspensionParams,
    ) -> Self {
        // The ONLY file read in this module. Everything after it is arithmetic on the numbers it
        // returned, which is exactly why [`Self::rebuild`] can skip it. Mass is the one number the
        // model takes from the blueprint (a readout input, not geometry); it is handed over as a
        // scalar so `model` has no route back to the RON's stale geometry fields. Aborts if the glb
        // cannot answer — see `DerivedModel::build`.
        let model = DerivedModel::build(blueprint.spec.mass, glb_path);
        let road_wheels = PerSide::new(
            side_wheel_centres(blueprint, Side::Left),
            side_wheel_centres(blueprint, Side::Right),
        );
        Self::assemble(model, road_wheels, teeth, link_count, params)
    }

    /// Re-assemble this rig at DIFFERENT authored counts, reusing the cached measurements.
    ///
    /// This is the sandbox's live `link_count` / `teeth` knob (see `mod.rs`'s `tune_rig_counts`).
    /// The glb is never reopened: [`DerivedModel`] and the blueprint's road-wheel centres are pure
    /// MEASUREMENTS of the model file — they cannot move when an authored count moves — so the
    /// keypress only has to redo the assembly (sprocket circle → rest circles → ride height →
    /// window). Parsing the Tiger glb per keystroke would make the knob unusable; caching the
    /// measurement and recomputing the assembly makes it free.
    ///
    /// `teeth` matters here as much as `link_count` does: it sets the chord-exact sprocket pitch
    /// radius, so it moves the front circle, every route around it, all three taut perimeters, and
    /// therefore the window verdict too.
    pub(crate) fn rebuild(&self, teeth: u32, link_count: usize, params: &SuspensionParams) -> Self {
        // `DerivedModel` is `Copy` (a flat bag of measurements); only the wheel-centre lists are
        // heap-backed, and two ~8-element `Vec<Vec3>`s per keypress is nothing next to a glb parse.
        Self::assemble(
            self.model,
            self.road_wheels.clone(),
            teeth,
            link_count,
            params,
        )
    }

    /// The assembly step both constructors share: measurements + the two authored counts in, whole
    /// geometry contract out. Pure — no file, no ECS.
    fn assemble(
        model: DerivedModel,
        road_wheels: PerSide<Vec<Vec3>>,
        teeth: u32,
        link_count: usize,
        params: &SuspensionParams,
    ) -> Self {
        let wheel_pin_r = derive::pin_line_radius(model.wheel_tread, model.pin_to_inner);
        let rest = PerSide::new(
            rest_circles(&model, road_wheels.get(Side::Left), wheel_pin_r, teeth),
            rest_circles(&model, road_wheels.get(Side::Right), wheel_pin_r, teeth),
        );

        // Ride height: what touches flat ground is the OUTER FACE of the rest belt — the pin
        // route's lowest point plus the measured `pin_to_outer` (never thickness/2: the pin does
        // not run mid-plate). Built taut (`belt_len = 0`) — slack only ever sags the TOP run,
        // never the belly, so the datum stays independent of the link count.
        let belly_y = build_route(rest.get(Side::Right), 0.0)
            .pts
            .iter()
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);

        Self {
            hull_rest_y: -belly_y + model.pin_to_outer,
            teeth,
            link_count,
            pitch: model.pitch,
            thickness: model.pin_to_inner + model.pin_to_outer,
            plane_x: model.plane_x,
            window: window_of(
                rest.get(Side::Right),
                wheel_pin_r,
                model.pitch,
                link_count,
                params,
            ),
            model,
            rest,
            road_wheels,
        }
    }

    /// The pin-line circles for one side at a cast pose, front→rear with the sprocket first and the
    /// idler last — the exact shape `track::route::build_route` expects. Droop lowers the sprung
    /// road wheels by the static deflection, compression raises them by the bump-stop; the hull-fixed
    /// sprocket and idler stay put.
    pub(crate) fn circles(
        &self,
        side: Side,
        pose: Pose,
        params: &SuspensionParams,
    ) -> Vec<(Vec2, f32)> {
        // Droop is the one pose the inextensible loop can restrain: the springs want the full static
        // deflection, but the chain may go taut first. Lower the belly by the CLAMPED travel so the
        // drawn (and downstream-consumed) pose is one the track can physically reach. Rest and
        // compression are hull-limited, not chain-limited, so they wrap straight through.
        if pose == Pose::Droop {
            let droop = self.droop_travel(params).effective;
            return droop_circles(self.rest.get(side), self.wheel_pin_radius(), droop);
        }
        pose_circles(self.rest.get(side), self.wheel_pin_radius(), pose, params)
    }

    /// How far the belly can actually droop at the given knobs: the spring static deflection, and the
    /// smaller CHAIN-CLAMPED value the inextensible loop permits (see [`DroopTravel`]).
    ///
    /// The clamp is exact and threshold-free: the taut droop-pose wrap grows monotonically with the
    /// droop `d` (more belly drop → longer belt path), so the largest reachable `d` is found by
    /// bisecting `d ∈ [0, spring]` for the point where the wrap fills the material loop `link_count ·
    /// pitch`. Measured on the right side — the sides are mirror images in the side plane and one loop
    /// length serves both, the same convention [`Self::link_window`] uses.
    pub(crate) fn droop_travel(&self, params: &SuspensionParams) -> DroopTravel {
        droop_travel(
            self.rest.get(Side::Right),
            self.wheel_pin_radius(),
            self.pitch,
            self.link_count,
            params,
        )
    }

    /// The material loop length (m) = `pitch × link_count`, exact. The loop is INEXTENSIBLE: this
    /// number never changes with pose, which is the whole premise of [`LinkWindow`].
    pub(crate) fn belt_len(&self) -> f32 {
        self.pitch * self.link_count as f32
    }

    /// Where arc length ZERO sits on the sprocket, as a side-plane angle (rad, `atan2(y, z)` about
    /// the sprocket centre) — i.e. **where `BeltPhase = 0` puts the first pin**.
    ///
    /// Both belt views parameterise the loop from the same vertex: `build_route` (and
    /// `track_sandbox::belt::conform_belts_field`, which repeats its construction) start the point
    /// list at the UPPER external tangent between the rear circle and the front one, then wrap
    /// forward around the sprocket's front. `resample(.., offset)` puts station 0 at exactly `offset`
    /// along that list and `phase_decompose` makes `offset` the belt phase mod one pitch — so at
    /// zero phase pin 0 sits precisely on that tangent point, where the return run lands back on the
    /// sprocket.
    ///
    /// That is the ONE fact `wheel_view`'s sprocket needs in order to be phase-locked rather than
    /// merely rate-locked, and it belongs here because it is a statement about the RIG's route
    /// parameterisation, not about rendering. It is also a constant of the rig: the tangent is taken
    /// between the two HULL-FIXED circles ([`Pose`] moves only the sprung road wheels), so no
    /// suspension travel can move it.
    pub(crate) fn belt_origin_angle(&self, side: Side) -> f32 {
        let circles = self.rest.get(side);
        let (sprocket_c, sprocket_r) = circles[0];
        let (idler_c, idler_r) = *circles
            .last()
            .expect("a side always has a sprocket and an idler");
        // Same call, same argument order, same sign as the route builder's: rear circle first, so
        // the returned pair is (idler tangent, sprocket tangent) and `+1` selects the upper line.
        let (_, sprocket_up) = external_tangent(idler_c, idler_r, sprocket_c, sprocket_r, 1.0);
        (sprocket_up - sprocket_c).to_angle()
    }

    /// Road-wheel PIN-LINE radius (m) = measured tread + the pin→inner offset.
    pub(crate) fn wheel_pin_radius(&self) -> f32 {
        derive::pin_line_radius(self.model.wheel_tread, self.model.pin_to_inner)
    }

    /// Road wheels on a side.
    pub(crate) fn wheel_count(&self, side: Side) -> usize {
        self.road_wheels.get(side).len()
    }

    /// Taut wrap length (m) around one side's circles at a pose — the perimeter a zero-slack loop
    /// would need. Built with `belt_len = 0` so the return run stays a straight chord (the editor's
    /// self-check precedent): this measures the HULL, not the belt.
    pub(crate) fn taut_perimeter(&self, side: Side, pose: Pose, params: &SuspensionParams) -> f32 {
        build_route(&self.circles(side, pose, params), 0.0).total()
    }

    /// The RED hard-stop wrap for a side, in the side plane `(z, y)` — the outermost surface the
    /// track can ever present toward the hull, and therefore the pure penetration BACKSTOP a
    /// collider is extruded from (see `mod.rs`'s `HardStop`). Everything softer than this is the
    /// support penalty's job; this is only the wall the suspension bottoms against.
    ///
    /// It is the taut wrap of the [`Pose::Compression`] circles (road wheels raised by
    /// `params.bump_stop`, sprocket and idler unsprung) DEFLATED to the track's INNER surface
    /// (every pin-line radius minus the measured `pin_to_inner` — the wheel tread the belt
    /// rides on). The rigid thing behind the belt is the WHEEL RIM, and everything between the
    /// rim and the ground — the full plate, `pin_to_inner + pin_to_outer` (~50 mm) — is track
    /// material the support penalty may dig into for support and grinding traction (owner
    /// verdict 2026-07-24: give the track its full thickness of bite). A rigid boundary at the
    /// belt face would mask the penalty entirely, and a pin-line stop would grant only the outer
    /// half. This matters most at the unsprung sprocket/idler arcs, where the envelope's travel profile
    /// is zero and the plate is the ONLY compliance. Cost: at most one full plate depth of
    /// visual crush at a bottoming impact. Built taut (`belt_len = 0`) like every perimeter.
    ///
    /// Pose-consistent with the ride height: the rest PIN wrap's belly is `pin_to_outer` above
    /// `-hull_rest_y`, this wrap's belly is a further `pin_to_inner` ABOVE the pose's pin wrap
    /// (deflating the radii pulls the belly up toward the wheels), and the compression pose
    /// raises it by up to `bump_stop` more — so on flat ground at rest the red belly clears
    /// the ground by the FULL PLATE plus the belly rise, and touches nothing (the invariant
    /// the collider MUST hold, or it would carry the tank instead of the belt).
    ///
    /// The taut wrap points come from `build_route`, but the returned polyline is their CONVEX HULL,
    /// and that is not a formality: in the compression pose the sprung wheels rise into the return
    /// run, where `build_route`'s belt model scallops the top over each raised wheel (its taut return
    /// run rides wheel tops — a belt fact, not a hard-stop fact). The outermost surface the track can
    /// occupy toward the hull is by definition the convex hull of those wheels, so hulling the wrap
    /// both matches the physical meaning and hands `Collider::convex_hull` a boundary that is already
    /// what it will compute — CONVEX and CLOSED (first == last), the two properties the collider and
    /// the tests rely on.
    pub(crate) fn hard_stop_polyline(&self, side: Side, params: &SuspensionParams) -> Vec<Vec2> {
        let inner: Vec<(Vec2, f32)> = self
            .circles(side, Pose::Compression, params)
            .into_iter()
            .map(|(c, r)| (c, r - self.model.pin_to_inner))
            .collect();
        convex_hull_ccw(build_route(&inner, 0.0).pts)
    }

    /// The three lateral GRIP COLUMNS on a side: hull-local `x` and load share, ordered
    /// INBOARD → CENTRE → OUTBOARD.
    ///
    /// This is one of the only two places width is allowed to enter (the other is
    /// [`Self::link_center_x`]), and it is built from the shoe's TRUE lateral faces
    /// ([`DerivedModel::lateral_min`]/[`DerivedModel::lateral_max`]) rather than from
    /// `plane_x ± width/2`. The difference is not cosmetic: the Tiger's shoe is authored ~17 mm
    /// outboard of the pin plane — flush with the wheel pack inboard, overhanging outboard — so the
    /// symmetric construction puts BOTH edge columns 17 mm off, over-reporting inboard contact and
    /// under-reporting the overhang that actually catches a rut lip.
    ///
    /// The weights are unchanged by that asymmetry, and it is worth saying why: they come from
    /// making the edge pair reproduce a laterally-uniform pressure strip's second moment exactly
    /// (`2·w_e·(w/2)² = w²/12` → `w_e = 1/6`, Simpson's weights), which is a statement about the
    /// strip's SHAPE, not about where it sits. Moving the whole strip outboard moves all three
    /// columns together and leaves the shares alone.
    pub(crate) fn grip_columns(&self, side: Side) -> [(f32, f32); 3] {
        // The sides are mirror images, so one lateral datum set (stored positive = right side)
        // serves both; the sign flip preserves the inboard→outboard ORDER on the left as well.
        let s = side.sign();
        [
            (s * self.model.lateral_min, EDGE_COLUMN_SHARE),
            (s * self.model.link_center_x, CENTER_COLUMN_SHARE),
            (s * self.model.lateral_max, EDGE_COLUMN_SHARE),
        ]
    }

    /// The same three columns as OFFSETS from this side's median plane (`±plane_x`) — the form
    /// [`crate::track::forces::ForceParams::columns`] takes, where the offset is applied along a
    /// single hull-frame lateral axis. Note the offsets are NOT `−w/2, 0, +w/2`: the middle one is
    /// the shoe's own outboard bias, which is exactly the fact the symmetric form threw away.
    pub(crate) fn grip_column_offsets(&self, side: Side) -> [(f32, f32); 3] {
        let median_x = side.plane_x(self.plane_x);
        self.grip_columns(side).map(|(x, w)| (x - median_x, w))
    }

    /// Just the lateral OFFSETS of [`Self::grip_column_offsets`] — what the wrap conform's and
    /// the wheel probes' terrain stations sample at (they carry no per-column weight; only the
    /// force law splits the coefficients across the columns).
    pub(crate) fn grip_stations(&self, side: Side) -> [f32; 3] {
        self.grip_column_offsets(side).map(|(x, _)| x)
    }

    /// Where to DRAW the link on a side: the shoe's own lateral centre, hull-local. Distinct from
    /// `side.plane_x(self.plane_x)`, which is where the pin line — and therefore the 2-D route the
    /// link rides — lives. Rendering the shoe on the pin plane is the ~17 mm error this exists to
    /// prevent.
    pub(crate) fn link_center_x(&self, side: Side) -> f32 {
        side.sign() * self.model.link_center_x
    }

    /// Where this rig's `link_count` sits in the feasible window at the given suspension knobs.
    /// Measured on the right side — the sides are mirror images in the side plane, and one loop
    /// length has to serve both.
    pub(crate) fn link_window(&self, params: &SuspensionParams) -> LinkWindow {
        window_of(
            self.rest.get(Side::Right),
            self.wheel_pin_radius(),
            self.pitch,
            self.link_count,
            params,
        )
    }

    /// What every wrap circle on a side DEMANDS of the link hinge, front→rear.
    ///
    /// Walks [`Self::rest`] rather than naming the wheels one by one, so a rig that grows a return
    /// roller or a second idler is covered the day the circle appears — the list IS the set of
    /// things the belt has to bend around. Pose-independent by construction: a cast pose moves
    /// wheel CENTRES, and a wrap angle is a function of the RADIUS alone.
    ///
    /// Test-only: this exists for the wrap-clearance asset guard
    /// (`the_authored_hinge_clears_every_wrap_the_running_gear_demands`), which is the thing that
    /// catches a silently re-exported smaller wheel. No runtime consumer.
    #[cfg(test)]
    pub(crate) fn wrap_demands(&self, side: Side) -> Vec<WrapDemand> {
        let circles = self.rest.get(side);
        let last = circles.len().saturating_sub(1);
        circles
            .iter()
            .enumerate()
            .map(|(i, &(_, pin_radius))| WrapDemand {
                role: match i {
                    0 => WrapRole::Sprocket,
                    i if i == last => WrapRole::Idler,
                    i => WrapRole::RoadWheel(i - 1),
                },
                pin_radius,
                joint_angle: derive::wrap_joint_angle(self.pitch, pin_radius),
            })
            .collect()
    }

    /// The TIGHTEST wrap on a side — the one circle the hinge limit actually has to clear.
    #[cfg(test)]
    pub(crate) fn worst_wrap_demand(&self, side: Side) -> WrapDemand {
        self.wrap_demands(side)
            .into_iter()
            .max_by(|a, b| a.joint_angle.total_cmp(&b.joint_angle))
            .expect("a side always has at least a sprocket and an idler")
    }
}

/// Which running-gear circle a [`WrapDemand`] came off — for the report, and so a failing
/// clearance assertion can name the part rather than an index.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WrapRole {
    Sprocket,
    /// Road-wheel station, front→rear from 0.
    RoadWheel(usize),
    Idler,
}

#[cfg(test)]
impl std::fmt::Display for WrapRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sprocket => write!(f, "sprocket"),
            Self::RoadWheel(i) => write!(f, "road wheel {i}"),
            Self::Idler => write!(f, "idler"),
        }
    }
}

/// How far ONE joint must fold to wrap a given circle. The demand side of the hinge budget; see
/// [`derive::wrap_joint_angle`] for the chord relation — the supply side is the authored
/// `track.link_angle.inward_deg`.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct WrapDemand {
    pub role: WrapRole,
    /// The PIN-LINE radius the belt's pins ride (not the visible tread).
    pub pin_radius: f32,
    /// Required hinge fold per joint (rad), always INWARD — every wrap curves toward the wheels.
    pub joint_angle: f32,
}

/// One side's cast-pose circles from its stored REST list: split off the hull-fixed sprocket (first)
/// and idler (last), move what's between them, reassemble. Free-standing so [`RigGeom::build`] can
/// classify the link window before the resource exists.
fn pose_circles(
    rest: &[(Vec2, f32)],
    wheel_pin_r: f32,
    pose: Pose,
    params: &SuspensionParams,
) -> Vec<(Vec2, f32)> {
    let sprocket = rest[0];
    let idler = *rest
        .last()
        .expect("a side always has a sprocket and an idler");
    let wheels: Vec<Vec2> = rest[1..rest.len() - 1].iter().map(|&(c, _)| c).collect();
    assemble_circles(sprocket, idler, &wheels, wheel_pin_r, pose, params)
}

/// Classify a link count against one side's three cast perimeters. Built taut (`belt_len = 0`) so
/// the return run stays a straight chord: this measures the HULL, not the belt.
fn window_of(
    rest: &[(Vec2, f32)],
    wheel_pin_r: f32,
    pitch: f32,
    link_count: usize,
    params: &SuspensionParams,
) -> LinkWindow {
    let wrap = |pose| build_route(&pose_circles(rest, wheel_pin_r, pose, params), 0.0).total();
    link_window(
        pitch,
        link_count,
        wrap(Pose::Compression),
        wrap(Pose::Rest),
        wrap(Pose::Droop),
    )
}

/// Hull-local centres of one side's road wheels, front→rear. The blueprint's `root_position` is
/// already model-root-relative — i.e. hull-local — so nothing is re-offset here.
fn side_wheel_centres(blueprint: &TankBlueprint, side: Side) -> Vec<Vec3> {
    let want = track_side(side);
    let mut centres: Vec<Vec3> = blueprint
        .geometry
        .roadwheels
        .iter()
        .filter(|(_, s)| *s == want)
        .map(|&(i, _)| blueprint.geometry.nodes[i].root_position)
        .collect();
    centres.sort_by(|a, b| a.z.total_cmp(&b.z));
    centres
}

/// The rig's [`Side`] as the tank bake's [`TrackSide`]. Two encodings of one fact — the tank rig
/// owns `TrackSide`, the track core owns `Side`; the track core's is the one to prefer, so the
/// conversion is confined to this seam.
fn track_side(side: Side) -> TrackSide {
    match side {
        Side::Left => TrackSide::Left,
        Side::Right => TrackSide::Right,
    }
}

/// Rest-pose pin-line circles for a side from the derived model + that side's wheel centres.
/// Sprocket radius is chord-exact from pitch × teeth; the idler and wheel radii are their measured
/// contact surfaces pushed out to the pin line.
fn rest_circles(
    model: &DerivedModel,
    wheels: &[Vec3],
    wheel_pin_r: f32,
    teeth: u32,
) -> Vec<(Vec2, f32)> {
    let sprocket = (
        model.sprocket_center,
        derive::sprocket_pitch_radius(model.pitch, teeth),
    );
    let idler = (
        model.idler_center,
        derive::pin_line_radius(model.idler_radius, model.pin_to_inner),
    );
    // Side plane (z, y): the wheel's lateral x is carried by `road_wheels`, not by the envelope.
    let side_plane: Vec<Vec2> = wheels.iter().map(|w| Vec2::new(w.z, w.y)).collect();
    assemble_circles(
        sprocket,
        idler,
        &side_plane,
        wheel_pin_r,
        Pose::Rest,
        &SuspensionParams::default(),
    )
}

/// Assemble the front→rear circle list, applying the cast-pose vertical offset to the SPRUNG road
/// wheels only. Pure: the ECS-facing builders feed it blueprint data, the tests feed it synthetic
/// gear. Wheels are re-sorted by z so the route builder's front→rear precondition holds no matter
/// what order they arrived in, with the sprocket pinned first and the idler last.
fn assemble_circles(
    sprocket: (Vec2, f32),
    idler: (Vec2, f32),
    wheels: &[Vec2],
    pin_r: f32,
    pose: Pose,
    params: &SuspensionParams,
) -> Vec<(Vec2, f32)> {
    let dy = match pose {
        Pose::Rest => 0.0,
        // The UNCLAMPED spring droop. This is the pose the link WINDOW is measured against (via
        // `window_of`); the chain clamp lives in `RigGeom::circles`, downstream of the window.
        Pose::Droop => -derive::static_deflection(params.ride_frequency),
        Pose::Compression => params.bump_stop,
    };
    assemble_circles_at(sprocket, idler, wheels, pin_r, dy)
}

/// The raw assembly: apply a vertical offset `dy` to the sprung wheels and re-sort front→rear, with
/// the hull-fixed sprocket pinned first and the idler last. `assemble_circles` (pose → dy) and
/// [`droop_circles`] (droop magnitude → dy) both route through here so there is one belt-envelope
/// construction, not three.
fn assemble_circles_at(
    sprocket: (Vec2, f32),
    idler: (Vec2, f32),
    wheels: &[Vec2],
    pin_r: f32,
    dy: f32,
) -> Vec<(Vec2, f32)> {
    let mut circles = vec![sprocket];
    let mut moved: Vec<Vec2> = wheels.iter().map(|w| Vec2::new(w.x, w.y + dy)).collect();
    moved.sort_by(|a, b| a.x.total_cmp(&b.x));
    circles.extend(moved.into_iter().map(|c| (c, pin_r)));
    circles.push(idler);
    circles
}

/// One side's droop-pose circles with the sprung wheels lowered by an arbitrary droop magnitude
/// `droop` (m, ≥ 0) — the same split as [`pose_circles`], but at a caller-chosen droop rather than the
/// fixed spring deflection. This is the parametric hook the chain clamp bisects over.
fn droop_circles(rest: &[(Vec2, f32)], wheel_pin_r: f32, droop: f32) -> Vec<(Vec2, f32)> {
    let sprocket = rest[0];
    let idler = *rest
        .last()
        .expect("a side always has a sprocket and an idler");
    let wheels: Vec<Vec2> = rest[1..rest.len() - 1].iter().map(|&(c, _)| c).collect();
    assemble_circles_at(sprocket, idler, &wheels, wheel_pin_r, -droop)
}

/// The reachable droop for a side: the spring static deflection, CLAMPED so the taut droop-pose wrap
/// never exceeds the material loop length `link_count · pitch`.
///
/// The taut wrap of the droop pose is monotone increasing in the droop `d` (dropping the belly only
/// lengthens the belt path), so:
///   * if the full spring droop already fits the loop, the chain never binds — return it unclamped;
///   * if even `d = 0` (the loaded rest pose) already overruns the loop, no droop is reachable — the
///     loop is too short (`DroopLimiter::Impossible`), clamp to `0`;
///   * otherwise bisect `d ∈ [0, spring]` for the droop whose taut wrap exactly fills the loop.
///
/// Wraps are built taut (`belt_len = 0`) like every other perimeter in this module — the return run
/// stays a straight chord, so this measures the HULL at that droop, not the belt's own slack.
fn droop_travel(
    rest: &[(Vec2, f32)],
    wheel_pin_r: f32,
    pitch: f32,
    link_count: usize,
    params: &SuspensionParams,
) -> DroopTravel {
    let spring = derive::static_deflection(params.ride_frequency);
    let belt_len = link_count as f32 * pitch;
    let wrap = |d: f32| build_route(&droop_circles(rest, wheel_pin_r, d), 0.0).total();

    if wrap(spring) <= belt_len {
        return DroopTravel {
            spring,
            effective: spring,
        };
    }
    if wrap(0.0) >= belt_len {
        return DroopTravel {
            spring,
            effective: 0.0,
        };
    }
    // Monotone bisection: `lo` always fits the loop, `hi` never does. ~48 halvings drives the
    // bracket well below f32 precision on a sub-metre stroke, deterministically.
    let (mut lo, mut hi) = (0.0_f32, spring);
    for _ in 0..48 {
        let mid = 0.5 * (lo + hi);
        if wrap(mid) <= belt_len {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    DroopTravel {
        spring,
        effective: lo,
    }
}

/// The CCW convex hull of a point set (Andrew's monotone chain), returned CLOSED (last == first).
/// Collinear vertices are dropped. Used to turn the taut compression wrap into the clean convex
/// hard-stop boundary (see [`RigGeom::hard_stop_polyline`]); a degenerate (< 3 unique points) input
/// is returned as-is, which only a broken rig could produce.
fn convex_hull_ccw(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    // Turn left (positive cross) to stay on the hull; pop right/collinear turns.
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let half = |iter: &mut dyn Iterator<Item = Vec2>, out: &mut Vec<Vec2>| {
        for p in iter {
            while out.len() >= 2 && cross(out[out.len() - 2], out[out.len() - 1], p) <= 0.0 {
                out.pop();
            }
            out.push(p);
        }
        out.pop(); // the shared endpoint reappears as the next chain's start
    };
    let mut hull = Vec::new();
    half(&mut pts.iter().copied(), &mut hull); // lower chain, left → right
    half(&mut pts.iter().rev().copied(), &mut hull); // upper chain, right → left
    if let Some(&first) = hull.first() {
        hull.push(first);
    }
    hull
}

// ---------------------------------------------------------------------------------------------
// The link-count / droop window
// ---------------------------------------------------------------------------------------------

/// What stops the suspension on the way DOWN.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DroopLimiter {
    /// The loop is too short to wrap even the most-compressed hull: no pose is reachable. An
    /// authoring bug, not a physical regime.
    Impossible,
    /// The chain goes taut part-way through the droop stroke and restrains the suspension. The
    /// track is the droop limiter — historically the normal case for a tensioned track.
    Chain,
    /// The springs reach full extension before the chain runs out; slack remains even at full
    /// droop, so the belt sags. The springs are the droop limiter.
    Spring,
}

/// The feasible link-count window for a rig, and which end the authored count sits against.
///
/// See [`link_window`] for the physics; the counts here are reports (what you'd author), while
/// [`Self::limiter`] is decided by the exact loop length, not by the rounded counts.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct LinkWindow {
    /// `ceil(P_min / pitch)` — the fewest links that can wrap the FULLY COMPRESSED hull.
    pub n_min: usize,
    /// `round(P_droop / pitch)` — links to exactly fill the FULLY DROOPED hull (zero slack there).
    pub n_droop: usize,
    /// The current authored/tuned link count.
    pub n: usize,
    /// Which regime `n` lands in.
    pub limiter: DroopLimiter,
    /// `belt_len − P_rest` (m): slack at the loaded rest pose. Negative means the rest pose itself
    /// is unreachable — the chain holds the wheels off their settled position.
    pub slack_rest: f32,
}

/// Classify a loop length against the poses it must wrap.
///
/// The loop is INEXTENSIBLE — its length is `L = n · pitch`, full stop. The taut perimeter `P(pose)`
/// is the wrap around the wheel-circle hull; since sprocket and idler are hull-fixed and only the
/// belly moves, `P` is smallest at full compression and largest at full spring droop.
///
/// The consequence that is easy to get backwards: `L < P(pose)` does NOT mean the links stretch, it
/// means **that pose is unreachable** — the chain physically restrains the suspension before it gets
/// there. So:
///
///   * `L < P_min` → [`DroopLimiter::Impossible`]: the loop cannot wrap even the most-compressed
///     hull, so there is no pose it fits. Author more links.
///   * `P_min ≤ L < P_droop` → [`DroopLimiter::Chain`]: reachable, and somewhere inside the travel
///     the chain goes taut and becomes the droop stop.
///   * `L ≥ P_droop` → [`DroopLimiter::Spring`]: the springs bottom out on droop first and slack is
///     still left over at full extension — the belt sags.
///
/// Perimeters come from the caller (`RigGeom::taut_perimeter`) so this stays pure `f32` math.
pub(crate) fn link_window(
    pitch: f32,
    n: usize,
    p_min: f32,
    p_rest: f32,
    p_droop: f32,
) -> LinkWindow {
    let pitch = pitch.max(1e-6);
    let belt_len = n as f32 * pitch;
    let limiter = if belt_len < p_min {
        DroopLimiter::Impossible
    } else if belt_len < p_droop {
        DroopLimiter::Chain
    } else {
        DroopLimiter::Spring
    };
    LinkWindow {
        n_min: (p_min / pitch).ceil().max(1.0) as usize,
        n_droop: (p_droop / pitch).round().max(1.0) as usize,
        n,
        limiter,
        slack_rest: belt_len - p_rest,
    }
}

/// The shipped Tiger spec, parsed straight from the RON that ships with the game — so the authored
/// hinge limits a test compares against are the ones the game loads, not a copy.
#[cfg(test)]
pub(crate) fn tiger_spec() -> crate::spec::TankSpec {
    ron::de::from_str(include_str!("../../assets/tiger_1/tiger_1.tank.ron"))
        .expect("tiger_1.tank.ron parses")
}

/// The REAL rig, built the way the sandbox builds it: the shipped glb + the shipped RON's two
/// authored counts. Nothing about it is synthetic, which is the point — this is what catches a
/// re-export that shrinks a wheel or turns the sprocket.
///
/// At module level rather than inside [`mod tests`] because [`crate::track_sandbox::wheel_view`]'s tooth-phase
/// calibration is asserted against the same shipped rig, and two independent "build the Tiger"
/// helpers would be two things to keep in step.
#[cfg(test)]
pub(crate) fn tiger_rig() -> RigGeom {
    let glb = crate::assets::asset_root().join(crate::tank::TIGER_GLB_PATH);
    let geometry = crate::bake::extract_tank_geometry(&glb).expect("the Tiger glb extracts");
    let spec = tiger_spec();
    let (teeth, link_count) = (spec.track.sprocket.teeth, spec.track.link_count);
    let blueprint = TankBlueprint {
        geometry: std::sync::Arc::new(geometry),
        spec: std::sync::Arc::new(spec),
    };
    RigGeom::build(
        &blueprint,
        &glb,
        teeth,
        link_count,
        &SuspensionParams::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Above the droop-filling count the SPRINGS reach free extension before the chain goes taut, so
    /// the clamp must be a no-op: `effective` is the full static deflection and nothing is chain-
    /// limited. Uses the shipped Tiger, so a re-export that tightened the hull would move `n_droop`
    /// and this would still test the right regime.
    #[test]
    fn the_chain_clamp_is_a_noop_when_the_springs_bind_first() {
        let p = SuspensionParams::default();
        let base = tiger_rig();
        let spring = derive::static_deflection(p.ride_frequency);
        let n_droop = base.link_window(&p).n_droop;

        for n in [n_droop, n_droop + 2] {
            let rig = base.rebuild(base.teeth, n, &p);
            assert_eq!(rig.link_window(&p).limiter, DroopLimiter::Spring);
            let dt = rig.droop_travel(&p);
            assert!((dt.spring - spring).abs() < 1e-6);
            assert!(
                (dt.effective - spring).abs() < 1e-4,
                "spring-limited n={n}: effective {} should equal the spring droop {spring}",
                dt.effective,
            );
            assert!(!dt.chain_limited());
        }
    }

    /// Inside the CHAIN regime (`n_min ≤ n < n_droop`) the inextensible loop goes taut before the
    /// springs bottom, so the reachable droop is strictly short of the spring deflection — and the
    /// defining fact of the clamp is that the taut wrap of the clamped droop pose exactly fills the
    /// material loop `link_count · pitch`. The shipped Tiger's window is one link wide, so its top
    /// (`n_droop − 1`) is the count to probe.
    #[test]
    fn the_chain_clamp_shortens_the_droop_and_pins_the_wrap_to_the_loop() {
        let p = SuspensionParams::default();
        let base = tiger_rig();
        let spring = derive::static_deflection(p.ride_frequency);
        let w = base.link_window(&p);

        let n = w.n_droop - 1;
        assert!(
            n >= w.n_min,
            "expected a chain-limited count to exist (n_min {} n_droop {})",
            w.n_min,
            w.n_droop,
        );
        let rig = base.rebuild(base.teeth, n, &p);
        assert_eq!(rig.link_window(&p).limiter, DroopLimiter::Chain);

        let dt = rig.droop_travel(&p);
        assert!((dt.spring - spring).abs() < 1e-6);
        // Strictly clamped, but the belly still reaches SOME droop at this count.
        assert!(dt.chain_limited());
        assert!(dt.effective < dt.spring);
        assert!(
            dt.effective > 0.0,
            "chain clamp should still leave reachable droop at n={n}, got {}",
            dt.effective,
        );

        // The chain is taut at the clamped pose: its taut wrap equals the loop length exactly.
        let wrap = rig.taut_perimeter(Side::Right, Pose::Droop, &p);
        assert!(
            (wrap - rig.belt_len()).abs() < 1e-4,
            "clamped droop wrap {wrap} must equal belt_len {}",
            rig.belt_len(),
        );

        // And the pose `RigGeom::circles` actually DRAWS is the clamped one: its belly sits above the
        // unclamped spring-droop belly the free `pose_circles` would produce.
        let belly = |circles: &[(Vec2, f32)]| {
            build_route(circles, 0.0)
                .pts
                .iter()
                .map(|q| q.y)
                .fold(f32::INFINITY, f32::min)
        };
        let clamped = rig.circles(Side::Right, Pose::Droop, &p);
        let spring_pose = pose_circles(
            rig.rest.get(Side::Right),
            rig.wheel_pin_radius(),
            Pose::Droop,
            &p,
        );
        assert!(
            belly(&clamped) > belly(&spring_pose),
            "clamped droop belly {} must sit above the spring-droop belly {}",
            belly(&clamped),
            belly(&spring_pose),
        );
    }

    /// The clamp is bounded on BOTH ends across every regime: never negative (a taut loop stops the
    /// belly, it does not pull the wheels up past rest) and never past the spring deflection. Swept
    /// from below the feasible window (Impossible) up through Spring; the Impossible floor is exactly
    /// zero, from the branch that fires when even the rest pose overruns the loop.
    #[test]
    fn the_clamped_droop_is_never_negative() {
        let p = SuspensionParams::default();
        let base = tiger_rig();
        let n_min = base.link_window(&p).n_min;

        for n in n_min.saturating_sub(3)..=n_min + 8 {
            let rig = base.rebuild(base.teeth, n, &p);
            let dt = rig.droop_travel(&p);
            assert!(
                dt.effective >= 0.0,
                "n={n}: effective droop {} went negative",
                dt.effective,
            );
            assert!(
                dt.effective <= dt.spring + 1e-6,
                "n={n}: effective {} must never exceed the spring droop {}",
                dt.effective,
                dt.spring,
            );
        }

        // Below the feasible window the loop cannot wrap even the rest pose, so the clamp bottoms at
        // exactly zero rather than going negative.
        let short = base.rebuild(base.teeth, n_min - 1, &p);
        assert_eq!(short.link_window(&p).limiter, DroopLimiter::Impossible);
        assert_eq!(short.droop_travel(&p).effective, 0.0);
    }

    /// Synthetic running gear with the road wheels clearly the lowest circles, so the belly (and
    /// with it the taut perimeter) tracks the sprung wheels.
    fn gear() -> ((Vec2, f32), (Vec2, f32), Vec<Vec2>) {
        let sprocket = (Vec2::new(-2.0, 0.6), 0.39);
        let idler = (Vec2::new(2.0, 0.6), 0.40);
        let wheels = vec![
            Vec2::new(-1.0, 0.4),
            Vec2::new(0.0, 0.4),
            Vec2::new(1.0, 0.4),
        ];
        (sprocket, idler, wheels)
    }

    /// A whole synthetic rig through the same [`RigGeom::assemble`] both constructors use — the glb
    /// read is the ONLY thing a test cannot do, and it is exactly the thing `rebuild` skips.
    fn synthetic_rig(teeth: u32, link_count: usize) -> RigGeom {
        let model = DerivedModel {
            pitch: 0.13,
            pin_to_inner: 0.025,
            pin_to_outer: 0.030,
            plane_x: 1.30,
            // An ASYMMETRIC shoe like the real one: 0.72 wide, centred 0.02 outboard of the pin
            // plane. Anything that reconstructs the edges as `plane_x ± width/2` gets caught.
            width: 0.72,
            lateral_min: 0.96,
            lateral_max: 1.68,
            link_center_x: 1.32,
            sprocket_center: Vec2::new(-2.0, 0.6),
            idler_center: Vec2::new(2.0, 0.6),
            idler_radius: 0.40,
            wheel_tread: 0.40,
            mass: 57_000.0,
        };
        // Side-plane `(z, y)` comes from the 3-D centres' z and y, so the wheels vary in z.
        let wheels: Vec<Vec3> = [-1.0, 0.0, 1.0]
            .into_iter()
            .map(|z| Vec3::new(1.30, 0.40, z))
            .collect();
        RigGeom::assemble(
            model,
            PerSide::new(wheels.clone(), wheels),
            teeth,
            link_count,
            &SuspensionParams::default(),
        )
    }

    /// The live-tuning contract behind the sandbox's `;` `'` / `n` `m` knobs: a retune must move
    /// exactly what the authored count owns and nothing else, and it must do it WITHOUT re-reading
    /// the model (the measurements simply ride along).
    #[test]
    fn rebuild_retunes_the_counts_and_carries_the_measurements() {
        let p = SuspensionParams::default();
        let base = synthetic_rig(20, 97);

        // Same counts in, same rig out — `rebuild` is not a second, drifting derivation.
        let same = base.rebuild(base.teeth, base.link_count, &p);
        assert_eq!(same.rest.get(Side::Right), base.rest.get(Side::Right));
        assert_eq!(same.hull_rest_y, base.hull_rest_y);
        assert_eq!(same.window, base.window);

        // MORE LINKS: the loop lengthens, the slack verdict follows, and the hull geometry — the
        // measured part — does not move at all. That asymmetry is the whole point: the link count
        // is the belt, not the running gear.
        let longer = base.rebuild(base.teeth, base.link_count + 3, &p);
        assert_eq!(longer.model.pitch, base.model.pitch);
        assert_eq!(longer.rest.get(Side::Right), base.rest.get(Side::Right));
        assert_eq!(longer.hull_rest_y, base.hull_rest_y);
        assert!((longer.belt_len() - base.belt_len() - 3.0 * base.pitch).abs() < 1e-4);
        assert_eq!(longer.window.n_min, base.window.n_min);
        assert_eq!(longer.window.n_droop, base.window.n_droop);
        assert!(longer.window.slack_rest > base.window.slack_rest);

        // MORE TEETH: the sprocket pitch circle grows, so the wrap grows, so the window itself
        // shifts — the tooth count is running gear, and it moves everything downstream of it.
        let toothier = base.rebuild(base.teeth + 6, base.link_count, &p);
        assert!(toothier.rest.get(Side::Right)[0].1 > base.rest.get(Side::Right)[0].1);
        assert!(
            toothier.taut_perimeter(Side::Right, Pose::Rest, &p)
                > base.taut_perimeter(Side::Right, Pose::Rest, &p)
        );
        assert!(toothier.window.n_min > base.window.n_min);
        // ...and the belt, which nobody re-authored, is now relatively shorter.
        assert!(toothier.window.slack_rest < base.window.slack_rest);
    }

    /// The wrap DEMAND list: one entry per circle the belt bends around, in route order, each
    /// carrying the joint angle that circle costs. The supply side is the AUTHORED
    /// `track.link_angle.inward`; this is the half a synthetic rig can pin, and
    /// [`the_authored_hinge_clears_every_wrap_the_running_gear_demands`] is where the two meet on
    /// the real model.
    #[test]
    fn wrap_demands_cover_every_circle_and_the_tightest_one_wins() {
        let rig = synthetic_rig(20, 97);
        let demands = rig.wrap_demands(Side::Right);
        // One per circle, in route order: sprocket, wheels front→rear, idler.
        assert_eq!(demands.len(), rig.rest.get(Side::Right).len());
        assert_eq!(demands[0].role, WrapRole::Sprocket);
        assert_eq!(demands.last().unwrap().role, WrapRole::Idler);
        assert_eq!(demands[1].role, WrapRole::RoadWheel(0));
        assert_eq!(demands[2].role, WrapRole::RoadWheel(1));

        // Sprocket, seen two ways: its pin circle is chord-exact from the tooth count, so the wrap
        // it demands is exactly one tooth pitch — τ/teeth — with the radius cancelling out.
        let per_tooth = std::f32::consts::TAU / rig.teeth as f32;
        assert!((demands[0].joint_angle - per_tooth).abs() < 1e-5);

        // The tightest wrap is the smallest circle. This is the relation the clearance test exists
        // to protect: shrink a wheel and the demand goes UP.
        let worst = rig.worst_wrap_demand(Side::Right);
        let smallest = demands
            .iter()
            .min_by(|a, b| a.pin_radius.total_cmp(&b.pin_radius))
            .unwrap();
        assert_eq!(worst.role, smallest.role);
        assert!(demands.iter().all(|d| d.joint_angle <= worst.joint_angle));

        // Fewer teeth = smaller sprocket = a tighter wrap, and the demand follows the geometry
        // rather than a stored number.
        let coarse = rig.rebuild(12, rig.link_count, &SuspensionParams::default());
        assert!(coarse.wrap_demands(Side::Right)[0].joint_angle > demands[0].joint_angle);
    }

    /// **The wrap-clearance guard.** Every circle the belt has to bend around demands a joint fold
    /// of `2·asin(pitch / 2r)` INWARD; the authored inward stop has to exceed the largest of them,
    /// with margin.
    ///
    /// This is the failure that is otherwise SILENT. Shrink the idler in Blender, re-export, and
    /// nothing complains — the route still draws, the links still place, and the belt simply
    /// cannot physically wrap the wheel it is drawn wrapping. So neither side of the comparison is
    /// hard-coded here: the DEMAND is derived from the shipped glb through [`RigGeom`], and the
    /// SUPPLY is read from the shipped RON. A changed asset or a re-authored limit is still
    /// covered; today's numbers are printed, not pinned.
    ///
    /// The supply is AUTHORED rather than measured off the shoe mesh — a hand measurement in
    /// Blender of the SHIPPED shoe, which makes this test a check on the real vehicle rather than on
    /// a parallel derivation. See `super::derive`'s module doc for why the mesh cannot supply it.
    #[test]
    fn the_authored_hinge_clears_every_wrap_the_running_gear_demands() {
        /// How much slack the tightest wrap must leave. Not a taste knob: a joint riding at its
        /// mechanical stop is a joint carrying wrap load through mesh-on-mesh contact, and the
        /// route is drawn on the pin line rather than on the tooth flanks, so the real geometry is
        /// always a little tighter than the ideal circle it is derived from.
        const MARGIN: f32 = 1.25;

        let rig = tiger_rig();
        // The AUTHORED supply — the RON's hand-measured inward stop, degrees on the RON side
        // (the authoring convention), converted here at the one place that reads it.
        let spec = tiger_spec();
        let supply = spec.track.link_angle.inward_deg.to_radians();

        println!(
            "\nwrap clearance — authored inward hinge limit {:.3}° (outward {:.3}°), pitch {:.5} m",
            supply.to_degrees(),
            spec.track.link_angle.outward_deg,
            rig.pitch,
        );
        let demands = rig.wrap_demands(Side::Right);
        assert!(
            demands.len() >= 3,
            "a side has a sprocket, road wheels and an idler"
        );
        for demand in &demands {
            println!(
                "  {:<14} pin radius {:.4} m   demands {:6.3}°   margin ×{:.2}",
                demand.role.to_string(),
                demand.pin_radius,
                demand.joint_angle.to_degrees(),
                supply / demand.joint_angle,
            );
            assert!(
                supply > demand.joint_angle * MARGIN,
                "the {} needs {:.2}° per joint but the shoe is only authored to fold {:.2}° \
                 inward (margin ×{MARGIN} required). Either a wheel shrank in the model or \
                 `track.link_angle.inward_deg` was re-authored downward.",
                demand.role,
                demand.joint_angle.to_degrees(),
                supply.to_degrees(),
            );
        }

        let worst = rig.worst_wrap_demand(Side::Right);
        assert_eq!(
            worst.joint_angle,
            demands
                .iter()
                .map(|d| d.joint_angle)
                .fold(f32::NEG_INFINITY, f32::max),
            "the worst demand must be the largest one"
        );
        // The SMALLEST circle is the tightest wrap — the relation that makes "shrinking a wheel is
        // dangerous" true, and the reason this test is worth having at all.
        let smallest = demands
            .iter()
            .min_by(|a, b| a.pin_radius.total_cmp(&b.pin_radius))
            .expect("non-empty");
        assert_eq!(worst.role, smallest.role);

        // The sprocket, seen two ways. Its pin circle is built chord-exact from the tooth count, so
        // the wrap it demands must come out at exactly one tooth pitch — `360/teeth` — with no
        // reference to the radius at all. Two derivations, one constraint.
        let sprocket = demands
            .iter()
            .find(|d| d.role == WrapRole::Sprocket)
            .expect("the sprocket is the first circle");
        let per_tooth = std::f32::consts::TAU / rig.teeth as f32;
        assert!(
            (sprocket.joint_angle - per_tooth).abs() < 1e-4,
            "the sprocket wrap {:.5} rad must equal one tooth pitch {per_tooth:.5} rad",
            sprocket.joint_angle,
        );
        assert!((sprocket.joint_angle.to_degrees() - 360.0 / rig.teeth as f32).abs() < 1e-2);
    }

    /// Width's ONLY structural job: placing the three grip columns. They must land on the shoe's
    /// true faces and its own centre — not on the pin plane ± half a width — and they must mirror.
    #[test]
    fn grip_columns_sit_on_the_true_shoe_faces_not_the_pin_plane() {
        let rig = synthetic_rig(20, 97);
        let m = &rig.model;
        let right = rig.grip_columns(Side::Right);
        assert_eq!(
            right.map(|(x, _)| x),
            [m.lateral_min, m.link_center_x, m.lateral_max]
        );
        // The symmetric construction would have put them here — 20 mm out on BOTH edges.
        assert!((right[0].0 - (rig.plane_x - m.width / 2.0)).abs() > 0.019);
        assert!((right[2].0 - (rig.plane_x + m.width / 2.0)).abs() > 0.019);
        // The span is still exactly the width, and the centre is still the mid of the faces.
        assert!((right[2].0 - right[0].0 - m.width).abs() < 1e-6);
        assert!(((right[0].0 + right[2].0) * 0.5 - right[1].0).abs() < 1e-6);

        // Left mirrors the right, keeping the inboard→outboard order (so index 0 is the inboard
        // edge on both sides — |x| grows across the array).
        let left = rig.grip_columns(Side::Left);
        assert_eq!(left.map(|(x, _)| x), right.map(|(x, _)| -x));
        assert!(left[0].0.abs() < left[2].0.abs());

        // Weights are a property of the strip's shape, so the asymmetry leaves them alone.
        let total: f32 = right.iter().map(|(_, w)| w).sum();
        assert!((total - 1.0).abs() < 1e-6);
        assert_eq!(right[0].1, right[2].1);

        // The offset form is the same columns relative to the side's median plane — and the middle
        // offset is the shoe's outboard bias, not zero.
        for side in [Side::Left, Side::Right] {
            let median_x = side.plane_x(rig.plane_x);
            let cols = rig.grip_columns(side);
            for (i, (offset, w)) in rig.grip_column_offsets(side).into_iter().enumerate() {
                assert!((median_x + offset - cols[i].0).abs() < 1e-6);
                assert_eq!(w, cols[i].1);
            }
        }
        assert!(rig.grip_column_offsets(Side::Right)[1].0 > 0.01);
        assert!(rig.grip_column_offsets(Side::Left)[1].0 < -0.01);

        // Rendering datum: the shoe's centre, NOT the pin plane the route lives in.
        assert_eq!(rig.link_center_x(Side::Right), m.link_center_x);
        assert!((rig.link_center_x(Side::Right) - rig.plane_x).abs() > 0.01);
    }

    /// Least signed distance from `p` into a CLOSED CCW convex polygon (last vertex == first): the
    /// min over edges of the left-normal projection. Positive = strictly inside, ~0 = on an edge,
    /// negative = outside by that much. The shared instrument for the convexity/enclosure tests.
    fn inset(poly: &[Vec2], p: Vec2) -> f32 {
        let ring = &poly[..poly.len() - 1];
        (0..ring.len())
            .map(|i| {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                let e = b - a;
                (b - a).perp_dot(p - a) / e.length().max(1e-9)
            })
            .fold(f32::INFINITY, f32::min)
    }

    /// The wrap must be CLOSED and CONVEX — the twin properties `Collider::convex_hull` relies on.
    /// Convexity is checked as the turn direction at every vertex, NORMALISED by the edge lengths so
    /// the threshold is an angle, not a raw cross product; the small negative tolerance is the
    /// arc-tessellation seam noise (`build_route` renders each wrap arc as 10 chords, so two nearly
    /// collinear segments can read a sub-degree right turn at a seam). CCW winding is confirmed by a
    /// positive signed area.
    #[test]
    fn the_hard_stop_wrap_is_convex_and_closed() {
        let p = SuspensionParams::default();
        let rig = tiger_rig();
        for side in [Side::Left, Side::Right] {
            let poly = rig.hard_stop_polyline(side, &p);
            assert!(poly.len() >= 4, "{side:?}: a wrap needs real extent");
            assert_eq!(
                poly.first(),
                poly.last(),
                "{side:?}: the wrap must close (first == last)"
            );

            let ring = &poly[..poly.len() - 1];
            let n = ring.len();
            let mut area = 0.0;
            for i in 0..n {
                let (a, b, c) = (ring[i], ring[(i + 1) % n], ring[(i + 2) % n]);
                area += a.perp_dot(b);
                let (e0, e1) = (b - a, c - b);
                // sin(turn): ≥ 0 for a left (convex) turn. Tolerance ≈ 1.7° of seam round-off.
                let sin_turn = e0.perp_dot(e1) / (e0.length() * e1.length()).max(1e-9);
                assert!(
                    sin_turn >= -0.03,
                    "{side:?}: non-convex turn at vertex {i} (sin {sin_turn})",
                );
            }
            assert!(area > 0.0, "{side:?}: the wrap must wind CCW (area {area})");
        }
    }

    /// The defining invariant of the red route — with the real limiter made explicit. Raising the
    /// sprung wheels by `bump_stop` lifts the belly, but only until the belly hands off to the
    /// UNSPRUNG idler/sprocket, which do not move; past that the belly is drive-wheel-limited and
    /// rises by LESS than the full bump-stop. So the clean "belly = rest pin wrap + bump_stop"
    /// identity is tested at a bump-stop small enough to stay road-wheel-limited, and the default
    /// (0.12 m, where the Tiger's idler is the limiter) is tested for the two properties that
    /// actually matter: the belly rise is CAPPED by the bump-stop, and the red belly still clears
    /// flat ground at rest — the stop lives on the track's INNER surface (wheel rim), so the
    /// full plate below it is the penalty's dig-in band.
    #[test]
    fn the_hard_stop_belly_is_bump_stop_of_travel_above_the_rest_inner_wrap() {
        let rig = tiger_rig();
        let belly = |poly: &[Vec2]| poly.iter().map(|q| q.y).fold(f32::INFINITY, f32::min);
        let pto = rig.model.pin_to_outer;
        for side in [Side::Left, Side::Right] {
            let rest = rig.rest.get(side);
            let last = rest.len() - 1;
            // Lowest sprung (road-wheel) vs lowest unsprung (sprocket/idler) pin bottoms: the head-
            // room the sprung belly has before the drive wheels take over as the limiter.
            let drive_bottom = (rest[0].0.y - rest[0].1).min(rest[last].0.y - rest[last].1);
            let road_bottom = rest[1..last]
                .iter()
                .map(|(c, r)| c.y - r)
                .fold(f32::INFINITY, f32::min);
            let headroom = drive_bottom - road_bottom;
            assert!(
                headroom > 0.0,
                "{side:?}: road wheels must be the rest belly"
            );

            // The rest PIN wrap belly IS that lowest road-wheel bottom; the ground face sits
            // `pin_to_outer` below it at the ride-height datum `-hull_rest_y`, and the INNER
            // (wheel-rim) wrap the stop rides sits `pin_to_inner` ABOVE it (smaller radii,
            // higher belly).
            let rest_pin = belly(&build_route(rest, 0.0).pts);
            assert!((rest_pin - pto + rig.hull_rest_y).abs() < 1e-4);
            let rest_inner = rest_pin + rig.model.pin_to_inner;

            // ROAD-WHEEL-LIMITED: a bump inside the headroom lifts the belly by EXACTLY that bump.
            let small = SuspensionParams {
                bump_stop: 0.5 * headroom,
                ..SuspensionParams::default()
            };
            let red_small = belly(&rig.hard_stop_polyline(side, &small));
            assert!(
                (red_small - (rest_inner + small.bump_stop)).abs() < 1e-4,
                "{side:?}: road-limited red belly {red_small} should be the rest inner wrap \
                 {rest_inner} + bump {}",
                small.bump_stop,
            );

            // DEFAULT (drive-wheel-limited on the Tiger): rise is positive but capped by the bump.
            let p = SuspensionParams::default();
            let red = belly(&rig.hard_stop_polyline(side, &p));
            let rise = red - rest_inner;
            assert!(
                rise > 0.0 && rise <= p.bump_stop + 1e-4,
                "{side:?}: default belly rise {rise} must be in (0, bump_stop {}]",
                p.bump_stop,
            );
            // Clears flat ground at rest by the FULL PLATE plus the rise — the collider
            // touches nothing until the belt bottoms AND crushes through the whole plate.
            assert!(
                red + rig.hull_rest_y > pto + rig.model.pin_to_inner,
                "{side:?}: red belly must clear the ground at rest by more than the plate \
                 (clearance {})",
                red + rig.hull_rest_y,
            );
        }
    }

    /// The enclosure guarantee: nothing the suspension bottoms onto crosses the wrap. Every
    /// compression INNER (wheel-rim) circle is inside-or-on the wrap to within the arc-chord
    /// sagitta — `build_route` renders each wrap arc as 10 chords that cut inside the true
    /// circle, so a smooth-circle sample bulges past the polyline by up to that sagitta on the
    /// arcs it defines (largest at the wide-sweep end circles). The collider re-hulls these
    /// same points, so its own boundary is convex regardless. (The stop deliberately lives on
    /// the track's INNER surface — the full plate below it is the support penalty's dig-in
    /// band, see [`RigGeom::hard_stop_polyline`] — so the PIN circles sit `pin_to_inner`
    /// OUTSIDE the wrap by design and are not tested for enclosure.)
    #[test]
    fn the_hard_stop_wrap_encloses_every_compression_circle() {
        let p = SuspensionParams::default();
        let rig = tiger_rig();
        let pti = rig.model.pin_to_inner;
        for side in [Side::Left, Side::Right] {
            let poly = rig.hard_stop_polyline(side, &p);
            let circles = rig.circles(side, Pose::Compression, &p);
            // Chord sagitta bound: a wrap arc sweeps at most ~1.25π (an end circle wrapping its
            // outer end) over `route::ARC_SEGMENTS` segments — the production constant, so the
            // bound cannot drift from the discretisation it is bounding.
            let max_r = circles
                .iter()
                .map(|&(_, r)| r - pti)
                .fold(0.0_f32, f32::max);
            let half_segment =
                1.25 * std::f32::consts::PI / (2.0 * crate::track::route::ARC_SEGMENTS as f32);
            let sagitta = max_r * (1.0 - half_segment.cos());
            for &(c, r) in &circles {
                for k in 0..96 {
                    let dir = Vec2::from_angle(std::f32::consts::TAU * k as f32 / 96.0);
                    let inset_d = inset(&poly, c + (r - pti) * dir);
                    assert!(
                        inset_d >= -sagitta - 1e-4,
                        "{side:?}: inner circle escaped past the arc sagitta (inset {inset_d}, \
                         sagitta {sagitta})",
                    );
                }
            }
        }
    }

    fn perimeter(pose: Pose, params: &SuspensionParams) -> f32 {
        let (sprocket, idler, wheels) = gear();
        let circles = assemble_circles(sprocket, idler, &wheels, 0.5, pose, params);
        let route = build_route(&circles, 0.0);
        assert!(
            route.pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
            "route must be finite"
        );
        route.total()
    }

    #[test]
    fn sprocket_stays_index_zero_and_the_loop_closes() {
        let (sprocket, idler, wheels) = gear();
        let circles = assemble_circles(
            sprocket,
            idler,
            &wheels,
            0.5,
            Pose::Rest,
            &SuspensionParams::default(),
        );
        assert_eq!(
            circles[0], sprocket,
            "route builder needs the sprocket at index 0"
        );
        assert_eq!(*circles.last().unwrap(), idler);
        let route = build_route(&circles, 12.6);
        assert_eq!(route.pts.first(), route.pts.last(), "the loop must close");
    }

    #[test]
    fn droop_drops_and_compression_lifts_the_belly() {
        let p = SuspensionParams::default();
        let belly = |pose| {
            let (sprocket, idler, wheels) = gear();
            let circles = assemble_circles(sprocket, idler, &wheels, 0.5, pose, &p);
            build_route(&circles, 0.0)
                .pts
                .iter()
                .map(|q| q.y)
                .fold(f32::INFINITY, f32::min)
        };
        let (rest, droop, comp) = (
            belly(Pose::Rest),
            belly(Pose::Droop),
            belly(Pose::Compression),
        );
        assert!(droop < rest, "droop {droop} should sit below rest {rest}");
        assert!(
            comp > rest,
            "compression {comp} should sit above rest {rest}"
        );
        // The droop stroke is exactly the static deflection the wheels dropped by.
        let expected = derive::static_deflection(p.ride_frequency);
        assert!((rest - droop - expected).abs() < 1e-4);
    }

    /// The premise the whole window rests on: only the belly moves, so the taut wrap is monotone in
    /// pose. If this ever flipped, `n_min`/`n_droop` would not be a window at all.
    #[test]
    fn taut_perimeter_is_monotone_in_pose() {
        let p = SuspensionParams::default();
        let (comp, rest, droop) = (
            perimeter(Pose::Compression, &p),
            perimeter(Pose::Rest, &p),
            perimeter(Pose::Droop, &p),
        );
        assert!(
            comp < rest,
            "compression {comp} must wrap tighter than rest {rest}"
        );
        assert!(
            droop > rest,
            "droop {droop} must wrap looser than rest {rest}"
        );
    }

    #[test]
    fn window_classifies_the_three_regimes() {
        let p = SuspensionParams::default();
        let (p_min, p_rest, p_droop) = (
            perimeter(Pose::Compression, &p),
            perimeter(Pose::Rest, &p),
            perimeter(Pose::Droop, &p),
        );
        let pitch = 0.13;
        let w = |n: usize| link_window(pitch, n, p_min, p_rest, p_droop);

        // Too few links to wrap even the tightest hull: no reachable pose.
        let short = w((p_min / pitch).floor() as usize - 1);
        assert_eq!(short.limiter, DroopLimiter::Impossible);
        // Inside the window: the chain goes taut mid-travel and limits the droop.
        let mid = w((0.5 * (p_min + p_droop) / pitch).round() as usize);
        assert_eq!(mid.limiter, DroopLimiter::Chain);
        // Long enough to wrap the fully-drooped hull: the springs stop it first, slack remains.
        let long = w((p_droop / pitch).ceil() as usize + 1);
        assert_eq!(long.limiter, DroopLimiter::Spring);

        // The reported counts bracket the window, and `n_min` is exactly the first feasible count.
        assert!(mid.n_min <= mid.n_droop);
        assert_eq!(w(mid.n_min).limiter, DroopLimiter::Chain);
        assert_eq!(w(mid.n_min - 1).limiter, DroopLimiter::Impossible);
        // Slack at rest is the loop's excess over the rest wrap — negative below it.
        assert!((mid.slack_rest - (mid.n as f32 * pitch - p_rest)).abs() < 1e-4);
        assert!(short.slack_rest < 0.0);
        assert!(long.slack_rest > 0.0);
    }
}
