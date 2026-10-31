//! A tank kind's envelopes: invisible boxes, never colliders, that every part of a body stays inside
//! whatever its servos do. Subject: the game's damage model, which resolves a round's crossing of a
//! tank against them (design `armor-penetration-and-damage.md` §13.5, "The envelope", provisional;
//! nothing reads them yet).
//!
//! Entry point: [`TankEnvelopes::of`], a pure function of the bake's data (the extracted geometry and
//! the tank sheet's servos), computed once per tank kind at load and shared by every spawn through
//! [`super::TankBlueprint`].
//!
//! Invariant: in exact arithmetic, every vertex of every captured primitive (armour, components,
//! crew boxes and collision proxies) under a body, at every angle each servo strictly below that
//! body can take within its sheet travel, lies inside the body's envelope, in the body's own frame.
//! The box is computed in f64 and each face is then stepped one f32 past the nearest f32, a margin
//! of at least half an f32 ulp of the face's coordinate that dominates the f64 evaluation's own
//! error (DERIVED). The sim composes poses in f32 and so places a vertex a few f32 ulps from where
//! exact arithmetic would (DERIVED, about 1e-6 m at tank scale); a consumer that needs the sim's
//! f32 placement inside grows the box by at least that, which a round's own radius already does.
//!
//! Not swept: a recoiling barrel's stroke, which no data bounds (`RecoilSpec` is a spring); the
//! recoiling parts are held at battery.

use std::f64::consts::{PI, TAU};

use bevy::math::bounding::Aabb3d;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

use super::TankGeometry;
use crate::spec::TankSpec;

/// The envelopes of one tank kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TankEnvelopes {
    /// The whole tank, in its body's frame: the tank root's, the frame every
    /// [`super::NodeGeometry::root_position`] is in.
    pub tank: Aabb3d,
    /// The turret a cook-off launches ([`super::turret_node`]), in that node's own frame, which is
    /// the frame of the free body it becomes. Its own traverse is the body's pose and is not swept;
    /// the servos under it are.
    pub turret: Aabb3d,
}

impl TankEnvelopes {
    /// Both envelopes, from the bake's data alone. Panics on a tank with no turret, the rig contract
    /// spawn asserts (`tank::spawn::resolve_rig_nodes`).
    pub(crate) fn of(geometry: &TankGeometry, spec: &TankSpec) -> Self {
        let turret = super::turret_node(geometry, spec).unwrap_or_else(|| {
            panic!(
                "envelope: no Yaw servo at or above the Gunner view's node, so no turret to \
                 launch — the rig contract spawn asserts"
            )
        });
        let joints = Joints::new(geometry, spec);
        Self {
            tank: joints.envelope(0),
            turret: joints.envelope(turret),
        }
    }
}

/// A node's pose relative to a frame, composed as avian's `ColliderTransform` propagation composes
/// it (`collider_transform/plugin.rs`, `propagate_collider_transforms_recursive`): translation
/// through [`Pose::apply`], rotations multiplied, scales componentwise. That is the pose a collider
/// hung under the node reaches the body at, a collision proxy's non-unit scale included.
#[derive(Clone, Copy)]
struct Pose {
    translation: DVec3,
    rotation: DQuat,
    scale: DVec3,
}

impl Pose {
    const IDENTITY: Self = Self {
        translation: DVec3::ZERO,
        rotation: DQuat::IDENTITY,
        scale: DVec3::ONE,
    };

    /// `ColliderTransform::transform_point`: scale, then rotate, then translate.
    fn apply(&self, point: DVec3) -> DVec3 {
        self.translation + self.rotation * (self.scale * point)
    }

    fn then(&self, local: &Transform) -> Self {
        Self {
            translation: self.apply(local.translation.as_dvec3()),
            rotation: self.rotation * local.rotation.as_dquat(),
            scale: self.scale * local.scale.as_dvec3(),
        }
    }
}

/// The node tree with each servo node's axis and travel, read once.
struct Joints<'a> {
    geometry: &'a TankGeometry,
    children: Vec<Vec<usize>>,
    /// Per node: the servo's local axis and its travel window in radians (`None`: a full turn).
    servos: Vec<Option<(DVec3, Option<(f64, f64)>)>>,
}

