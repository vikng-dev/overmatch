//! Synchronous running-gear construction shared by the game and track sandbox.
//!
//! `build` resolves the sheet's axle nodes and contact dimensions, reads the common shoe template
//! from the sim artifact, and refuses an infeasible rig before publication. `rebuild` reuses those
//! datums for sandbox count tuning. Every side keeps its own stations, pin plane and droop bound.
//!
//! Circles use the hull-local side plane `(z, y)` in front-to-rear order, with the sprocket first
//! and idler last. Their radii reach the pin line; tread radii and pin-to-face offsets are separate
//! physical facts. `hard_stop_polyline` encloses the compressed outer shoe surface for rigid bottoming.

use std::path::Path;

use bevy::math::{Affine3A, Dir3, Vec2, Vec3};
use bevy::prelude::Resource;

use crate::bake::TankBlueprint;
use crate::tank::TrackSide;
use crate::track::route::{build_route, external_tangent};
use crate::track::side::{PerSide, Side};

use super::derive::{self, SuspensionParams};
use super::marker_model::DerivedModel;

/// Width-independent edge load share; [`RigGeom::grip_columns`] derives the strip's second moment.
pub(crate) const EDGE_COLUMN_SHARE: f32 = 1.0 / 6.0;
/// Centre share: total load stays one, matching the single-column flat-ground model.
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

/// Spring-requested and chain-permitted droop. The inextensible loop may go taut before full
/// static deflection; the HUD carries both to show how much droop the chain removes.
/// [`RigGeom::circles`] lowers the belly by `effective` for [`Pose::Droop`].
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
    /// Shoe pitch and face datums measured from the sim glb; missing markers abort the build.
    /// Station construction and contact radii come from the blueprint, not this template.
    pub model: DerivedModel,
    /// Per-side REST-pose pin-line circles, front→rear: `[sprocket, road wheels…, idler]`.
    /// Hull-local side plane `(z, y)`; the radius is the PIN-LINE radius (what the route wraps),
    /// not the visible tread.
    pub rest: PerSide<Vec<(Vec2, f32)>>,
    /// Hull-local road-wheel centres, front→rear as in [`Self::rest`]. Full 3-D for wheel spawning:
    /// each retains its node's lateral `x`, rather than the track's median plane.
    pub road_wheels: PerSide<Vec<Vec3>>,
    /// Each declared station's tread, in the same front-to-rear order as `road_wheels`.
    pub wheel_treads: PerSide<Vec<crate::spec::WheelTread>>,
    /// Actual hull-local pin planes; the shared template may be placed asymmetrically.
    pub pin_planes: PerSide<f32>,
    /// Usable inward articulation including the authored connector/contact clearance (rad).
    inward_articulation: f32,
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
    /// Half the separation of the two authored pin planes (m), the drivetrain's steering lever.
    /// Actual contact and draw placements use `side_plane_x`.
    pub plane_x: f32,
    /// Build-time loop feasibility and droop limiter; [`Self::link_window`] recomputes it when
    /// suspension knobs move.
    #[cfg(any(feature = "dev_ui", test))]
    pub window: LinkWindow,
}

/// A measured running gear that the authored loop or suspension cannot support.
#[derive(Debug)]
pub(crate) enum RigFault {
    PinChord {
        side: Side,
        role: WrapRole,
        radius: f32,
        pitch: f32,
    },
    Articulation {
        side: Side,
        role: WrapRole,
        required: f32,
        available: f32,
    },
    NonfiniteGeometry {
        side: Side,
    },
    ZeroFootprint {
        side: Side,
    },
    TooFewTeeth(u32),
    LinkAddress(usize),
    ShortLoop {
        side: Side,
        links: usize,
        minimum: usize,
    },
    InsufficientTravel {
        side: Side,
        metres: f32,
    },
}

impl std::fmt::Display for RigFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PinChord {
                side,
                role,
                radius,
                pitch,
            } => write!(
                f,
                "track: {side:?} {role} pin radius {radius} m cannot carry a {pitch} m chord; correct its contact radius or the pin markers"
            ),
            Self::Articulation {
                side,
                role,
                required,
                available,
            } => write!(
                f,
                "track.link_angle.inward_deg: {side:?} {role} demands {} degrees but the authored usable articulation is {} degrees; correct the contact geometry or usable articulation",
                required.to_degrees(),
                available.to_degrees()
            ),
            Self::NonfiniteGeometry { side } => write!(
                f,
                "track: the {side:?} route is not finite; correct the axle transforms, contact radii and pin markers"
            ),
            Self::ZeroFootprint { side } => write!(
                f,
                "roadwheels: the {side:?} longitudinal centre span must be finite and positive; declare at least two longitudinally separated stations and correct their axle transforms"
            ),
            Self::TooFewTeeth(teeth) => write!(
                f,
                "track.sprocket.teeth: {teeth} cannot form a pin polygon; author at least three teeth"
            ),
            Self::LinkAddress(count) => write!(
                f,
                "track.link_count: {count} exceeds the belt table address; author at most {} links",
                super::belt_draw::MAX_BELT_LINKS
            ),
            Self::ShortLoop {
                side,
                links,
                minimum,
            } => write!(
                f,
                "track.link_count: {links} cannot wrap the {side:?} compressed running gear; author at least {minimum} links"
            ),
            Self::InsufficientTravel { side, metres } => write!(
                f,
                "track.link_count/track.suspension.ride_frequency: the {side:?} belt has {metres} m of droop, below the contact law's {} m domain; author a longer loop or a lower ride frequency",
                super::envelope::MIN_ENVELOPE_TRAVEL
            ),
        }
    }
}
impl std::error::Error for RigFault {}

/// Nominal weight over both tracks' footprint, kPa. Construction is admitted before spawn.
pub(crate) fn ground_pressure_kpa(mass_kg: f32, track_width_m: f32, contact_length_m: f32) -> f32 {
    assert!(
        mass_kg.is_finite()
            && mass_kg >= 0.0
            && track_width_m.is_finite()
            && track_width_m > 0.0
            && contact_length_m.is_finite()
            && contact_length_m > 0.0,
        "ground pressure requires finite nonnegative mass and positive footprint dimensions from admitted construction"
    );
    mass_kg * crate::track::derive::G / (2.0 * track_width_m * contact_length_m) / 1000.0
}

/// Nominal footprint uses shoe width and the mean span of both road-wheel centre sets.
/// The sim stores this pressure at spawn; view-only prints consume the same construction.
pub(crate) fn tank_ground_pressure_kpa(geom: &RigGeom) -> f32 {
    let length = 0.5
        * (road_wheel_span(geom.road_wheels.get(Side::Left))
            + road_wheel_span(geom.road_wheels.get(Side::Right)));
    ground_pressure_kpa(geom.model.mass, geom.model.width, length)
}

/// The centre of mass's height (m) over flat ground at rest: the `Center_Of_Mass` node over the
/// rest belt's outer face, which sits [`RigGeom::hull_rest_y`] below the model origin.
pub(crate) fn com_rest_height_m(geometry: &crate::bake::TankGeometry, geom: &RigGeom) -> f32 {
    let com = geometry
        .by_name
        .get("Center_Of_Mass")
        .expect("playable-sheet admission requires the Center_Of_Mass node");
    geom.hull_rest_y + geometry.nodes[*com].root_position.y
}

