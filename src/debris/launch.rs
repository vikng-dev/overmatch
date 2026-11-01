//! Free-body momentum exchange in the game's environment physics (game state only).
//! Walls and trunks supply geometry and loads; this module owns their common plastic launch,
//! coupled energy bound and give-back. The wall design's "Bodies in contact" owns the derivation.

use crate::buildings::fracture::{Load, Origin};
use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::{
    ComputeMassProperties3d, MassProperties3d,
};
use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;
use std::collections::BTreeMap;

/// Momentum the rigid structure took beyond a departing chunk's plastic exchange: the source
/// body, contact point (m) and returned impulse (N·s), in the building's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BodyReturn {
    pub(crate) body: Entity,
    pub(crate) at: Vec3,
    pub(crate) impulse: Vec3,
}

impl BodyReturn {
    pub(crate) fn of(load: &Load, launched: Vec3) -> Option<Self> {
        match load.origin {
            Origin::Field => None,
            Origin::Body { body, .. } if load.blow != Vec3::ZERO => Some(Self {
                body,
                at: load.at,
                impulse: load.blow - launched,
            }),
            Origin::Body { .. } => None,
        }
    }
}

/// One freed group's collider frame and departure in building terms, and the source bodies'
/// give-back. The mass and inertia are those of these exact colliders at their data's density.
#[derive(Debug)]
pub(crate) struct Launch {
    pub(crate) centroid: Vec3,
    pub(crate) colliders: Vec<(usize, Collider)>,
    pub(crate) velocity: Vec3,
    pub(crate) spin: Vec3,
    pub(crate) returns: Vec<BodyReturn>,
    pub(crate) mass: f32,
    pub(crate) inertia: Mat3,
    pub(crate) body_velocity: Vec3,
    pub(crate) body_spin: Vec3,
    pub(crate) exchanges: Vec<(Load, Vec3)>,
}

/// The shared free-body launch, from the exact occupied colliders in a frame about `centroid`.
/// Material density supplies mass; each load transfers its field impulse or plastic body share.
pub(crate) fn launch_colliders(
    centroid: Vec3,
    colliders: Vec<(usize, Collider)>,
    density: impl Fn(usize) -> f32,
    loads: &[Load],
) -> Launch {
    let total = colliders
        .iter()
        .map(|(c, shape)| shape.mass_properties(density(*c)))
        .fold(MassProperties3d::default(), |sum, part| sum + part);
    assert!(total.mass > 0.0, "a freed group has occupied mass");
    let inverse_inertia = total.angular_inertia_tensor().inverse();
    let com = centroid + total.center_of_mass;
    let mut impulse = Vec3::ZERO;
    let mut angular = Vec3::ZERO;
    let mut returns = Vec::new();
    let mut body_impulse = Vec3::ZERO;
    let mut body_angular = Vec3::ZERO;
    let mut exchanges = Vec::new();
    for load in loads {
        // A press, or a field carrying only crushing energy, launches no momentum.
        if load.blow == Vec3::ZERO {
            continue;
        }
        let arm = (load.at - com).cross(load.blow.as_dvec3().normalize().as_vec3());
        let inverse_mass = 1.0 / total.mass + arm.dot(inverse_inertia * arm);
        let launched = load.blow * load.origin.share(inverse_mass);
        impulse += launched;
        angular += (load.at - com).cross(launched);
        if let Some(returned) = BodyReturn::of(load, launched) {
            body_impulse += launched;
            body_angular += (load.at - com).cross(launched);
            returns.push(returned);
            exchanges.push((*load, launched));
        }
    }
    Launch {
        centroid,
        colliders,
        velocity: impulse / total.mass,
        spin: inverse_inertia * angular,
        returns,
        mass: total.mass,
        inertia: total.angular_inertia_tensor().to_mat3(),
        body_velocity: body_impulse / total.mass,
        body_spin: inverse_inertia * body_angular,
        exchanges,
    }
}