impl<'a> Joints<'a> {
    fn new(geometry: &'a TankGeometry, spec: &TankSpec) -> Self {
        let mut children = vec![Vec::new(); geometry.nodes.len()];
        for (index, node) in geometry.nodes.iter().enumerate() {
            if let Some(parent) = node.parent {
                children[parent].push(index);
            }
        }
        let servos = geometry
            .nodes
            .iter()
            .map(|node| {
                let servo = spec.servos.get(&node.name)?;
                let travel = servo
                    .travel_limits()
                    .map(|(min, max)| (f64::from(min), f64::from(max)));
                Some((servo.role().axis().as_dvec3(), travel))
            })
            .collect();
        Self {
            geometry,
            children,
            servos,
        }
    }

    /// The envelope of everything under `body`, in `body`'s frame.
    fn envelope(&self, body: usize) -> Aabb3d {
        let points = self.hull_points(body);
        assert!(
            !points.is_empty(),
            "envelope: `{}` holds no captured geometry",
            self.geometry.nodes[body].name
        );
        let (low, high) = points.iter().fold(
            (DVec3::INFINITY, DVec3::NEG_INFINITY),
            |(low, high), &point| (low.min(point), high.max(point)),
        );
        Aabb3d::from_min_max(
            low.as_vec3().map(f32::next_down),
            high.as_vec3().map(f32::next_up),
        )
    }

    /// Points whose convex hull holds every captured primitive under `frame`, in `frame`'s own
    /// coordinates, with every servo strictly below `frame` swept over its travel. A servo's
    /// subtree enters as the corners of its swept box ([`swept_corners`]); everything else enters
    /// vertex by vertex.
    ///
    /// Restarting the composition at a servo is exact because a servo and every ancestor of it is
    /// authored at unit scale (`L2.UNIT_SCALE`), so the pose above it is rigid.
    fn hull_points(&self, frame: usize) -> Vec<DVec3> {
        let mut points = Vec::new();
        let mut stack = vec![(frame, Pose::IDENTITY)];
        while let Some((index, pose)) = stack.pop() {
            if index != frame
                && let Some((axis, travel)) = self.servos[index]
            {
                assert_eq!(
                    pose.scale,
                    DVec3::ONE,
                    "envelope: servo `{}` composes a scale; L2.UNIT_SCALE refuses that asset",
                    self.geometry.nodes[index].name
                );
                let swept = swept_corners(&self.hull_points(index), axis, travel);
                points.extend(swept.into_iter().map(|corner| pose.apply(corner)));
                continue;
            }
            for primitive in &self.geometry.nodes[index].primitives {
                points.extend(
                    primitive
                        .positions
                        .iter()
                        .map(|&vertex| pose.apply(Vec3::from(vertex).as_dvec3())),
                );
            }
            for &child in &self.children[index] {
                stack.push((child, pose.then(&self.geometry.nodes[child].transform)));
            }
        }
        points
    }
}

/// The corners of the box, in a servo's own frame, that holds every one of `points` turned about
/// `axis` through every angle of `travel` (`None`: a full turn). The servo turns its subtree by
/// `rest · R(axis, θ)` (`tank::servo_pose`), so in the frame its rest pose defines, a point `p`
/// sweeps `R(axis, θ) p`. Each point's arc is bounded exactly ([`arc_range`]), and the box of the
/// corners holds the hull of the turned points at every angle.
fn swept_corners(points: &[DVec3], axis: DVec3, travel: Option<(f64, f64)>) -> Vec<DVec3> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut low = DVec3::INFINITY;
    let mut high = DVec3::NEG_INFINITY;
    for &point in points {
        // Rodrigues: R(axis, θ) p = along + across·cos θ + turn·sin θ.
        let along = axis * axis.dot(point);
        let across = point - along;
        let turn = axis.cross(point);
        for i in 0..3 {
            let (from, to) = arc_range(across[i], turn[i], travel);
            low[i] = low[i].min(along[i] + from);
            high[i] = high[i].max(along[i] + to);
        }
    }
    (0..8)
        .map(|corner| {
            DVec3::new(
                if corner & 1 == 0 { low.x } else { high.x },
                if corner & 2 == 0 { low.y } else { high.y },
                if corner & 4 == 0 { low.z } else { high.z },
            )
        })
        .collect()
}