/// An empty or coincident station set has no longitudinal footprint and fails rig admission.
fn road_wheel_span(wheels: &[Vec3]) -> f32 {
    let (low, high) = wheels
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), wheel| {
            (low.min(wheel.z), high.max(wheel.z))
        });
    high - low
}

impl RigGeom {
    /// Derive a side's whole running gear from the blueprint + its glb. `teeth` and `link_count` are
    /// the two authored counts the caller owns (the sandbox tunes `link_count` against
    /// [`LinkWindow`]); everything else is measured or derived. `params` validates the authored
    /// loop and suspension on both sides and supplies the sandbox's cached window readout.
    /// The rest pose itself is knob-free by construction.
    pub(crate) fn build(
        blueprint: &TankBlueprint,
        glb_path: &Path,
        teeth: u32,
        link_count: usize,
        params: &SuspensionParams,
    ) -> Self {
        // The module's only file read; `rebuild` reuses its measurements. Mass supplies nominal
        // ground pressure as well as readouts; geometry still comes only from the sim artifact.
        let model = DerivedModel::build(blueprint.spec.mass, glb_path);
        let road_wheels = PerSide::new(
            side_wheel_centres(blueprint, Side::Left),
            side_wheel_centres(blueprint, Side::Right),
        );
        let wheel_treads = PerSide::new(
            side_wheel_treads(blueprint, Side::Left),
            side_wheel_treads(blueprint, Side::Right),
        );
        let fixed = Side::ALL.map(|side| {
            let spec = &blueprint.spec.track;
            let names = |nodes: &crate::spec::SideNodes| match side {
                Side::Left => nodes.left.clone(),
                Side::Right => nodes.right.clone(),
            };
            let centre = |name: String, field: &str| {
                let index = blueprint.geometry.nodes.iter().position(|node| node.name == name)
                    .unwrap_or_else(|| panic!("{}: {field} names missing node {name}; restore the axle node or update its construction reference", blueprint.file));
                let at = axle_position(blueprint, index);
                assert!(at.is_finite(), "{}: {field} axle must be finite; repair the node transform", blueprint.file);
                Vec2::new(at.z, at.y)
            };
            [
                (centre(names(&spec.sprocket.nodes), "track.sprocket.nodes"), derive::sprocket_pitch_radius(model.pitch, teeth)),
                (centre(names(&spec.idler.nodes), "track.idler.nodes"), derive::pin_line_radius(spec.idler.radius.get(side), model.pin_to_inner)),
            ]
        });
        let planes = blueprint.spec.track.pin_plane_x;
        let geom = Self::assemble(
            model,
            road_wheels,
            wheel_treads,
            PerSide(fixed),
            PerSide::new(planes.left, planes.right),
            blueprint.spec.track.link_angle.inward_deg.to_radians(),
            teeth,
            link_count,
            #[cfg(any(feature = "dev_ui", test))]
            params,
        );
        geom.validate(params)
            .unwrap_or_else(|error| panic!("{}: {error}", blueprint.file));
        blueprint
            .spec
            .track
            .transmission_params(&blueprint.spec.roadwheels, geom.wrap_radii(), geom.plane_x)
            .unwrap_or_else(|error| {
                panic!(
                    "{}: {error}; correct the running-gear construction facts",
                    blueprint.file
                )
            });
        geom
    }

    /// The radii the belt turns its wrapped wheels at: each side's rest sprocket and idler
    /// pin-line circles, the first and last of [`Self::rest`]. One sprocket radius serves both
    /// sides: the teeth and the measured pitch are common to both.
    pub(crate) fn wrap_radii(&self) -> crate::spec::WrapRadii {
        let idler = |side| {
            self.rest
                .get(side)
                .last()
                .expect("a side always has an idler")
                .1
        };
        crate::spec::WrapRadii {
            sprocket: self.rest.get(Side::Right)[0].1,
            idler: crate::spec::SideValues {
                left: idler(Side::Left),
                right: idler(Side::Right),
            },
        }
    }

    /// The bake builds this geometry before publishing a blueprint or any body. Live operator
    /// tuning uses the same typed refusal before replacing the accepted rig. Both sides must
    /// wrap and supply the contact law's minimum travel; a warning cannot make a fictitious rig.
    pub(crate) fn validate(&self, params: &SuspensionParams) -> Result<(), RigFault> {
        if self.link_count > super::belt_draw::MAX_BELT_LINKS {
            return Err(RigFault::LinkAddress(self.link_count));
        }
        if self.teeth < 3 {
            return Err(RigFault::TooFewTeeth(self.teeth));
        }
        for side in Side::ALL {
            let rest = self.rest.get(side);
            if !self.hull_rest_y.is_finite()
                || self
                    .road_wheels
                    .get(side)
                    .iter()
                    .any(|centre| !centre.is_finite())
                || rest.iter().any(|(centre, radius)| {
                    !centre.is_finite() || !radius.is_finite() || *radius <= 0.0
                })
            {
                return Err(RigFault::NonfiniteGeometry { side });
            }
            let span = road_wheel_span(self.road_wheels.get(side));
            if !span.is_finite() || span <= 0.0 {
                return Err(RigFault::ZeroFootprint { side });
            }
            for demand in self.wrap_demands(side) {
                if demand.pin_radius < self.pitch * 0.5 {
                    return Err(RigFault::PinChord {
                        side,
                        role: demand.role,
                        radius: demand.pin_radius,
                        pitch: self.pitch,
                    });
                }
                if demand.joint_angle > self.inward_articulation {
                    return Err(RigFault::Articulation {
                        side,
                        role: demand.role,
                        required: demand.joint_angle,
                        available: self.inward_articulation,
                    });
                }
            }
            if !build_route(rest, self.belt_len()).total().is_finite() {
                return Err(RigFault::NonfiniteGeometry { side });
            }
            let window = window_of(rest, self.pitch, self.link_count, params);
            if window.limiter == DroopLimiter::Impossible {
                return Err(RigFault::ShortLoop {
                    side,
                    links: self.link_count,
                    minimum: window.n_min,
                });
            }
            let travel = droop_travel(rest, self.pitch, self.link_count, params);
            if travel.effective < super::envelope::MIN_ENVELOPE_TRAVEL {
                return Err(RigFault::InsufficientTravel {
                    side,
                    metres: travel.effective,
                });
            }
        }
        Ok(())
    }

    /// DERIVED projection of the declared road-contact cylinders onto a hull-local direction.
    /// Station positions, tread radii and lateral intervals define this footprint; the inertia
    /// box describes mass distribution and supplies no contact extent.
    pub(crate) fn contact_extent(&self, axis: Dir3) -> (f32, f32) {
        let axis = *axis;
        let mut low = f32::INFINITY;
        let mut high = f32::NEG_INFINITY;
        for side in Side::ALL {
            for (centre, tread) in self
                .road_wheels
                .get(side)
                .iter()
                .zip(self.wheel_treads.get(side))
            {
                let radial = tread.radius * axis.y.hypot(axis.z);
                for &(start, end) in &tread.spans {
                    let (a, b) = (axis.x * start, axis.x * end);
                    low = low.min(centre.dot(axis) + a.min(b) - radial);
                    high = high.max(centre.dot(axis) + a.max(b) + radial);
                }
            }
        }
        assert!(
            low.is_finite() && high.is_finite() && low < high,
            "admitted contact cylinders have a finite positive footprint in every direction"
        );
        (low, high)
    }