/// Cap simultaneous plastic exchanges by their common velocity along the combined impulse.
/// The shares from `launch` fix each point's relative impulse. DERIVED: scaling them by `s`
/// changes total kinetic energy by `-s W + s² Q/2`; its minimum is `s = W/Q`. `W` is the
/// source work less the carrier's field-driven motion, and `Q` sums every carrier's and every
/// source body's combined linear/angular response. Counting a source per point would miss the
/// cross terms and can create energy. An isolated contact already has this plastic minimum.
pub(crate) fn cap_exchanges(
    departures: &mut [(&Transform, &mut Launch)],
    inertia_of: impl Fn(Entity) -> crate::contact_transfer::BodyInertia,
) {
    // Carriers couple only through a shared source. Disjoint collisions must not change one
    // another's exchange, even when the reader queued them in the same physics deposit.
    let mut parent: Vec<usize> = (0..departures.len()).collect();
    let root = |parent: &[usize], mut index: usize| {
        while parent[index] != index {
            index = parent[index];
        }
        index
    };
    let mut owner = BTreeMap::new();
    for (index, departure) in departures.iter().enumerate() {
        for (load, _) in &departure.1.exchanges {
            let Origin::Body { body, .. } = load.origin else {
                unreachable!("only body blows have a source exchange");
            };
            if let Some(other) = owner.insert(body, index) {
                let (a, b) = (root(&parent, index), root(&parent, other));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, departure) in departures.iter().enumerate() {
        if !departure.1.exchanges.is_empty() {
            components
                .entry(root(&parent, index))
                .or_default()
                .push(index);
        }
    }
    for component in components.values() {
        cap_connected_exchanges(departures, component, &inertia_of);
    }
}

/// Bound one connected set of source bodies and carriers, with all source cross terms retained.
fn cap_connected_exchanges(
    departures: &mut [(&Transform, &mut Launch)],
    component: &[usize],
    inertia_of: &impl Fn(Entity) -> crate::contact_transfer::BodyInertia,
) {
    let mut sources: BTreeMap<Entity, (DVec3, DVec3)> = BTreeMap::new();
    let (mut work, mut quadratic) = (0.0_f64, 0.0_f64);
    for departure in component.iter().map(|&index| &departures[index]) {
        let launch = &*departure.1;
        let field_velocity = launch.velocity - launch.body_velocity;
        let field_spin = launch.spin - launch.body_spin;
        let body_velocity = launch.body_velocity.as_dvec3();
        let body_spin = launch.body_spin.as_dvec3();
        let inertia = launch.inertia.as_dmat3();
        work -= f64::from(launch.mass) * field_velocity.as_dvec3().dot(body_velocity)
            + field_spin.as_dvec3().dot(inertia * body_spin);
        quadratic += f64::from(launch.mass) * body_velocity.length_squared()
            + body_spin.dot(inertia * body_spin);
        for (load, impulse) in &launch.exchanges {
            let Origin::Body { body, speed, .. } = load.origin else {
                unreachable!("only body blows have a source exchange");
            };
            let inertia = inertia_of(body);
            let at = departure.0.translation + departure.0.rotation * load.at;
            let impulse_world = (departure.0.rotation * *impulse).as_dvec3();
            let source = sources.entry(body).or_default();
            source.0 += impulse_world;
            source.1 += (at - inertia.centre).as_dvec3().cross(impulse_world);
            work += impulse.as_dvec3().length() * f64::from(speed);
        }
    }
    for (body, (impulse, angular)) in sources {
        let inertia = inertia_of(body);
        quadratic += impulse.dot(inertia.inverse_mass.as_dvec3() * impulse)
            + angular.dot(inertia.inverse_inertia.as_dmat3() * angular);
    }
    assert!(
        work.is_finite() && quadratic.is_finite() && quadratic >= 0.0,
        "finite contact velocities, impulses and occupied carrier mass give a finite nonnegative response"
    );
    // DERIVED: squaring an f32 impulse in f64 retains even its smallest subnormal. A launch
    // rounded wholly to zero has no exchange to scale; return its source's blow whole.
    let scale = if quadratic > 0.0 {
        (work / quadratic).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    for &index in component {
        let launch = &mut *departures[index].1;
        launch.velocity += launch.body_velocity * (scale - 1.0);
        launch.spin += launch.body_spin * (scale - 1.0);
        for (returned, (load, impulse)) in launch.returns.iter_mut().zip(&launch.exchanges) {
            returned.impulse = load.blow - *impulse * scale;
        }
    }
}

/// A newly freed body's exchange, finalized once all structures have answered this step.
#[derive(Component)]
pub(crate) struct PendingLaunch {
    frame: Transform,
    motion: Option<Launch>,
}

impl PendingLaunch {
    pub(crate) fn new(frame: Transform, motion: Launch) -> Self {
        Self {
            frame,
            motion: Some(motion),
        }
    }
}

/// Every simultaneous departure shares one budget, including a source touching walls and trees.
pub(crate) struct LaunchPlugin;

impl Plugin for LaunchPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedPostUpdate,
            finish_launches
                .after(crate::buildings::breakage::WallLawSet)
                .after(crate::trees::TreeDerived)
                .before(crate::state::SimPhase::Publish),
        );
    }
}

fn finish_launches(
    mut pending: Query<(Entity, &mut PendingLaunch)>,
    transfers: Res<crate::contact_transfer::ContactTransfers>,
    mut bodies: Query<(Forces, &RigidBody)>,
    mut commands: Commands,
) {
    let mut departures: Vec<_> = pending
        .iter_mut()
        .map(|(entity, mut pending)| {
            (
                entity,
                pending.frame,
                pending
                    .motion
                    .take()
                    .expect("a pending launch is finalized once before its component is removed"),
            )
        })
        .collect();
    let mut motions: Vec<_> = departures
        .iter_mut()
        .map(|(_, frame, motion)| (&*frame, motion))
        .collect();
    cap_exchanges(&mut motions, |body| transfers.inertia(body));
    for (entity, frame, motion) in departures {
        for returned in motion.returns {
            let (mut source, kind) = bodies
                .get_mut(returned.body)
                .expect("a contact's dynamic source survives the same step's structural laws");
            assert!(kind.is_dynamic(), "a contact source is dynamic");
            source.apply_linear_impulse_at_point(
                frame.rotation * returned.impulse,
                frame.translation + frame.rotation * returned.at,
            );
        }
        commands
            .entity(entity)
            .insert((
                LinearVelocity(frame.rotation * motion.velocity),
                AngularVelocity(frame.rotation * motion.spin),
            ))
            .remove::<PendingLaunch>();
    }
}
