//! Production hull driving against the map's admitted side bound, relied on by the server sim.

use super::*;
use crate::track::{
    rig_geom::{Pose, RigGeom},
    side::Side,
};
use avian3d::prelude::*;

struct Bed {
    app: BootedSim,
    tank: Entity,
    hulls: Vec<(Collider, ColliderTransform)>,
    droop_feet: Vec<Vec3>,
    half: f32,
    /// Where Avian holds a hull: the polygon edge less the walls' contact margin.
    face: f32,
    tolerance: f32,
    rest: f32,
}

impl Bed {
    fn new(ramp: bool) -> Self {
        // Fixture: a 256 m square; a 1:8 ramp returns to ground 8 m before the east wall.
        let manifest = crate::map::tests::fixture_manifest(128.0, 0.0, 50.0);
        let half = manifest.extent.half_extent();
        let size = 129;
        let heights = (0..size * size)
            .map(|i| {
                let x = -half + (i % size) as f32 * (2.0 * half / (size - 1) as f32);
                if ramp && x > half - 40.0 && x < half - 8.0 {
                    (x - (half - 40.0)) / 8.0
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        let grid = crate::terrain_grid::HeightGrid::new(heights.into(), size, manifest.extent);
        let mut app = booted_sim_on_map(Some(grid), Some(manifest));
        start_fixed_clock(&mut app);
        app.update();
        let world = app.world_mut();
        let tank = world
            .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
            .single(world)
            .unwrap();
        let hulls = world
            .query::<(&Collider, &ColliderTransform, &ColliderOf, &CollisionLayers)>()
            .iter(world)
            .filter(|(_, _, of, layers)| {
                of.body == tank
                    && layers.memberships.has_all(crate::Layer::Vehicle)
                    && layers.filters.has_all(crate::Layer::Terrain)
            })
            .map(|(shape, local, _, _)| (shape.clone(), *local))
            .collect::<Vec<_>>();
        assert!(
            !hulls.is_empty(),
            "the witness must observe the production hull proxies"
        );
        let geom = world.resource::<RigGeom>();
        let params = world
            .resource::<TankBlueprint>()
            .spec
            .track
            .suspension
            .params();
        let rest = geom.hull_rest_y;
        // DERIVED: each roadwheel's lowest reachable outer shoe point at full chain-limited droop.
        // If an end rises past this reach on flat ground, it has reared or climbed off support.
        let droop_feet = Side::ALL
            .into_iter()
            .flat_map(|side| {
                geom.circles(side, Pose::Droop, &params)
                    .into_iter()
                    .skip(1)
                    .take(geom.road_wheels.get(side).len())
                    .map(move |(c, r)| {
                        Vec3::new(
                            geom.side_plane_x(side),
                            c.y - r - geom.model.pin_to_outer,
                            c.x,
                        )
                    })
            })
            .collect();
        let margin = world
            .query_filtered::<Option<&CollisionMargin>, With<crate::world::BoundaryWall>>()
            .iter(world)
            .map(|margin| margin.map_or(0.0, |margin| margin.0))
            .fold(0.0_f32, f32::max);
        let tolerance = world.resource::<NarrowPhaseConfig>().contact_tolerance
            * world.resource::<PhysicsLengthUnit>().0;
        Self {
            app,
            tank,
            hulls,
            droop_feet,
            half,
            face: half - margin,
            tolerance,
            rest,
        }
    }

    fn pose(&self) -> (Vec3, Quat) {
        let world = self.app.world();
        (
            world.get::<Position>(self.tank).unwrap().0,
            world.get::<Rotation>(self.tank).unwrap().0,
        )
    }

    fn side_overrun(&self) -> f32 {
        let (p, r) = self.pose();
        self.hulls
            .iter()
            .map(|(shape, local)| {
                let aabb = shape.aabb(p + r * local.translation, r * local.rotation.0);
                aabb.min.xz().abs().max(aabb.max.xz().abs()).max_element() - self.face
            })
            .fold(f32::NEG_INFINITY, f32::max)
    }

    fn kinetic_energy(&self) -> f32 {
        let world = self.app.world();
        let mass = world.get::<ComputedMass>(self.tank).unwrap().value();
        let velocity = world.get::<LinearVelocity>(self.tank).unwrap().0;
        let spin = self.pose().1.inverse() * world.get::<AngularVelocity>(self.tank).unwrap().0;
        let inertia = world
            .get::<ComputedAngularInertia>(self.tank)
            .unwrap()
            .tensor();
        let energy = (mass * velocity.length_squared() + spin.dot(inertia * spin)) / 2.0;
        assert!(
            energy.is_finite() && energy >= 0.0,
            "finite rigid-body arrival energy"
        );
        energy
    }

    fn foot_height(&self) -> f32 {
        let (p, r) = self.pose();
        self.droop_feet
            .iter()
            .map(|foot| (p + r * *foot).y)
            .fold(f32::NEG_INFINITY, f32::max)
    }

    fn enter(&mut self, at: Vec2, yaw_deg: f32, rolling: bool) {
        let rotation = Quat::from_rotation_y(yaw_deg.to_radians());
        let position = Vec3::new(at.x, self.rest, at.y);
        drive_load::place(&mut self.app, self.tank, position, rotation, 0.0, 1);
        // Fixture: settle the suspension before drive entry, so launch transients are not wall lift.
        for _ in 0..2 * FIXED_TICKS_PER_SECOND {
            drive_tick(&mut self.app, self.tank, 0.0, 0.0);
        }
        let position = self.pose().0;
        let trans = self
            .app
            .world()
            .resource::<crate::track::sim::TrackGear>()
            .trans();
        let (speed, gear) = if rolling {
            (trans.governed_speed(false), trans.gears_fwd.len() as u8)
        } else {
            (0.0, 1)
        };
        drive_load::place(&mut self.app, self.tank, position, rotation, speed, gear);
    }
}

/// Margins against compliant containment and the fixture's support reach, in metres.
struct DriveMargins {
    side: f32,
    revisit: f32,
    held: f32,
    height: f32,
    slide: f32,
}

/// Successive held-position observations over the later half of a contact trace. Resolution: a
/// displacement counts as sustained only after three successive climbs, each past the numeric
/// allowance; a window's length is printed with the margins.
const HELD_WINDOWS: usize = 4;

/// What [`Containment::margins`] measures, in metres; each is met at zero or above.
struct ContainmentMargins {
    /// The arrival bound less the peak overlap.
    side: f32,
    /// The static hold less the later half's least overlap.
    revisit: f32,
    /// Against sustained outward displacement.
    held: f32,
}

/// Server containment permits bounded contact compression, never accumulating outward drift.
/// Derivation and premises: map-surroundings §3.6. All inputs come from the mounted sim.
struct Containment {
    mass: f32,
    linear_overlap_limit: f32,
    effective_mass: f32,
    omega_squared: f32,
    discovery_step: f32,
    roundoff: f32,
    push: f32,
    energy: f32,
    overlap: Vec<f32>,
    first_contact: Option<usize>,
}

impl Containment {
    fn new(bed: &Bed) -> Self {
        let world = bed.app.world();
        let mass = *world.get::<ComputedMass>(bed.tank).unwrap();
        let inverse_inertia = world
            .get::<ComputedAngularInertia>(bed.tank)
            .unwrap()
            .inverse_tensor();
        // DERIVED: the Frobenius norm bounds every directional inverse inertia under rotation.
        let inverse_norm = [Vec3::X, Vec3::Y, Vec3::Z]
            .into_iter()
            .map(|axis| (inverse_inertia * axis).length_squared())
            .sum::<f32>()
            .sqrt();
        let center = world.get::<ComputedCenterOfMass>(bed.tank).unwrap().0;
        let radius = bed
            .hulls
            .iter()
            .map(|(shape, local)| {
                let aabb = shape.aabb(local.translation, local.rotation);
                (aabb.min - center)
                    .abs()
                    .max((aabb.max - center).abs())
                    .length()
            })
            .fold(0.0_f32, f32::max);
        let effective_mass = (mass.inverse() + radius * radius * inverse_norm).recip();
        let softness = world
            .resource::<avian3d::dynamics::solver::ContactSoftnessCoefficients>()
            .non_dynamic;
        let h = world.resource::<Time<Substeps>>().delta_secs();
        // DERIVED from SoftnessParameters::compute_coefficients: s*b/(i*h) = omega^2.
        let omega_squared = softness.mass_scale * softness.bias / (softness.impulse_scale * h);
        assert!(effective_mass > 0.0 && omega_squared.is_finite() && omega_squared > 0.0);
        assert!(
            (0.0..1.0).contains(&(1.0 - h * softness.mass_scale * softness.bias)),
            "the configured scalar contact must contract overlap each substep"
        );
        Self {
            mass: mass.value(),
            linear_overlap_limit: world
                .resource::<avian3d::dynamics::solver::SolverConfig>()
                .max_overlap_solve_speed
                * world.resource::<PhysicsLengthUnit>().0
                / softness.bias,
            effective_mass,
            omega_squared,
            discovery_step: world.resource::<Time<Fixed>>().timestep().as_secs_f32(),
            // Resolution: a 32-operation f32 error budget for pose composition and the AABB,
            // scaled by the fixture's coordinates; independent of observed solver overlap.
            roundoff: 32.0 * f32::EPSILON * (bed.half + radius),
            push: 0.0,
            energy: bed.kinetic_energy(),
            overlap: Vec::new(),
            first_contact: None,
        }
    }

    fn observe(&mut self, bed: &Bed) {
        let world = bed.app.world();
        let effect = world
            .get::<crate::track::sim::TrackGripEffect>(bed.tank)
            .unwrap();
        let push = effect.traction_capacity_n + effect.rolling_force.length();
        assert!(
            push.is_finite() && push >= 0.0,
            "finite contact-owned push limit"
        );
        self.push = self.push.max(push);
        let overlap = bed.side_overrun();
        assert!(overlap.is_finite(), "finite hull side overlap");
        if self.first_contact.is_none() {
            self.energy = self.energy.max(bed.kinetic_energy());
            if overlap >= -bed.tolerance {
                self.first_contact = Some(self.overlap.len());
            }
        }
        self.overlap.push(overlap);
    }

    fn margins(&self, name: &str) -> ContainmentMargins {
        let first = self
            .first_contact
            .expect("the drive must exercise the wall");
        let stiffness = self.effective_mass * self.omega_squared;
        let steady = self.push / stiffness;
        // DERIVED: k*x^2/2 <= E + F*x, discarding damping to bound arrival compression.
        let arrival = steady + (steady * steady + 2.0 * self.energy / stiffness).sqrt();
        // Avian 0.7 refreshes manifolds once per physics tick, outside its solver substeps.
        // DERIVED by Cauchy-Schwarz in the mass metric: any hull point's speed is bounded by
        // sqrt(2E/m_eff). Discovery can add one tick of motion before contact can resist it.
        let discovery = (2.0 * self.energy / self.effective_mass).sqrt() * self.discovery_step;
        assert!(
            arrival <= self.linear_overlap_limit,
            "{name}: arrival bound {arrival} exceeds the uncapped contact law's domain {}",
            self.linear_overlap_limit
        );
        let peak = self
            .overlap
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let contact = &self.overlap[first..];
        assert!(contact.len() >= 2, "the drive must continue after contact");
        let late = &contact[contact.len() / 2..];
        // A revisitation check, not a transient bound: none is derived for the coupled rig
        // (map-surroundings §3.6). Somewhere in the later half the overlap comes back within one
        // force-limited compression of the face, where the static hold sits.
        let floor = late.iter().copied().fold(f32::INFINITY, f32::min);
        let revisit = steady + self.roundoff - floor;
        // Sustained outward displacement: the held position, each successive window's floor,
        // stands past the static hold and farther out than the window before, in every window.
        // A decaying transient comes back in however slowly; a hold that yields keeps climbing.
        assert!(
            late.len() >= HELD_WINDOWS,
            "{name}: the later half spans every window"
        );
        let window = late.len().div_ceil(HELD_WINDOWS);
        let floors: Vec<f32> = late
            .chunks(window)
            .map(|window| window.iter().copied().fold(f32::INFINITY, f32::min))
            .collect();
        let beyond = floors
            .iter()
            .map(|floor| floor - steady - self.roundoff)
            .fold(f32::INFINITY, f32::min);
        let climb = floors
            .windows(2)
            .map(|pair| pair[1] - pair[0] - self.roundoff)
            .fold(f32::INFINITY, f32::min);
        let held = -beyond.min(climb);
        let late_peak = late.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        println!(
            "DERIVED {name}: mass={} kg, effective_mass>={} kg, omega={} rad/s, push<={} N, arrival_energy<={} J, steady<={} m, contact_arrival<={} m, discovery<={} m, roundoff={} m, held window={} s; MEASURED peak={} m, side margin={} m, later-half peak={} m, later-half floor={} m, revisit margin={} m, held floors={:?} m, sustained margin={} m",
            self.mass,
            self.effective_mass,
            self.omega_squared.sqrt(),
            self.push,
            self.energy,
            steady,
            arrival,
            discovery,
            self.roundoff,
            window as f32 * self.discovery_step,
            peak,
            arrival + discovery + self.roundoff - peak,
            late_peak,
            floor,
            revisit,
            floors,
            held
        );
        ContainmentMargins {
            side: arrival + discovery + self.roundoff - peak,
            revisit,
            held,
        }
    }
}

fn drive_at_wall(name: &str, yaw: f32, corner: bool, rolling: bool) -> DriveMargins {
    let mut bed = Bed::new(false);
    let z = if corner { bed.half - 16.0 } else { -40.0 };
    bed.enter(Vec2::new(bed.half - 16.0, z), yaw, rolling);
    let mut containment = Containment::new(&bed);
    let mut foot = f32::NEG_INFINITY;
    let mut slide_start = None;
    let mut slide = 0.0_f32;
    // Fixture: enough time to reach the side from rest and traverse several hull lengths.
    for _ in 0..12 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut bed.app, bed.tank, 1.0, 0.0);
        containment.observe(&bed);
        foot = foot.max(bed.foot_height());
        if bed.side_overrun() >= -bed.tolerance {
            let start = *slide_start.get_or_insert(bed.pose().0.z);
            slide = slide.max((bed.pose().0.z - start).abs());
        }
    }
    let margins = containment.margins(name);
    println!(
        "MEASURED {name}: droop-foot height={foot} m, height margin={} m, slide={slide} m",
        bed.tolerance - foot
    );
    // DERIVED: a whole body width of tangential travel rules out a stationary perch.
    let width = bed
        .hulls
        .iter()
        .map(|(shape, local)| {
            let aabb = shape.aabb(local.translation, local.rotation);
            aabb.max.x - aabb.min.x
        })
        .fold(0.0_f32, f32::max);
    DriveMargins {
        side: margins.side,
        revisit: margins.revisit,
        held: margins.held,
        height: bed.tolerance - foot,
        slide: slide - width,
    }
}

#[test]
fn full_throttle_head_on_stays_inside() {
    let margins = drive_at_wall("head-on", -90.0, false, true);
    assert!(margins.side >= 0.0 && margins.revisit >= 0.0 && margins.held >= 0.0);
}

#[test]
fn full_throttle_into_a_convex_corner_stays_inside() {
    let margins = drive_at_wall("convex-corner", -135.0, true, true);
    assert!(margins.side >= 0.0 && margins.revisit >= 0.0 && margins.held >= 0.0);
}

/// The server relies on bounded compression without sustained outward displacement throughout the
/// grind.
#[test]
fn oblique_drive_slides_without_rearing_climbing_or_perching() {
    // Fixture: 30 degrees to the wall, accelerating from rest.
    let margins = drive_at_wall("oblique-30", -150.0, false, false);
    assert!(
        margins.height >= 0.0,
        "an end lifted beyond its own droop reach"
    );
    assert!(
        margins.slide > 0.0,
        "the hull must slide a full body width while touching the wall"
    );
    assert!(
        margins.side >= 0.0,
        "grind exceeded the compliant arrival bound"
    );
    assert!(
        margins.revisit >= 0.0,
        "the grind's later half never came back to the static hold"
    );
    assert!(
        margins.held >= 0.0,
        "the grind's held position climbed outward window after window"
    );
}

/// The server's side bound must hold even when terrain support is absent on arrival. The ramp
/// supplies the launch through ordinary track contacts; the test never authors an upward impulse.
#[test]
fn a_hull_launched_off_a_ramp_stays_inside_the_wall() {
    let mut bed = Bed::new(true);
    bed.enter(Vec2::new(bed.half - 50.0, -40.0), -90.0, true);
    let mut containment = Containment::new(&bed);
    let mut airborne_at_wall = false;
    let mut peak = f32::NEG_INFINITY;
    for _ in 0..12 * FIXED_TICKS_PER_SECOND {
        drive_tick(&mut bed.app, bed.tank, 1.0, 0.0);
        let side = bed.side_overrun();
        containment.observe(&bed);
        peak = peak.max(bed.pose().0.y);
        let contacts = &bed
            .app
            .world()
            .get::<crate::track::sim::TrackContacts>(bed.tank)
            .unwrap()
            .0;
        airborne_at_wall |= side >= -bed.tolerance && contacts.iter().all(Vec::is_empty);
    }
    let margins = containment.margins("ramp");
    println!("MEASURED ramp: peak root={peak} m, airborne at wall={airborne_at_wall}");
    assert!(
        airborne_at_wall,
        "the ramp must deliver an airborne wall contact"
    );
    assert!(
        margins.side >= 0.0,
        "airborne hull exceeded the compliant arrival bound"
    );
    assert!(
        margins.revisit >= 0.0,
        "the airborne arrival's later half never came back to the static hold"
    );
    assert!(
        margins.held >= 0.0,
        "the airborne arrival's held position climbed outward window after window"
    );
}