    /// Sandbox `link_count` / `teeth` tuning (`tune_rig_counts`), without reopening the glb on each
    /// keypress. Cached shoe measurements and blueprint station centres cannot move with counts;
    /// only assembly repeats: sprocket circle → rest circles → ride height → window.
    /// `teeth` sets the chord-exact sprocket pitch radius, changing every route, all three taut
    /// perimeters and the window verdict.
    #[cfg(any(feature = "dev_ui", test))]
    pub(crate) fn rebuild(&self, teeth: u32, link_count: usize, params: &SuspensionParams) -> Self {
        // `DerivedModel` is `Copy`; clone centres and treads instead of reparsing the glb per keypress.
        // MEASURED seed sheet: two eight-centre lists.
        Self::assemble(
            self.model,
            self.road_wheels.clone(),
            self.wheel_treads.clone(),
            PerSide(Side::ALL.map(|side| {
                let rest = self.rest.get(side);
                let mut fixed = [rest[0], *rest.last().expect("a side has an idler")];
                fixed[0].1 = derive::sprocket_pitch_radius(self.pitch, teeth);
                fixed
            })),
            self.pin_planes,
            self.inward_articulation,
            teeth,
            link_count,
            #[cfg(any(feature = "dev_ui", test))]
            params,
        )
    }

    /// Shared pure assembly from measurements, construction and authored counts; no file or ECS.
    #[expect(
        clippy::too_many_arguments,
        reason = "the measured shoe and authored per-side station construction are independent inputs"
    )]
    fn assemble(
        model: DerivedModel,
        road_wheels: PerSide<Vec<Vec3>>,
        wheel_treads: PerSide<Vec<crate::spec::WheelTread>>,
        fixed: PerSide<[(Vec2, f32); 2]>,
        pin_planes: PerSide<f32>,
        inward_articulation: f32,
        teeth: u32,
        link_count: usize,
        #[cfg(any(feature = "dev_ui", test))] params: &SuspensionParams,
    ) -> Self {
        let rest = PerSide(Side::ALL.map(|side| {
            let [sprocket, idler] = *fixed.get(side);
            let mut circles = vec![sprocket];
            assert_eq!(
                road_wheels.get(side).len(),
                wheel_treads.get(side).len(),
                "every declared roadwheel has one tread"
            );
            circles.extend(
                road_wheels
                    .get(side)
                    .iter()
                    .zip(wheel_treads.get(side))
                    .map(|(pivot, tread)| {
                        (
                            Vec2::new(pivot.z, pivot.y),
                            derive::pin_line_radius(tread.radius, model.pin_to_inner),
                        )
                    }),
            );
            circles.push(idler);
            circles
        }));

        // Ride height: what touches flat ground is the OUTER FACE of the rest belt — the pin
        // route's lowest point plus the measured `pin_to_outer` (never thickness/2: the pin does
        // not run mid-plate). Built taut (`belt_len = 0`) — slack only ever sags the TOP run,
        // never the belly, so the datum stays independent of the link count.
        let belly_y = rest
            .values()
            .flat_map(|circles| build_route(circles, 0.0).pts)
            .map(|p| p.y)
            .fold(f32::INFINITY, f32::min);

        Self {
            hull_rest_y: -belly_y + model.pin_to_outer,
            teeth,
            link_count,
            pitch: model.pitch,
            thickness: model.pin_to_inner + model.pin_to_outer,
            plane_x: (pin_planes.0[1] - pin_planes.0[0]) * 0.5,
            pin_planes,
            inward_articulation,
            wheel_treads,
            #[cfg(any(feature = "dev_ui", test))]
            window: window_of(rest.get(Side::Right), model.pitch, link_count, params),
            model,
            rest,
            road_wheels,
        }
    }

    /// Pin-line circles for `track::route::build_route`, front→rear: sprocket, road wheels, idler.
    /// Droop lowers road wheels by chain-permitted travel; compression raises them by the bump-stop.
    /// Sprocket and idler stay hull-fixed.
    pub(crate) fn circles(
        &self,
        side: Side,
        pose: Pose,
        params: &SuspensionParams,
    ) -> Vec<(Vec2, f32)> {
        // Only droop is chain-limited; rest and compression use the hull-limited poses directly.
        if pose == Pose::Droop {
            let droop = self.side_droop_travel(side, params).effective;
            return droop_circles(self.rest.get(side), droop);
        }
        pose_circles(self.rest.get(side), pose, params)
    }

    /// Spring-requested and chain-permitted droop, using this side's authored stations and radii.
    /// [`droop_travel`] owns the monotone, threshold-free clamp derivation.
    pub(crate) fn side_droop_travel(&self, side: Side, params: &SuspensionParams) -> DroopTravel {
        droop_travel(self.rest.get(side), self.pitch, self.link_count, params)
    }

    /// Maximum reachable droop across both sides, the common calibration/readout scale.
    /// Each side's actual motion uses `side_droop_travel`.
    pub(crate) fn droop_travel(&self, params: &SuspensionParams) -> DroopTravel {
        Side::ALL
            .into_iter()
            .map(|side| self.side_droop_travel(side, params))
            .max_by(|a, b| a.effective.total_cmp(&b.effective))
            .expect("a rig has two sides")
    }

    pub(crate) fn side_plane_x(&self, side: Side) -> f32 {
        *self.pin_planes.get(side)
    }

    /// Inextensible material loop length (m), independent of pose; the premise of [`LinkWindow`].
    pub(crate) fn belt_len(&self) -> f32 {
        self.pitch * self.link_count as f32
    }

    /// First pin at `BeltPhase = 0`: side-plane angle (rad, `atan2(y, z)` about the sprocket centre).
    /// Both belt views start at the upper idler-to-sprocket tangent, where the return run lands,
    /// then wrap forward around the sprocket. `build_route` and
    /// `track_sandbox::belt::conform_belts_field` construct that same origin; `resample` places
    /// station 0 at its offset, and `phase_decompose` reduces phase modulo one pitch.
    /// `wheel_view` needs this rig parameterisation to phase-lock, rather than only rate-lock,
    /// the sprocket. Both tangent circles are hull-fixed, so suspension travel cannot move it.
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

    /// Maximum roadwheel pin-line radius (m), for conservative probe bounds and diagnostics.
    #[cfg(test)]
    pub(crate) fn wheel_pin_radius(&self) -> f32 {
        self.rest
            .values()
            .flat_map(|rest| &rest[1..rest.len() - 1])
            .map(|(_, radius)| *radius)
            .fold(0.0, f32::max)
    }

    #[cfg(feature = "dev_ui")]
    pub(crate) fn wheel_count(&self, side: Side) -> usize {
        self.road_wheels.get(side).len()
    }

    /// Taut pose perimeter (m), measuring the hull rather than belt slack; [`window_of`] owns
    /// the zero-length construction.
    #[cfg(any(feature = "dev_ui", test))]
    pub(crate) fn taut_perimeter(&self, side: Side, pose: Pose, params: &SuspensionParams) -> f32 {
        build_route(&self.circles(side, pose, params), 0.0).total()
    }

    /// Outer shoe face at full compression, in `(z, y)`, extruded by [`super::bump_stop`].
    /// Road wheels rise by `params.bump_stop`; hull-fixed end wheels have no travel and meet
    /// the world rigidly from first touch. Measured `pin_to_outer` carries the pin line to
    /// the only radial face the world can touch. The convex, closed CCW envelope bounds the
    /// compressed wheel circles within the route's arc-chord sagitta.
    pub(crate) fn hard_stop_polyline(&self, side: Side, params: &SuspensionParams) -> Vec<Vec2> {
        let outer: Vec<(Vec2, f32)> = self
            .circles(side, Pose::Compression, params)
            .into_iter()
            .map(|(c, r)| (c, r + self.model.pin_to_outer))
            .collect();
        convex_hull_ccw(build_route(&outer, 0.0).pts)
    }

    /// Hull-local `x` and load share, inboard → centre → outboard. Width enters only here and
    /// [`Self::link_center_x`], through the true [`DerivedModel::lateral_min`]/[`DerivedModel::lateral_max`]
    /// rather than `plane_x ± width/2`. MEASURED seed-shoe bias: ~17 mm outboard, flush with the wheel
    /// pack inboard and overhanging outboard. Symmetric placement shifts both edges by 17 mm,
    /// over-reporting inboard contact and under-reporting the overhang that catches rut lips.
    ///
    /// DERIVED Simpson shares reproduce a uniform strip's second moment:
    /// `2·w_e·(w/2)² = w²/12` → `w_e = 1/6`. Translation moves all columns without changing that
    /// shape or its shares.
    pub(crate) fn grip_columns(&self, side: Side) -> [(f32, f32); 3] {
        // The sheet places the common shoe template independently on each pin plane. Its
        // left-side mirror preserves inboard-to-outboard column order.
        let at = |x: f32| self.side_plane_x(side) + side.sign() * (x - self.model.plane_x);
        [
            (at(self.model.lateral_min), EDGE_COLUMN_SHARE),
            (at(self.model.link_center_x), CENTER_COLUMN_SHARE),
            (at(self.model.lateral_max), EDGE_COLUMN_SHARE),
        ]
    }

    /// Columns relative to this side's pin plane, applied along one hull-frame lateral axis by
    /// [`crate::track::forces::ForceParams::columns`]. Not `−w/2, 0, +w/2`: the centre retains shoe bias.
    pub(crate) fn grip_column_offsets(&self, side: Side) -> [(f32, f32); 3] {
        let median_x = self.side_plane_x(side);
        self.grip_columns(side).map(|(x, w)| (x - median_x, w))
    }

    /// Terrain-sampling offsets for wrap conform and wheel probes; only the force law uses weights.
    pub(crate) fn grip_stations(&self, side: Side) -> [f32; 3] {
        self.grip_column_offsets(side).map(|(x, _)| x)
    }

    /// Native shoe centre offset from its own pin plane, independent of track placement.
    /// The template binder and hull-local placements share this construction datum.
    pub(crate) fn shoe_center_offset(&self) -> f32 {
        self.model.link_center_x - self.model.plane_x
    }

    /// Hull-local shoe centre; `side_plane_x` places the pin line carrying its two-dimensional route.
    pub(crate) fn link_center_x(&self, side: Side) -> f32 {
        self.side_plane_x(side) + side.sign() * self.shoe_center_offset()
    }

    /// Where this rig's `link_count` sits in the feasible window at the given suspension knobs.
    /// Right-side diagnostic for the sandbox panel. Admission validates both sides independently.
    #[cfg(any(feature = "dev_ui", test))]
    pub(crate) fn link_window(&self, params: &SuspensionParams) -> LinkWindow {
        window_of(
            self.rest.get(Side::Right),
            self.pitch,
            self.link_count,
            params,
        )
    }

    /// Per-circle hinge demand, front→rear, compared directly with authored usable articulation.
    /// [`Self::rest`] also covers added return rollers/idlers without a separate wheel list.
    /// Pose changes centres, not radii, so demands are pose-independent.
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WrapRole {
    Sprocket,
    /// Road-wheel station, front→rear from 0.
    RoadWheel(usize),
    Idler,
}

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
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct WrapDemand {
    pub role: WrapRole,
    /// The PIN-LINE radius the belt's pins ride (not the visible tread).
    pub pin_radius: f32,
    /// Required hinge fold per joint (rad), always INWARD — every wrap curves toward the wheels.
    pub joint_angle: f32,
}

