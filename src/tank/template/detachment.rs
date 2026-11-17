//! The game's damage model: before-play admission of the remaining body's inertia.

use bevy::math::{DMat3, DQuat, DVec3};
use bevy::prelude::*;

use super::{TankGeometry, TankSpec, TurretBody};

// Resolution: stop at 0.1 microradian half-width, avoiding searches finer than f32 rotations
// at order-one angles can reliably preserve.
// An unresolved interval is refused as numerically uncertified, not as disproven sourced data.
const MIN_HALF_WIDTH: f64 = 1e-7;
// Resolution: reserve f32 arithmetic headroom for the runtime tensor subtraction and pose
// composition. Squared chain length and absolute coordinate reach cover accumulated pose error,
// cancellation in centre differences, and the quadratic PA term.
const ROUNDING_ULPS: f64 = 64.0;

/// Require λmin(I_vehicle - R I_t Rᵀ - μ PA(t-c)) positive over allowed servo poses,
/// with M=spec.mass, m=body.mass=spec.turret.mass and μ=mM/(M-m). TankSpec::validate
/// guarantees M>m and finite positive body moments.
/// Unit-scale assembly ancestry is certified by the model gate. Servo composition matches
/// servo_pose: authored rest rotation, then the role's local-axis turn.
///
/// Each cell uses its centre's exact tensor, not independently maximized tensor terms.
/// DERIVED: if servo i varies by at most h_i, its centroid moves by at most r_i h_i.
/// Thus e=Σr_i h_i bounds centroid motion and a=Σh_i bounds orientation change. The tensor
/// changes by at most B=(max(I_t)-min(I_t))a + μ(2|t-c|+e)e in spectral norm: the first term
/// bounds rotation's commutator, the second follows from PA(d)=[d]×ᵀ[d]×. Weyl then gives
/// λmin(cell) >= λmin(centre)-B. Subdivide any unproved cell; a sample alone never admits it.
/// Only margins unresolved at MIN_HALF_WIDTH or within the f32 rounding reserve over-refuse.
pub(super) fn admit_detachment(
    file: &str,
    spec: &TankSpec,
    geometry: &TankGeometry,
    turret: usize,
    centre: Vec3,
    body: TurretBody,
) {
    let (mut links, mut windows, mut reaches) = (Vec::new(), Vec::new(), Vec::new());
    let mut reach = body.centre_of_mass.as_dvec3().length();
    let mut index = turret;
    while index != 0 {
        let node = &geometry.nodes[index];
        let joint = spec.servos.get(&node.name).map(|servo| {
            let tau = std::f64::consts::TAU;
            let (lo, hi) = servo
                .travel_limits()
                .map_or((-tau / 2.0, tau / 2.0), |(lo, hi)| {
                    let span = (f64::from(hi) - f64::from(lo)).min(tau);
                    let lo = f64::from(lo).rem_euclid(tau);
                    (lo, lo + span)
                });
            let slot = windows.len();
            windows.push((lo, hi));
            reaches.push(reach);
            (slot, servo.role().axis().as_dvec3())
        });
        links.push((
            node.transform.translation.as_dvec3(),
            node.transform.rotation.as_dquat().normalize(),
            joint,
        ));
        reach += node.transform.translation.as_dvec3().length();
        index = node
            .parent
            .expect("every model node descends from the scene wrapper");
    }
    let vehicle = spec.hull_angular_inertia().tensor().to_mat3().as_dmat3();
    let turret_tensor = body.angular_inertia.tensor().to_mat3().as_dmat3();
    let principal = body.angular_inertia.principal.as_dvec3();
    let spread = principal.max_element() - principal.min_element();
    let m = f64::from(body.mass);
    let total = f64::from(spec.mass);
    let mu = m * total / (total - m);
    let rounding = ROUNDING_ULPS
        * f64::from(f32::EPSILON)
        * (links.len() + 1).pow(2) as f64
        * (vehicle.x_axis.x.max(vehicle.y_axis.y).max(vehicle.z_axis.z)
            + principal.max_element()
            + mu * (reach + centre.as_dvec3().length()).powi(2));
    let mut cells = vec![windows];
    let mut margin = f64::INFINITY;
    while let Some(cell) = cells.pop() {
        let (mut point, mut rotation) = (body.centre_of_mass.as_dvec3(), DQuat::IDENTITY);
        for &(translation, rest, joint) in &links {
            let local = joint.map_or(rest, |(slot, axis)| {
                let (lo, hi) = cell[slot];
                rest * DQuat::from_axis_angle(axis, (lo + hi) / 2.0)
            });
            point = translation + local * point;
            rotation = local * rotation;
        }
        let d = point - centre.as_dvec3();
        let r = DMat3::from_quat(rotation);
        let tensor = vehicle
            - r * turret_tensor * r.transpose()
            - mu * (DMat3::from_diagonal(DVec3::splat(d.length_squared()))
                - DMat3::from_cols(d * d.x, d * d.y, d * d.z));
        let eigenvalue = smallest_eigenvalue(tensor);
        let widths: Vec<_> = cell.iter().map(|&(lo, hi)| (hi - lo) / 2.0).collect();
        let e: f64 = reaches.iter().zip(&widths).map(|(r, h)| r * h).sum();
        let bound = spread * widths.iter().sum::<f64>() + mu * (2.0 * d.length() + e) * e;
        if eigenvalue > bound + rounding {
            margin = margin.min(eigenvalue - bound - rounding);
            continue;
        }
        let split = widths
            .iter()
            .enumerate()
            .filter(|(_, h)| **h > MIN_HALF_WIDTH)
            .max_by(|(i, a), (j, b)| {
                let weight = |slot: usize| spread + 2.0 * mu * (d.length() + e) * reaches[slot];
                (weight(*i) * **a).total_cmp(&(weight(*j) * **b))
            })
            .map(|(i, _)| i);
        let reason = if eigenvalue <= 0.0 {
            "non-positive remaining inertia at an allowed pose"
        } else {
            "remaining inertia numerically uncertified at the angular/rounding resolution"
        };
        assert!(
            eigenvalue > 0.0 && split.is_some(),
            "{file}: mass/inertia_extents full-vehicle box versus turret.mass/inertia_extents \
             and assembly position (turret.node, model Center_Of_Mass, servo pivots/axes/limits): \
             {reason}; remaining smallest moment {eigenvalue} \
             kg m², between-pose bound {bound}, rounding reserve {rounding}, servo cells \
             {cell:?} radians. Reconcile the full-vehicle box with the assembly's mass, inertia \
             and position; a positive sample within the reserve or an unresolved cell is \
             numerically uncertified, not proof that sourced masses are wrong"
        );
        let slot = split.expect("the unproved positive cell has a resolvable servo interval");
        let mut other = cell.clone();
        let mid = (cell[slot].0 + cell[slot].1) / 2.0;
        other[slot].0 = mid;
        let mut cell = cell;
        cell[slot].1 = mid;
        cells.extend([other, cell]);
    }
    debug!("{file}: DERIVED detachment inertia margin >= {margin} kg m² over allowed servo poses");
}