/// The range of `b cos θ + c sin θ` over `travel`. It is `ρ cos(θ − φ)` with `ρ = hypot(b, c)` and
/// `φ = atan2(c, b)`, so its extremes lie at the window's ends, at `ρ` where the window holds `φ`,
/// and at `−ρ` where it holds `φ + π`.
fn arc_range(b: f64, c: f64, travel: Option<(f64, f64)>) -> (f64, f64) {
    let rho = b.hypot(c);
    let Some((start, end)) = travel else {
        return (-rho, rho);
    };
    let at = |theta: f64| b * theta.cos() + c * theta.sin();
    let (mut low, mut high) = (at(start).min(at(end)), at(start).max(at(end)));
    let phase = c.atan2(b);
    if window_holds(start, end, phase) {
        high = rho;
    }
    if window_holds(start, end, phase + PI) {
        low = -rho;
    }
    (low, high)
}

/// Whether `[start, end]` holds an angle congruent to `angle` modulo a full turn.
fn window_holds(start: f64, end: f64, angle: f64) -> bool {
    angle + TAU * ((start - angle) / TAU).ceil() <= end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::{certify_asset, fixture, spec_ron_name};
    use crate::substances::SubstanceRegistry;

    fn shipped() -> Vec<(std::path::PathBuf, super::super::CertifiedAsset)> {
        let shipped = fixture::shipped_assets();
        assert!(!shipped.is_empty(), "no shipped asset was discovered");
        shipped
            .into_iter()
            .map(|model| {
                let spec_ron =
                    std::fs::read_to_string(spec_ron_name(&model)).expect("the sheet is readable");
                let asset = certify_asset(&model, &spec_ron, &SubstanceRegistry::shipped())
                    .expect("the pair certifies");
                (model, asset)
            })
            .collect()
    }

    /// The angles the oracle poses a servo at: both ends of a limited window and evenly between
    /// them, or a full turn in even steps. Counts are test resolution, not a property of the tank.
    fn sampled(travel: Option<(f64, f64)>) -> Vec<f64> {
        match travel {
            Some((start, end)) => (0..=12)
                .map(|step| start + (end - start) * f64::from(step) / 12.0)
                .collect(),
            None => (0..36).map(|step| TAU * f64::from(step) / 36.0).collect(),
        }
    }

    /// Forward kinematics, written apart from the sweep: every vertex under `frame` at every
    /// combination of sampled angles along its own servo chain, in `frame`'s coordinates. A servo
    /// node's local rotation is `rest · R(axis, θ)` (`tank::servo_pose`); a pose composes as avian's
    /// collider propagation does.
    fn posed_vertices(joints: &Joints, frame: usize, mut visit: impl FnMut(&str, DVec3)) {
        let nodes = &joints.geometry.nodes;
        let mut stack: Vec<(usize, DVec3, DQuat, DVec3)> =
            vec![(frame, DVec3::ZERO, DQuat::IDENTITY, DVec3::ONE)];
        while let Some((index, translation, rotation, scale)) = stack.pop() {
            for primitive in &nodes[index].primitives {
                for &vertex in &primitive.positions {
                    let local = Vec3::from(vertex).as_dvec3();
                    visit(&nodes[index].name, translation + rotation * (scale * local));
                }
            }
            for &child in &joints.children[index] {
                let local = nodes[child].transform;
                let at = translation + rotation * (scale * local.translation.as_dvec3());
                let rest = rotation * local.rotation.as_dquat();
                let composed = scale * local.scale.as_dvec3();
                match joints.servos[child] {
                    Some((axis, travel)) => {
                        for theta in sampled(travel) {
                            let turned = rest * DQuat::from_axis_angle(axis, theta);
                            stack.push((child, at, turned, composed));
                        }
                    }
                    None => stack.push((child, at, rest, composed)),
                }
            }
        }
    }

    fn inside(envelope: &Aabb3d, point: DVec3) -> bool {
        let (low, high) = (envelope.min.as_dvec3(), envelope.max.as_dvec3());
        point.cmpge(low).all() && point.cmple(high).all()
    }

    /// Invariant: every vertex of every part, at every sampled traverse and elevation within the
    /// sheet's travel windows (both ends included), lies inside the tank's envelope. Step 6b relies
    /// on it: a round outside every envelope meets no part of any tank, so its crossing can start
    /// at the envelope. A mutant that sweeps the wrong way round an axis, drops a servo's subtree,
    /// or leaves a part out fails here.
    #[test]
    fn every_part_at_every_servo_angle_lies_inside_the_tank_envelope() {
        for (model, asset) in shipped() {
            let envelope = TankEnvelopes::of(&asset.geometry, &asset.spec).tank;
            let joints = Joints::new(&asset.geometry, &asset.spec);
            let mut checked = 0usize;
            posed_vertices(&joints, 0, |node, point| {
                checked += 1;
                assert!(
                    inside(&envelope, point),
                    "{}: `{node}` reaches {point} outside {envelope:?}",
                    model.display()
                );
            });
            assert!(checked > 0, "{}: no vertex posed", model.display());
        }
    }

    /// Invariant: the turret a cook-off launches, at every sampled elevation of the servos under
    /// it, lies inside its own envelope in its own frame, which is the free body's frame once it is
    /// launched. Step 6b relies on it for a launched turret as for a tank.
    #[test]
    fn a_launched_turret_lies_inside_its_own_envelope() {
        for (model, asset) in shipped() {
            let envelope = TankEnvelopes::of(&asset.geometry, &asset.spec).turret;
            let turret = crate::bake::turret_node(&asset.geometry, &asset.spec)
                .expect("a shipped tank has a turret");
            let joints = Joints::new(&asset.geometry, &asset.spec);
            let mut checked = 0usize;
            posed_vertices(&joints, turret, |node, point| {
                checked += 1;
                assert!(
                    inside(&envelope, point),
                    "{}: turret part `{node}` reaches {point} outside {envelope:?}",
                    model.display()
                );
            });
            assert!(checked > 0, "{}: the turret holds no part", model.display());
        }
    }

    /// Invariant: a kind's envelopes are a function of its data alone, so every spawn of the kind
    /// reads the same boxes. Two independent loads parse two sheets whose servo maps iterate in
    /// different orders (`HashMap`'s per-instance seed); a mutant that let iteration order, or any
    /// state beyond the data, reach the result breaks bit equality here. The blueprint carries
    /// exactly what the function gives.
    #[test]
    fn every_load_of_a_kind_gives_the_same_envelopes_bit_for_bit() {
        let bits = |envelope: &TankEnvelopes| {
            [envelope.tank, envelope.turret]
                .map(|aabb| [aabb.min, aabb.max].map(|v| v.to_array().map(f32::to_bits)))
        };
        for ((model, first), (_, second)) in shipped().into_iter().zip(shipped()) {
            let a = TankEnvelopes::of(&first.geometry, &first.spec);
            let b = TankEnvelopes::of(&second.geometry, &second.spec);
            assert_eq!(bits(&a), bits(&b), "{}", model.display());
            let blueprint =
                super::super::TankBlueprint::new(String::new(), second.geometry, second.spec);
            assert_eq!(bits(&blueprint.envelopes), bits(&a), "{}", model.display());
        }
    }

    /// Invariant: [`arc_range`] is the exact range of `b cos θ + c sin θ` over a window, so the
    /// sweep neither leaves out an arc's peak inside the window nor claims one outside it. Checked
    /// against dense evaluation on windows that hold the peak, the trough, both, or neither.
    #[test]
    fn the_arc_range_is_the_range_of_the_arc() {
        let (b, c) = (0.6_f64, -0.8_f64);
        for (start, end) in [
            (-0.3, 0.2),
            (-2.5, -0.5),
            (1.0, 2.0),
            (-4.0, 4.0),
            (5.0, 6.0),
        ] {
            let (low, high) = arc_range(b, c, Some((start, end)));
            let samples: Vec<f64> = (0..=10_000)
                .map(|step| {
                    let theta = match step {
                        10_000 => end,
                        _ => start + (end - start) * f64::from(step) / 10_000.0,
                    };
                    b * theta.cos() + c * theta.sin()
                })
                .collect();
            let sampled_low = samples.iter().copied().fold(f64::INFINITY, f64::min);
            let sampled_high = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            // Dense sampling lands within the curvature of the peak: `ρ (1 − cos(δ/2))` for a step
            // `δ` of at most 8/10 000 rad, under 1e-7 (DERIVED).
            assert!(
                low <= sampled_low && sampled_low - low < 1.0e-7,
                "low on [{start}, {end}]"
            );
            assert!(
                high >= sampled_high && high - sampled_high < 1.0e-7,
                "high on [{start}, {end}]"
            );
        }
    }
}