/// Cast-pose circles, moving only road wheels between hull-fixed sprocket and idler.
/// Free-standing so [`RigGeom::build`] can classify the window before the resource exists.
fn pose_circles(rest: &[(Vec2, f32)], pose: Pose, params: &SuspensionParams) -> Vec<(Vec2, f32)> {
    let dy = match pose {
        Pose::Rest => 0.0,
        Pose::Droop => -derive::static_deflection(params.ride_frequency),
        Pose::Compression => params.bump_stop,
    };
    let last = rest.len() - 1;
    rest.iter()
        .enumerate()
        .map(|(index, &(mut centre, radius))| {
            if index > 0 && index < last {
                centre.y += dy;
            }
            (centre, radius)
        })
        .collect()
}

/// Classify a link count against one side's three cast perimeters. Built taut (`belt_len = 0`) so
/// the return run stays a straight chord: this measures the HULL, not the belt.
fn window_of(
    rest: &[(Vec2, f32)],
    pitch: f32,
    link_count: usize,
    params: &SuspensionParams,
) -> LinkWindow {
    let wrap = |pose| build_route(&pose_circles(rest, pose, params), 0.0).total();
    link_window(
        pitch,
        link_count,
        wrap(Pose::Compression),
        wrap(Pose::Rest),
        wrap(Pose::Droop),
    )
}

/// Resolve and certify an axle's full authored frame before publishing any running gear.
fn axle_position(blueprint: &TankBlueprint, index: usize) -> Vec3 {
    let node = &blueprint.geometry.nodes[index];
    let mut chain = Vec::new();
    let mut cursor = node.parent;
    while let Some(parent) = cursor {
        chain.push(parent);
        cursor = blueprint.geometry.nodes[parent].parent;
    }
    let parent = chain
        .iter()
        .rev()
        .fold(Affine3A::IDENTITY, |frame, &index| {
            let ancestor = &blueprint.geometry.nodes[index];
            assert_eq!(
                ancestor.transform.scale.to_array().map(f32::to_bits),
                [1.0f32.to_bits(); 3],
                "{}: running-gear parent `{}` carries scale {:?}; apply ancestor scale before export so the rigid sim pose and wheel pose share one frame",
                blueprint.file, ancestor.name, ancestor.transform.scale,
            );
            frame * ancestor.transform.compute_affine()
        });
    super::gear_phase::GearFrame::new(node.transform, parent).unwrap_or_else(|fault| {
        panic!(
            "{}: running-gear node {}: {fault}",
            blueprint.file, node.name
        )
    });
    // The bake's rigid composition is the one axle position. The parent gate above establishes
    // its unit-scale invariant before the view's full affine frame can be used for posing.
    node.root_position
}