/// Bisection on Sylvester's criterion, with normalization preventing determinant overflow.
/// Resolution: 64 iterations resolve the eigenvalue to f64 precision; admission separately
/// reserves f32 headroom. Three row entries bound every eigenvalue by ±3 after normalization.
fn smallest_eigenvalue(tensor: DMat3) -> f64 {
    let scale = tensor
        .to_cols_array()
        .into_iter()
        .map(f64::abs)
        .fold(0.0, f64::max);
    if scale == 0.0 {
        return 0.0;
    }
    let tensor = tensor / scale;
    let (mut lo, mut hi) = (-3.0, 3.0);
    for _ in 0..64 {
        let mid = (lo + hi) / 2.0;
        let shifted = tensor - DMat3::from_diagonal(DVec3::splat(mid));
        if shifted.x_axis.x > 0.0
            && shifted.x_axis.x * shifted.y_axis.y - shifted.x_axis.y * shifted.y_axis.x > 0.0
            && shifted.determinant() > 0.0
        {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo * scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::NodeGeometry;
    use crate::tank::servo::{ServoRole, ServoSpec};

    /// Fixture full vehicle: 30 t, 3 × 1.8 × 6 m box. Assembly: 8 t, 2.6 × 1.1 × 2.8 m box,
    /// centroid 0.45 m above and behind its pivot; vehicle centre 1 m below the model origin.
    /// The caller owns the pivot height and hence the mass/inertia consistency under yaw.
    fn construction(pivot_y: f32) -> (TankSpec, TankGeometry, TurretBody, Vec3) {
        let mut spec = super::super::shipped_sheet().0;
        spec.mass = 30_000.0;
        spec.inertia_extents = (3.0, 1.8, 6.0);
        spec.turret.mass.0 = 8_000.0;
        spec.turret.inertia_extents.0 = (2.6, 1.1, 2.8);
        spec.servos.clear();
        spec.servos.insert(
            "FixtureYaw".into(),
            ServoSpec::test_continuous(ServoRole::Yaw, 1.0),
        );
        let node = |name: &str, parent, transform: Transform| NodeGeometry {
            name: name.into(),
            parent,
            transform,
            root_position: transform.translation,
            root_rotation: transform.rotation,
            root_scale: Vec3::ONE,
            primitives: Vec::new(),
        };
        let geometry = TankGeometry {
            nodes: vec![
                node("FixtureRoot", None, Transform::IDENTITY),
                node(
                    "FixtureYaw",
                    Some(0),
                    Transform::from_xyz(0.0, pivot_y, 0.0),
                ),
            ],
            by_name: Default::default(),
            roadwheels: Vec::new(),
            collision_proxies: Vec::new(),
            ballistic_volumes: Vec::new(),
        };
        let body = TurretBody {
            mass: spec.turret.mass.0,
            angular_inertia: spec.turret.angular_inertia(),
            centre_of_mass: Vec3::new(0.0, 0.45, -0.45),
        };
        (spec, geometry, body, Vec3::new(0.0, -1.0, 0.0))
    }

    /// The game's damage model: the load gate must refuse a kind invalid only after turning.
    /// Dropping positional subtraction or its between-sample reach bound, checking only rest,
    /// or using the origin as the vehicle centre admits this fixture. The premise uses Avian's
    /// parallel-axis tensor, independently
    /// of the admission walk; Sylvester's minors establish both signs without diagonalization.
    #[test]
    #[should_panic(
        expected = "fixture.tank.ron: mass/inertia_extents full-vehicle box versus turret.mass/inertia_extents"
    )]
    fn an_assembly_valid_at_rest_but_not_when_turned_is_refused() {
        let (mut spec, geometry, mut body, centre) = construction(0.02);
        // Fixture: a 2.1 m cube makes inertia rotation-invariant. Only centroid motion can
        // invalidate the rest-valid model, so deleting the reach bound admits the first cell.
        spec.turret.inertia_extents.0 = (2.1, 2.1, 2.1);
        body.angular_inertia = spec.turret.angular_inertia();
        let mu = body.mass * spec.mass / (spec.mass - body.mass);
        let remaining = |angle| {
            let rotation = Quat::from_rotation_y(angle);
            let d =
                geometry.nodes[1].transform.translation + rotation * body.centre_of_mass - centre;
            spec.hull_angular_inertia().tensor().to_mat3()
                - body
                    .angular_inertia
                    .tensor()
                    .rotated(rotation)
                    .shifted(mu, d)
                    .to_mat3()
        };
        let rest = remaining(0.0);
        assert!(
            rest.x_axis.x > 0.0
                && rest.x_axis.x * rest.y_axis.y - rest.x_axis.y * rest.y_axis.x > 0.0
                && rest.determinant() > 0.0,
            "fixture must be positive-definite at rest"
        );
        assert!(
            remaining(std::f32::consts::FRAC_PI_2).z_axis.z < 0.0,
            "fixture must be invalid when turned"
        );
        admit_detachment("fixture.tank.ron", &spec, &geometry, 1, centre, body);
    }

    /// The game's damage model: coherent box/assembly data must load even when independently
    /// worst directions and triangle reach would reject it. Fixture: pivot 0.85 m above centre.
    #[test]
    fn a_coherent_assembly_is_admitted_over_a_full_turn() {
        let (spec, geometry, body, centre) = construction(-0.15);
        admit_detachment("fixture.tank.ron", &spec, &geometry, 1, centre, body);
    }

    /// The game's damage model: only the construction's actual travel can invalidate a kind.
    /// Fixture: the otherwise invalid full-turn construction, limited to ±5° around valid rest.
    #[test]
    fn a_limited_assembly_does_not_have_to_survive_unreachable_angles() {
        let (mut spec, geometry, body, centre) = construction(0.02);
        spec.servos.insert(
            "FixtureYaw".into(),
            ServoSpec::test_limited(ServoRole::Yaw, 1.0, -5.0, 5.0),
        );
        admit_detachment("fixture.tank.ron", &spec, &geometry, 1, centre, body);
    }

    /// The game's damage model: a second allowed axis can invalidate an otherwise coherent
    /// assembly. Fixture: the valid full-yaw construction with a nested ±45° pitch joint.
    /// Ignoring a joint or replacing its axis with yaw admits a body the launch cannot split.
    #[test]
    #[should_panic(
        expected = "fixture.tank.ron: mass/inertia_extents full-vehicle box versus turret.mass/inertia_extents"
    )]
    fn admission_covers_nested_servo_axes() {
        let (mut spec, mut geometry, body, centre) = construction(-0.15);
        spec.servos.insert(
            "FixturePitch".into(),
            ServoSpec::test_limited(ServoRole::Pitch, 1.0, -45.0, 45.0),
        );
        geometry.nodes.push(NodeGeometry {
            name: "FixturePitch".into(),
            parent: Some(1),
            transform: Transform::IDENTITY,
            root_position: Vec3::ZERO,
            root_rotation: Quat::IDENTITY,
            root_scale: Vec3::ONE,
            primitives: Vec::new(),
        });
        admit_detachment("fixture.tank.ron", &spec, &geometry, 2, centre, body);
    }
}