/// Hull-local centres of one side's road wheels, front→rear, composed through the full authored
/// parent chain; scaled construction groups refuse before running gear is published.
fn side_wheel_centres(blueprint: &TankBlueprint, side: Side) -> Vec<Vec3> {
    let want = track_side(side);
    let mut centres: Vec<Vec3> = blueprint
        .geometry
        .roadwheels
        .iter()
        .filter(|(_, s)| *s == want)
        .map(|&(i, _)| axle_position(blueprint, i))
        .collect();
    centres.sort_by(|a, b| a.z.total_cmp(&b.z));
    centres
}

/// Tread declarations sorted by the same axle Z order as `side_wheel_centres`.
fn side_wheel_treads(blueprint: &TankBlueprint, side: Side) -> Vec<crate::spec::WheelTread> {
    let mut stations: Vec<_> = blueprint
        .spec
        .roadwheels
        .iter()
        .filter(|wheel| wheel.side == track_side(side))
        .map(|wheel| {
            let index = *blueprint
                .geometry
                .by_name
                .get(&wheel.node)
                .expect("the bake resolves every declared roadwheel node");
            (axle_position(blueprint, index).z, wheel.tread.clone())
        })
        .collect();
    stations.sort_by(|a, b| a.0.total_cmp(&b.0));
    stations.into_iter().map(|(_, tread)| tread).collect()
}

fn track_side(side: Side) -> TrackSide {
    match side {
        Side::Left => TrackSide::Left,
        Side::Right => TrackSide::Right,
    }
}

/// [`pose_circles`]'s sprung-wheel split at caller-chosen `droop` (m, ≥ 0), for chain-clamp bisection.
fn droop_circles(rest: &[(Vec2, f32)], droop: f32) -> Vec<(Vec2, f32)> {
    let last = rest.len() - 1;
    rest.iter()
        .enumerate()
        .map(|(index, &(mut centre, radius))| {
            if index > 0 && index < last {
                centre.y -= droop;
            }
            (centre, radius)
        })
        .collect()
}

/// Chain-permitted droop, bounded by spring static deflection and the inextensible `link_count ·
/// pitch` loop. Taut wrap increases monotonically with droop: return full spring droop if it fits,
/// zero if even rest overruns the loop (including every [`DroopLimiter::Impossible`] count), else
/// bisect `[0, spring]` until the wrap fills the loop, without a physical threshold.
/// As in [`window_of`], `belt_len = 0` keeps the return chord straight, measuring hull, not slack.
fn droop_travel(
    rest: &[(Vec2, f32)],
    pitch: f32,
    link_count: usize,
    params: &SuspensionParams,
) -> DroopTravel {
    let spring = derive::static_deflection(params.ride_frequency);
    let belt_len = link_count as f32 * pitch;
    let wrap = |d: f32| build_route(&droop_circles(rest, d), 0.0).total();

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
    // Resolution: `lo` fits, `hi` does not; DERIVED 48 halvings put a sub-metre bracket below f32
    // precision, with deterministic iteration count.
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
    // Turn left (positive cross) to stay on the hull; pop right/collinear turns. Each chain starts
    // from an empty buffer: built into one shared buffer, the upper pass could pop back into the
    // lower chain and drop a lone rightmost apex.
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let chain = |iter: &mut dyn Iterator<Item = Vec2>| {
        let mut out: Vec<Vec2> = Vec::new();
        for p in iter {
            while out.len() >= 2 && cross(out[out.len() - 2], out[out.len() - 1], p) <= 0.0 {
                out.pop();
            }
            out.push(p);
        }
        out.pop(); // the shared endpoint reappears as the next chain's start
        out
    };
    let mut hull = chain(&mut pts.iter().copied()); // lower chain, left → right
    hull.extend(chain(&mut pts.iter().rev().copied())); // upper chain, right → left
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
    /// Springs reach full extension without chain restraint; excess loop length remains as sag.
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
    /// `round(P_droop / pitch)` — nearest link count to fill the fully drooped hull.
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
/// Inextensible `L = n · pitch` versus taut wheel-hull perimeter `P(pose)`: fixed sprocket/idler
/// and moving belly make `P` smallest at full compression, largest at full spring droop.
/// `L < P(pose)` makes that pose unreachable; links cannot stretch to reach it.
///
///   * `L < P_min` → [`DroopLimiter::Impossible`]: the loop cannot wrap even the most-compressed
///     hull, so there is no pose it fits. Author more links.
///   * `P_min ≤ L < P_droop` → [`DroopLimiter::Chain`]: reachable, and somewhere inside the travel
///     the chain goes taut and becomes the droop stop.
///   * `L ≥ P_droop` → [`DroopLimiter::Spring`]: full spring extension is reachable; any excess
///     loop length sags.
///
/// Perimeters come from the caller (`RigGeom::taut_perimeter`) so this stays pure `f32` math.
pub(crate) fn link_window(
    pitch: f32,
    n: usize,
    p_min: f32,
    p_rest: f32,
    p_droop: f32,
) -> LinkWindow {
    assert!(
        pitch.is_finite() && pitch > 0.0,
        "the marker gate certifies a finite positive link pitch"
    );
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
    crate::provenance::from_shipped_str(
        crate::tank::TANK_SPEC_PATH,
        include_str!("../../assets/tiger_1/tiger_1.tank.ron"),
    )
    .expect("tiger_1.tank.ron parses")
}

/// Shipped glb and authored counts, built as in the sandbox to catch wheel/sprocket re-export drift.
/// Shared with [`crate::track_sandbox::wheel_view`]'s tooth-phase calibration to keep one rig fixture.
#[cfg(test)]
pub(crate) fn tiger_rig() -> RigGeom {
    let glb = crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH);
    let spec = tiger_spec();
    let geometry = crate::bake::extract_tank_geometry(
        &glb,
        &spec,
        &crate::substances::SubstanceRegistry::shipped(),
    )
    .expect("the Tiger glb extracts");
    let (teeth, link_count) = (spec.track.sprocket.teeth, spec.track.link_count);
    let blueprint = TankBlueprint::new(crate::tank::TANK_SPEC_PATH.to_owned(), geometry, spec);
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

    /// Reflected inertia and drawn idler spin require rest sprocket/idler pin circles, including
    /// the idler's measured pin offset. Contact-only or roadwheel radii would give the wrong rate.
    #[test]
    fn wrap_radii_are_the_rest_sprocket_and_idler_pin_circles() {
        let rig = tiger_rig();
        let spec = tiger_spec();
        let wrap = rig.wrap_radii();
        for side in Side::ALL {
            let rest = rig.rest.get(side);
            assert_eq!(wrap.sprocket, rest[0].1);
            let idler = wrap.idler.get(side);
            assert_eq!(idler, rest.last().expect("a side always has an idler").1);
            assert_eq!(
                idler,
                derive::pin_line_radius(spec.track.idler.radius.get(side), rig.model.pin_to_inner)
            );
            assert!(idler > spec.track.idler.radius.get(side), "{side:?}");
        }
    }

    /// Running-gear admission is the gate that refuses a construction the joint drive cannot
    /// represent, before any `TrackGear` exists, naming the sheet: unequal side inertias panic in
    /// `RigGeom::build` with the file and the refusal. FIXTURE DATA: the shipped sheet with its
    /// left sprocket moment doubled. A mutant deleting the admission call builds the rig, and the
    /// sheet then reaches `init_track_gear`'s expect, which names no file.
    #[test]
    fn running_gear_admission_refuses_unequal_side_inertia_naming_the_sheet() {
        let glb = crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH);
        let mut spec = tiger_spec();
        spec.track.sprocket.spin_inertia_kgm2.0.left *= 2.0;
        let geometry = crate::bake::extract_tank_geometry(
            &glb,
            &spec,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .expect("the Tiger glb extracts");
        let (teeth, link_count) = (spec.track.sprocket.teeth, spec.track.link_count);
        let blueprint = TankBlueprint::new(crate::tank::TANK_SPEC_PATH.to_owned(), geometry, spec);
        let refusal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            RigGeom::build(
                &blueprint,
                &glb,
                teeth,
                link_count,
                &SuspensionParams::default(),
            )
        }))
        .err()
        .expect("unequal side inertias must be refused at running-gear admission");
        let message = refusal
            .downcast_ref::<String>()
            .expect("the refusal is a formatted message");
        assert!(
            message.contains(crate::tank::TANK_SPEC_PATH) && message.contains("differs by side"),
            "{message}"
        );
    }

    /// Startup collider construction must refuse a zero pressure denominator before any body
    /// spawns, with the sheet, field and repair. Tooling sheets may still parse with one wheel.
    /// Fixture: the shipped geometry with a single road station per side, or coincident stations.
    #[test]
    fn running_gear_admission_refuses_a_zero_footprint_naming_the_sheet() {
        let glb = crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH);
        for single_station in [true, false] {
            let mut spec = tiger_spec();
            if single_station {
                let first = Side::ALL.map(|side| {
                    spec.roadwheels
                        .iter()
                        .find(|wheel| wheel.side == track_side(side))
                        .unwrap()
                        .node
                        .clone()
                });
                spec.roadwheels.retain(|wheel| first.contains(&wheel.node));
            }
            spec.validate()
                .expect("structural sheets permit one wheel per side");
            let mut geometry = crate::bake::extract_tank_geometry(
                &glb,
                &spec,
                &crate::substances::SubstanceRegistry::shipped(),
            )
            .unwrap();
            if !single_station {
                for &(index, _) in &geometry.roadwheels {
                    geometry.nodes[index].root_position.z = 0.0;
                }
            }
            let blueprint =
                TankBlueprint::new("fixture/zero-footprint.tank.ron".into(), geometry, spec);
            let refusal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::super::bump_stop::TrackColliders::of_blueprint(&blueprint, &glb)
            }))
            .err()
            .expect("zero footprint must refuse at startup collider admission");
            let message = refusal.downcast_ref::<String>().unwrap();
            assert!(
                message.contains(&blueprint.file)
                    && message.contains("roadwheels")
                    && message.contains("separated stations"),
                "{message}"
            );
        }
    }

    /// Live count tuning uses the same address bound as file admission, before replacing a rig.
    #[test]
    fn live_rig_counts_cannot_exceed_the_table_address() {
        let mut rig = synthetic_rig(20, 97); // FIXTURE DATA: feasible construction.
        rig.link_count = super::super::belt_draw::MAX_BELT_LINKS + 1;
        assert!(matches!(
            rig.validate(&SuspensionParams::default()),
            Err(RigFault::LinkAddress(_))
        ));
    }

    /// The 2D belt route cannot certify a station's lateral position. Wheel posing and footprint
    /// projection rely on all three coordinates being finite before the rig is published.
    #[test]
    fn admission_refuses_nonfinite_lateral_station_positions() {
        let mut rig = synthetic_rig(20, 97); // FIXTURE DATA: a feasible synthetic running gear.
        rig.road_wheels.get_mut(Side::Left)[0].x = f32::NAN;
        assert!(matches!(
            rig.validate(&SuspensionParams::default()),
            Err(RigFault::NonfiniteGeometry { side: Side::Left })
        ));
    }

    /// Scripted braking relies on declared contact geometry, including asymmetry and tread gaps.
    /// FIXTURE DATA: two synthetic stations with unequal radii and lateral intervals.
    #[test]
    fn the_contact_footprint_projects_declared_cylinders() {
        let mut rig = tiger_rig();
        rig.road_wheels = PerSide::new(
            vec![Vec3::new(-1.0, 0.5, -2.0)],
            vec![Vec3::new(2.0, 0.6, 3.0)],
        );
        rig.wheel_treads = PerSide::new(
            vec![crate::spec::WheelTread {
                radius: 0.5,
                spans: vec![(-0.1, 0.2)],
            }],
            vec![crate::spec::WheelTread {
                radius: 1.0,
                spans: vec![(-0.2, -0.1), (0.3, 0.4)],
            }],
        );
        assert_eq!(rig.contact_extent(Dir3::Z), (-2.5, 4.0));
        assert_eq!(rig.contact_extent(Dir3::X), (-1.1, 2.4));
        assert_eq!(rig.contact_extent(Dir3::NEG_Z), (-4.0, 2.5));
    }

    /// Axles and collider/armour placement rely on the same baked rigid frame. A scaled parent
    /// must refuse instead of letting the affine wheel pose disagree with that position.
    #[test]
    fn axle_placement_uses_the_baked_position_and_refuses_scaled_parents() {
        let path = crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH);
        let spec = tiger_spec();
        let geometry = crate::bake::extract_tank_geometry(
            &path,
            &spec,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .unwrap();
        let axle = geometry.roadwheels[0].0; // Fixture: any declared station, no asset-specific bound.
        let parent = geometry.nodes[axle]
            .parent
            .expect("a station belongs to its authored hierarchy");
        let mut blueprint = TankBlueprint::new("fixture.tank.ron".into(), geometry, spec);
        assert_eq!(
            axle_position(&blueprint, axle),
            blueprint.geometry.nodes[axle].root_position
        );
        std::sync::Arc::get_mut(&mut blueprint.geometry)
            .unwrap()
            .nodes[parent]
            .transform
            .scale = Vec3::splat(2.0);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| axle_position(
                &blueprint, axle
            )))
            .is_err()
        );
    }

    /// Chain-limited droop is below spring deflection and its taut wrap fills `link_count · pitch`.
    /// Probe the highest Chain count (most remaining droop): `n_droop` rounds `P_droop / pitch`, so
    /// use it when rounded down, or `n_droop − 1` when rounded up.
    #[test]
    fn the_chain_clamp_shortens_the_droop_and_pins_the_wrap_to_the_loop() {
        let p = SuspensionParams {
            ride_frequency: 0.6,
            ..SuspensionParams::default()
        };
        let base = synthetic_rig(20, 97);
        let spring = derive::static_deflection(p.ride_frequency);
        let w = base.link_window(&p);

        let chain_limited = |n: usize| {
            base.rebuild(base.teeth, n, &p).link_window(&p).limiter == DroopLimiter::Chain
        };
        let n = if chain_limited(w.n_droop) {
            w.n_droop
        } else {
            w.n_droop - 1
        };
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

        // The drawn pose must use clamped travel, above the free spring-droop belly.
        let belly = |circles: &[(Vec2, f32)]| {
            build_route(circles, 0.0)
                .pts
                .iter()
                .map(|q| q.y)
                .fold(f32::INFINITY, f32::min)
        };
        let clamped = rig.circles(Side::Right, Pose::Droop, &p);
        let spring_pose = pose_circles(rig.rest.get(Side::Right), Pose::Droop, &p);
        assert!(
            belly(&clamped) > belly(&spring_pose),
            "clamped droop belly {} must sit above the spring-droop belly {}",
            belly(&clamped),
            belly(&spring_pose),
        );
    }

    /// The sim and view both rely on each station keeping its own radius through every pose.
    /// Fixture data deliberately uses unequal left/right geometry and nonmirrored track planes;
    /// copying one radius, one side, or a mirrored placement breaks a different assertion.
    #[test]
    fn station_contacts_and_placements_survive_all_poses() {
        let mut base = synthetic_rig(20, 110);
        base.wheel_treads.get_mut(Side::Left)[0].radius = 0.32;
        base.wheel_treads.get_mut(Side::Right)[0].radius = 0.47;
        base.pin_planes = PerSide::new(-1.8, 1.1);
        let params = SuspensionParams::default();
        let rig = base.rebuild(base.teeth, base.link_count, &params);
        assert_eq!(rig.side_plane_x(Side::Left), -1.8);
        assert_eq!(rig.side_plane_x(Side::Right), 1.1);
        assert_eq!(
            rig.shoe_center_offset(),
            base.shoe_center_offset(),
            "the same template keeps its local pin-to-centre datum when track planes move"
        );
        for pose in [Pose::Rest, Pose::Droop, Pose::Compression] {
            assert_eq!(
                rig.circles(Side::Left, pose, &params)[1].1,
                0.32 + rig.model.pin_to_inner
            );
            assert_eq!(
                rig.circles(Side::Right, pose, &params)[1].1,
                0.47 + rig.model.pin_to_inner
            );
        }
        for side in Side::ALL {
            let shift = rig.side_plane_x(side) - side.sign() * rig.model.plane_x;
            assert_eq!(
                rig.link_center_x(side),
                side.sign() * rig.model.link_center_x + shift
            );
        }
    }

    /// Across Impossible → Spring, droop stays in `[0, spring]`: the chain stops the belly without
    /// pulling wheels above rest. Impossible counts overrun even rest and yield exactly zero;
    /// Spring counts retain full static deflection without chain limitation.
    #[test]
    fn the_clamped_droop_is_never_negative() {
        let p = SuspensionParams::default();
        let base = synthetic_rig(20, 97);
        let n_min = base.link_window(&p).n_min;

        let mut spring_limited = 0;
        let spring_count =
            (base.taut_perimeter(Side::Right, Pose::Droop, &p) / base.pitch).ceil() as usize;
        for n in n_min.saturating_sub(1)..=spring_count + 1 {
            let rig = base.rebuild(base.teeth, n, &p);
            let dt = rig.droop_travel(&p);
            if rig.link_window(&p).limiter == DroopLimiter::Spring {
                spring_limited += 1;
                assert!(
                    (dt.effective - dt.spring).abs() < 1e-4,
                    "spring-limited n={n}: effective {} should equal the spring droop {}",
                    dt.effective,
                    dt.spring,
                );
                assert!(!dt.chain_limited(), "spring-limited n={n} is chain-limited");
            }
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

        assert!(spring_limited > 0, "the sweep must reach the Spring regime");

        // Below the feasible window the loop cannot wrap even the rest pose, so the clamp bottoms at
        // exactly zero rather than going negative.
        let short = base.rebuild(base.teeth, n_min - 1, &p);
        assert_eq!(short.link_window(&p).limiter, DroopLimiter::Impossible);
        assert_eq!(short.droop_travel(&p).effective, 0.0);
    }

    /// A rest circle list from synthetic gear, in the production order `pose_circles` reads:
    /// sprocket first, the wheels front to rear at one pin-line radius, idler last.
    fn rest_list(
        sprocket: (Vec2, f32),
        idler: (Vec2, f32),
        wheels: &[Vec2],
        pin_r: f32,
    ) -> Vec<(Vec2, f32)> {
        let mut rest = vec![sprocket];
        rest.extend(wheels.iter().map(|&wheel| (wheel, pin_r)));
        rest.push(idler);
        rest
    }

    /// Synthetic running gear with the road wheels clearly the lowest circles, so the belly
    /// and taut perimeter track the sprung wheels.
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

    /// Synthetic measurements and construction through production [`RigGeom::assemble`], without a glb read.
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
            PerSide::new(
                vec![
                    crate::spec::WheelTread {
                        radius: 0.4,
                        spans: vec![(-0.1, 0.1)]
                    };
                    3
                ],
                vec![
                    crate::spec::WheelTread {
                        radius: 0.4,
                        spans: vec![(-0.1, 0.1)]
                    };
                    3
                ],
            ),
            PerSide(
                [[
                    (
                        Vec2::new(-2.0, 0.6),
                        derive::sprocket_pitch_radius(model.pitch, teeth),
                    ),
                    (Vec2::new(2.0, 0.6), 0.4 + model.pin_to_inner),
                ]; 2],
            ),
            PerSide::new(-1.3, 1.3),
            40.0_f32.to_radians(),
            teeth,
            link_count,
            &SuspensionParams::default(),
        )
    }

    /// The game and sandbox must refuse a loop outside the measured rig's domain before any
    /// body uses it. FIXTURE DATA: synthetic geometry, with both a too-short loop and a spring
    /// requesting less than the solver's minimum travel. Mutants accepting either reach play
    /// with an invented rigid support pad.
    #[test]
    fn authored_loops_and_suspension_must_support_the_contact_law() {
        let valid = synthetic_rig(20, 100);
        assert!(valid.validate(&SuspensionParams::default()).is_ok());
        let short = synthetic_rig(20, 3);
        assert!(matches!(
            short.validate(&SuspensionParams::default()),
            Err(RigFault::ShortLoop { .. })
        ));
        let stiff = SuspensionParams {
            ride_frequency: 100.0,
            ..SuspensionParams::default()
        };
        assert!(matches!(
            valid.validate(&stiff),
            Err(RigFault::InsufficientTravel { .. })
        ));
    }

    /// Sandbox `;` `'` / `n` `m` retuning changes only what each count owns, preserving measurements
    /// without rereading the model.
    #[test]
    fn rebuild_retunes_the_counts_and_carries_the_measurements() {
        let p = SuspensionParams::default();
        let base = synthetic_rig(20, 97);

        // Same counts in, same rig out — `rebuild` is not a second, drifting derivation.
        let same = base.rebuild(base.teeth, base.link_count, &p);
        assert_eq!(same.rest.get(Side::Right), base.rest.get(Side::Right));
        assert_eq!(same.hull_rest_y, base.hull_rest_y);
        assert_eq!(same.window, base.window);

        // Link count owns belt length and slack, not measured hull geometry.
        let longer = base.rebuild(base.teeth, base.link_count + 3, &p);
        assert_eq!(longer.model.pitch, base.model.pitch);
        assert_eq!(longer.rest.get(Side::Right), base.rest.get(Side::Right));
        assert_eq!(longer.hull_rest_y, base.hull_rest_y);
        assert!((longer.belt_len() - base.belt_len() - 3.0 * base.pitch).abs() < 1e-4);
        assert_eq!(longer.window.n_min, base.window.n_min);
        assert_eq!(longer.window.n_droop, base.window.n_droop);
        assert!(longer.window.slack_rest > base.window.slack_rest);

        // Tooth count owns sprocket geometry, hence wrap length and the feasible window.
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

    /// Synthetic demand coverage and radius/angle relations; shipped authored supply meets demand
    /// in [`the_authored_hinge_clears_every_wrap_the_running_gear_demands`].
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

        // Smaller circles demand tighter folds: shrinking a wheel must increase demand.
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

    /// Usable articulation is the authored supply, including connector clearance. The asset
    /// gate must accept a joint slightly above its tightest demand and refuse one below it;
    /// adding a fixed multiplicative margin would reject the valid fixture.
    #[test]
    fn usable_articulation_is_compared_directly_with_each_wrap_demand() {
        let params = SuspensionParams::default();
        let mut rig = synthetic_rig(20, 110);
        let demand = rig.worst_wrap_demand(Side::Right).joint_angle;
        rig.inward_articulation = demand * 1.01; // DERIVED fixture: just above the required travel.
        rig.validate(&params)
            .expect("the usable articulation clears every demanded wrap");
        rig.inward_articulation = demand * 0.99; // DERIVED fixture: just below the required travel.
        assert!(matches!(
            rig.validate(&params),
            Err(RigFault::Articulation { .. })
        ));
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

    /// Dropping an extreme vertex dents the extruded hard stop by its arc sagitta. Fixture: convex
    /// pentagon with a lone rightmost apex and lower neighbour just left; sharing the lower chain's
    /// buffer lets the upper chain pop that apex.
    #[test]
    fn the_convex_hull_keeps_a_lone_extreme_apex() {
        let pts = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(0.9, 0.0),
            Vec2::new(0.89, 1.0),
            Vec2::new(1.0, 0.5),
        ];
        let hull = convex_hull_ccw(pts.clone());
        assert_eq!(
            hull.len(),
            pts.len() + 1,
            "every point is a vertex, closed: {hull:?}"
        );
        for p in pts {
            assert!(
                inset(&hull, p) >= -1e-6,
                "{p} lies outside the hull {hull:?}"
            );
        }
    }

    /// The hard-stop collider requires a closed convex wrap. Normalising turns by edge lengths
    /// makes tolerance angular; DERIVED: 10 chords per `build_route` arc can leave sub-degree right
    /// turns at nearly collinear seams. Positive signed area verifies CCW winding.
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

    /// The rigid stop follows the lowest compressed outer circle, whether a roadwheel or fixed
    /// gear limits it. A fixture requiring roadwheel limitation from every tank would reject
    /// valid drive-wheel-limited construction; the collider's consumers rely only on enclosure.
    #[test]
    fn the_hard_stop_belly_obeys_both_suspension_limiting_regimes() {
        let road_limited = synthetic_rig(20, 110);
        let mut fixed_limited = synthetic_rig(20, 110);
        for side in Side::ALL {
            let rest = fixed_limited.rest.get_mut(side);
            let last = rest.len() - 1;
            rest[0].0.y = -0.2; // Fixture fixed-gear axle height (m).
            rest[last].0.y = -0.2;
        }
        for rig in [&road_limited, &fixed_limited] {
            for side in Side::ALL {
                let rest = rig.rest.get(side);
                for bump in [0.01, 0.5] {
                    // Fixture compression strokes (m), across the handoff.
                    let params = SuspensionParams {
                        bump_stop: bump,
                        ..SuspensionParams::default()
                    };
                    let expected = rest
                        .iter()
                        .enumerate()
                        .map(|(index, (centre, radius))| {
                            let travel = if index > 0 && index + 1 < rest.len() {
                                bump
                            } else {
                                0.0
                            };
                            centre.y + travel - (radius + rig.model.pin_to_outer)
                        })
                        .fold(f32::INFINITY, f32::min);
                    // DERIVED worst chord sagitta: any wrap arc is at most a full revolution.
                    let max_radius = rest
                        .iter()
                        .map(|(_, radius)| radius + rig.model.pin_to_outer)
                        .fold(0.0, f32::max);
                    let half_segment =
                        std::f32::consts::PI / super::super::route::ARC_SEGMENTS as f32;
                    let sagitta = max_radius * (1.0 - half_segment.cos());
                    let actual = rig
                        .hard_stop_polyline(side, &params)
                        .iter()
                        .map(|point| point.y)
                        .fold(f32::INFINITY, f32::min);
                    assert!(
                        actual >= expected - 1e-5 && actual <= expected + sagitta + 1e-5,
                        "the compressed outer circle envelope bounds the stop independently of route sampling"
                    );
                }
            }
        }
    }

    /// Bottoming must enclose every compressed outer shoe circle within arc-chord sagitta.
    /// DERIVED: `build_route` uses 10 chords per arc, cutting inside smooth circles most at wide-sweep
    /// end arcs. The collider re-hulls these points, so its boundary is convex regardless.
    #[test]
    fn the_hard_stop_wrap_encloses_every_compression_circle() {
        let p = SuspensionParams::default();
        let rig = tiger_rig();
        let outer_offset = rig.model.pin_to_outer;
        for side in [Side::Left, Side::Right] {
            let poly = rig.hard_stop_polyline(side, &p);
            let circles = rig.circles(side, Pose::Compression, &p);
            // Chord sagitta bound: a wrap arc sweeps at most ~1.25π (an end circle wrapping its
            // outer end) over `route::ARC_SEGMENTS` segments — the production constant, so the
            // bound cannot drift from the discretisation it is bounding.
            let max_r = circles
                .iter()
                .map(|&(_, r)| r + outer_offset)
                .fold(0.0_f32, f32::max);
            let half_segment =
                1.25 * std::f32::consts::PI / (2.0 * crate::track::route::ARC_SEGMENTS as f32);
            let sagitta = max_r * (1.0 - half_segment.cos());
            for &(c, r) in &circles {
                for k in 0..96 {
                    let dir = Vec2::from_angle(std::f32::consts::TAU * k as f32 / 96.0);
                    let inset_d = inset(&poly, c + (r + outer_offset) * dir);
                    assert!(
                        inset_d >= -sagitta - 1e-4,
                        "{side:?}: outer circle escaped past the arc sagitta (inset {inset_d}, \
                         sagitta {sagitta})",
                    );
                }
            }
        }
    }

    fn perimeter(pose: Pose, params: &SuspensionParams) -> f32 {
        let (sprocket, idler, wheels) = gear();
        let circles = pose_circles(&rest_list(sprocket, idler, &wheels, 0.5), pose, params);
        let route = build_route(&circles, 0.0);
        assert!(
            route.pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
            "route must be finite"
        );
        route.total()
    }

    #[test]
    fn droop_drops_and_compression_lifts_the_belly() {
        let p = SuspensionParams::default();
        let belly = |pose| {
            let (sprocket, idler, wheels) = gear();
            let circles = pose_circles(&rest_list(sprocket, idler, &wheels, 0.5), pose, &p);
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
