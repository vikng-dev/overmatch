//! The game's damage model: collider poses at successive walk boundaries.
//!
//! Avian's `PhysicsInterpolationPlugin` uses Bevy's transform easing states, which hold render
//! transforms only for opted-in entities and are disabled by `net::physics::physics_plugins`.
//! Avian's solver has no public previous collider pose. This history therefore retains the
//! composed physics pose seen by the walk, independently of rendering or frame count.

use avian3d::prelude::{
    Collider, ColliderOf, ColliderTransform, Position, RigidBody, Rotation, SimpleCollider,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pose {
    pub position: Vec3,
    pub rotation: Quat,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Poses {
    pub from: Pose,
    pub to: Pose,
}

/// The game's damage model: membership is frozen at a walk boundary. A new collider has no
/// earlier trajectory and is born with identical endpoints; a removed collider leaves the table.
#[derive(Resource, Default)]
pub(super) struct ColliderMotion {
    pub colliders: HashMap<Entity, Poses>,
    pub bodies: HashMap<Entity, Poses>,
    /// DERIVED conservative displacement of any collider point, used to grow a tree query.
    pub largest_motion: f32,
}

type Colliders<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Position,
        &'static Rotation,
        Ref<'static, Collider>,
        Option<&'static ColliderOf>,
        Option<&'static ColliderTransform>,
    ),
>;

fn composed_pose(
    entity: Entity,
    position: &Position,
    rotation: &Rotation,
    owner: Option<&ColliderOf>,
    local: Option<&ColliderTransform>,
    bodies: &Query<(&Position, &Rotation)>,
) -> Pose {
    if let Some(owner) = owner.filter(|owner| owner.body != entity) {
        let (body_position, body_rotation) = bodies
            .get(owner.body)
            .expect("game's damage model: a collider's body has a physics pose");
        let local = local.expect("ColliderOf requires ColliderTransform");
        Pose {
            position: body_position.0 + body_rotation.0 * local.translation,
            rotation: (body_rotation.0 * local.rotation.0).normalize(),
        }
    } else {
        Pose {
            position: position.0,
            rotation: rotation.0,
        }
    }
}

/// The game's damage model: a collider born at a boundary starts at its birth pose, before
/// physics can move it. A replaced shape is new geometry at that same boundary.
pub(super) fn seed_births(
    mut history: ResMut<ColliderMotion>,
    colliders: Colliders,
    bodies: Query<(&Position, &Rotation)>,
    roots: Query<(Entity, &Position, &Rotation), With<RigidBody>>,
) {
    for (entity, position, rotation) in &roots {
        let pose = Pose {
            position: position.0,
            rotation: rotation.0,
        };
        history.bodies.entry(entity).or_insert(Poses {
            from: pose,
            to: pose,
        });
    }
    for (entity, position, rotation, collider, owner, local) in &colliders {
        if collider.is_changed() || !history.colliders.contains_key(&entity) {
            let pose = composed_pose(entity, position, rotation, owner, local, &bodies);
            history.colliders.insert(
                entity,
                Poses {
                    from: pose,
                    to: pose,
                },
            );
        }
    }
}

/// The game's damage model: refresh child physics poses after the solver moved their body.
/// Avian's own child placement runs before its solver; the walk reads the completed interval.
pub(super) fn place_children(
    mut children: Query<
        (
            &ColliderOf,
            &ColliderTransform,
            &mut Position,
            &mut Rotation,
        ),
        Without<RigidBody>,
    >,
    bodies: Query<(&Position, &Rotation), With<RigidBody>>,
) {
    for (owner, local, mut position, mut rotation) in &mut children {
        let (at, turn) = bodies
            .get(owner.body)
            .expect("game's damage model: every child collider is attached to a posed rigid body");
        position.set_if_neq(Position(at.0 + turn.0 * local.translation));
        rotation.set_if_neq(Rotation((turn.0 * local.rotation.0).normalize()));
    }
}

pub(super) fn snapshot(
    mut history: ResMut<ColliderMotion>,
    colliders: Colliders,
    bodies: Query<(&Position, &Rotation)>,
    roots: Query<(Entity, &Position, &Rotation), With<RigidBody>>,
) {
    let mut next = HashMap::with_capacity(colliders.iter().len());
    let mut largest_motion = 0.0f32;
    for (entity, position, rotation, collider, owner, local) in &colliders {
        let to = composed_pose(entity, position, rotation, owner, local, &bodies);
        // Absence means birth at this boundary, not an invented earlier pose.
        let from = history.colliders.get(&entity).map_or(to, |poses| poses.to);
        let poses = Poses { from, to };
        let displacement = point_motion_bound(poses, &collider);
        largest_motion = largest_motion.max(displacement);
        next.insert(entity, poses);
    }
    history.bodies = roots
        .iter()
        .map(|(entity, position, rotation)| {
            let to = Pose {
                position: position.0,
                rotation: rotation.0,
            };
            let from = history.bodies.get(&entity).map_or(to, |poses| poses.to);
            (entity, Poses { from, to })
        })
        .collect();
    history.colliders = next;
    history.largest_motion = largest_motion;
}

/// The game's damage model: a conservative geometric displacement for any collider point.
/// DERIVED: for unit quaternions, their shorter signed chord `d = 2 sin(theta / 4)` bounds
/// `sin(theta / 2)` from above. Thus `2 r min(d, 1)` bounds every rotational chord, without
/// an inverse cosine whose rounded dot product can erase a small but real turn.
fn point_motion_bound(poses: Poses, collider: &Collider) -> f32 {
    if poses.from == poses.to {
        return 0.0;
    }
    let bounds = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
    let radius = bounds.min.abs().max(bounds.max.abs()).as_dvec3().length();
    let from = poses.from.rotation.to_array().map(f64::from);
    let to = poses.to.rotation.to_array().map(f64::from);
    let difference = from
        .iter()
        .zip(to)
        .map(|(&a, b)| (a - b).powi(2))
        .sum::<f64>();
    let sum = from
        .iter()
        .zip(to)
        .map(|(&a, b)| (a + b).powi(2))
        .sum::<f64>();
    let angular = 2.0 * radius * difference.min(sum).sqrt().min(1.0);
    let linear = poses
        .from
        .position
        .as_dvec3()
        .distance(poses.to.position.as_dvec3());
    let bound = (linear + angular) as f32;
    // Round the geometric bound outward; an equivalent signed quaternion at rest stays exact zero.
    if bound == 0.0 { 0.0 } else { bound.next_up() }
}

impl Pose {
    pub(super) fn local(self, point: Vec3) -> Vec3 {
        self.rotation.inverse() * (point - self.position)
    }
}

impl Poses {
    pub(super) fn at(self, fraction: f32) -> Pose {
        if self.from == self.to {
            return self.to;
        }
        Pose {
            position: self.from.position.lerp(self.to.position, fraction),
            rotation: self.from.rotation.slerp(self.to.rotation, fraction),
        }
    }

    /// The game's damage model: the struck point's velocity on the declared lerp/slerp chord.
    /// Collider origins include child servo and recoil offsets, rather than only body velocity.
    pub(super) fn velocity_at(self, fraction: f32, local: Vec3, dt: f32) -> Vec3 {
        if self.from == self.to {
            return Vec3::ZERO;
        }
        let mut delta = self.to.rotation * self.from.rotation.inverse();
        if delta.w < 0.0 {
            delta = -delta;
        } // The same shortest arc as quaternion slerp.
        let angular = delta.to_scaled_axis() / dt;
        (self.to.position - self.from.position) / dt
            + angular.cross(self.at(fraction).rotation * local)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Sweep {
    pub from: f32,
    pub to: f32,
    pub dt: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ContactMotion {
    pub fraction: f32,
    pub point: Vec3,
    pub velocity: Vec3,
    /// Material already holds the start; contact-time membership must seed its remaining chord.
    pub contained: bool,
}

/// The game's damage model: one collider's local endpoint chord. The collector keeps a unit
/// local axis; its distance is converted back to a shared tick fraction before candidates compete.
#[expect(
    clippy::too_many_arguments,
    reason = "one moving candidate and one world segment"
)]
pub(super) fn sweep_collider(
    poses: Poses,
    sweep: Sweep,
    node: Entity,
    primitive: Entity,
    collider: &Collider,
    surfaces: Option<&super::BallisticSurfaces>,
    start: Vec3,
    end: Vec3,
) -> Option<(ContactMotion, Vec3)> {
    let from = poses.at(sweep.from);
    let to = poses.at(sweep.to);
    let a = from.local(start);
    let b = to.local(end);
    let relative = b - a;
    // With no relative movement there is no new entry. A fixed local probe still reads material
    // already holding the start, without inventing relative motion.
    let axis = Dir3::new(relative).map_or(Vec3::Z, |axis| *axis);
    let distance = relative.length();
    let bounds = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
    let far = (bounds.min - a).abs().max((bounds.max - a).abs()).length();
    let behind = (far + super::MARCH_EPS).next_up();
    let length = (behind + distance.max(behind)).next_up();
    let laws = super::walk::WalkLaws::default();
    let mut hits = Vec::new();
    super::collect::collect_collider(
        &super::collect::Corridor {
            anchor: Vec3::ZERO,
            origin: a,
            axis,
            length,
            behind,
            seeded: &[],
            handoff: None,
            laws: &laws,
        },
        node,
        primitive,
        collider,
        surfaces,
        Position(Vec3::ZERO),
        Rotation::IDENTITY,
        &mut hits,
    )
    .unwrap_or_else(|error| error.panic());
    let contact = super::walk::discovery_entry(
        &hits,
        f64::from(behind),
        super::walk::topology_band(f64::from(a.length()), &laws)
            .min(f64::from(super::MARCH_EPS) * 0.25),
    )?;
    if !contact.contained && (distance == 0.0 || contact.distance > f64::from(distance)) {
        return None;
    }
    let part = if contact.contained {
        0.0
    } else {
        (contact.distance / f64::from(distance)).clamp(0.0, 1.0) as f32
    };
    let fraction = sweep.from + (sweep.to - sweep.from) * part;
    let pose = poses.at(fraction);
    let local = a + relative * part;
    Some((
        ContactMotion {
            fraction,
            point: if contact.contained {
                start
            } else {
                pose.position + pose.rotation * local
            },
            velocity: poses.velocity_at(fraction, local, sweep.dt),
            contained: contact.contained,
        },
        pose.rotation * contact.normal,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's damage model: the collider-local chord finds an overtaking piece as an
    /// ordinary entering face. A world-only ray misses it; subsequent discovery relies on this.
    #[test]
    fn a_faster_piece_meets_the_round_from_behind() {
        let collider = Collider::cuboid(2.0, 2.0, 0.1);
        let poses = Poses {
            from: Pose {
                position: Vec3::NEG_Z,
                rotation: Quat::IDENTITY,
            },
            to: Pose {
                position: Vec3::Z * 19.0,
                rotation: Quat::IDENTITY,
            },
        };
        let (contact, normal) = sweep_collider(
            poses,
            Sweep {
                from: 0.0,
                to: 1.0,
                dt: 0.015625,
            },
            Entity::PLACEHOLDER,
            Entity::PLACEHOLDER,
            &collider,
            None,
            Vec3::ZERO,
            Vec3::Z * 12.5,
        )
        .expect("overtaking contact");
        assert!((contact.fraction - 0.95 / 7.5).abs() < 1e-6);
        assert_eq!(contact.velocity, Vec3::Z * 1280.0);
        assert_eq!(normal, Vec3::Z);
    }

    /// The game's damage model: material holding a shallow start contacts immediately. The
    /// caller seeds its remaining chord; the authority must neither skip it nor panic.
    #[test]
    fn discovery_contacts_a_shallow_inside_start() {
        let pose = Pose {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        };
        let (contact, _) = sweep_collider(
            Poses {
                from: pose,
                to: pose,
            },
            Sweep {
                from: 0.0,
                to: 1.0,
                dt: 0.015625,
            },
            Entity::PLACEHOLDER,
            Entity::PLACEHOLDER,
            &Collider::cuboid(1.0, 1.0, 1.0),
            None,
            Vec3::new(0.0, 0.0, 0.4999),
            Vec3::NEG_Z,
        )
        .expect("material holds the start");
        assert_eq!(contact.fraction, 0.0);
        assert!(contact.contained);
    }

    /// The game's damage model: an occupied start at an exhausted tick boundary still contacts
    /// at that boundary. Its zero-length sweep must not divide by an empty interval, and the
    /// authority's seeded walk must still see the material. The cube is fixture geometry.
    #[test]
    fn contained_discovery_at_an_exhausted_boundary_has_a_finite_contact() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = super::super::march_tests::world_with_trimesh_plates(&[(
            Vec3::new(4.0, 4.0, 0.05),
            Vec3::new(0.0, 2.0, 0.0),
        )]);
        app.world_mut()
            .run_system_once(|world: super::super::ProjectileMarchWorld| {
                world.sweep.set(Some(Sweep {
                    from: 1.0,
                    to: 1.0,
                    dt: 0.015625,
                }));
                let at = Vec3::new(0.0, 2.0, 0.0);
                let Some(super::super::SegmentHit::Armor(hit)) = super::super::cast_disc_segment(
                    &world,
                    &[Vec3::ZERO],
                    at,
                    Dir3::Z,
                    0.0,
                    &|_| true,
                ) else {
                    panic!("material holds the start")
                };
                assert_eq!(hit.distance, 0.0);
                let contact = world.contact_motion.get().unwrap();
                assert_eq!(contact.fraction, 1.0);
                assert_eq!(contact.point, at);
                assert!(contact.contained);
            })
            .unwrap();
    }

    /// The game's damage model: static discovery retains the original Avian hit distance and
    /// normal bits. The mechanism relies on this cutover changing no static contact arithmetic.
    #[test]
    fn static_discovery_keeps_the_original_contact_bits() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = super::super::march_tests::world_with_trimesh_plates(&[(
            Vec3::new(4.0, 4.0, 0.05),
            Vec3::new(0.0, 2.0, 0.0),
        )]);
        app.init_resource::<ColliderMotion>();
        app.world_mut().run_system_once(snapshot).unwrap();
        app.world_mut()
            .run_system_once(|world: super::super::ProjectileMarchWorld| {
                for x in [0.0, 0.99, 1.99, 2.02] {
                    let origin = Vec3::new(x, 2.0, 5.0);
                    let offsets = super::super::resolve::layout(
                        &super::super::walk::DiscFrame::anchored(Vec3::NEG_Z).unwrap(),
                        0.044,
                    );
                    let mut reference = None;
                    for offset in &offsets {
                        if let Some(hit) = world.spatial.cast_ray_predicate(
                            origin + *offset,
                            Dir3::NEG_Z,
                            12.5,
                            true,
                            &avian3d::prelude::SpatialQueryFilter::from_mask(crate::Layer::Armor),
                            &|_| true,
                        ) && reference.is_none_or(|best: avian3d::prelude::RayHitData| {
                            hit.distance < best.distance
                        }) {
                            reference = Some(hit);
                        }
                    }
                    world.sweep.set(Some(Sweep {
                        from: 0.0,
                        to: 1.0,
                        dt: 0.015625,
                    }));
                    let found = super::super::cast_disc_segment(
                        &world,
                        &offsets,
                        origin,
                        Dir3::NEG_Z,
                        12.5,
                        &|_| true,
                    );
                    match (reference, found) {
                        (Some(expected), Some(super::super::SegmentHit::Armor(actual))) => {
                            assert_eq!(actual.entity, expected.entity);
                            assert_eq!(actual.distance.to_bits(), expected.distance.to_bits());
                            assert_eq!(
                                actual.normal.to_array().map(f32::to_bits),
                                expected.normal.to_array().map(f32::to_bits)
                            );
                        }
                        (None, None) => {}
                        _ => panic!("static discovery changed contact presence"),
                    }
                }
            })
            .unwrap();
    }

    /// The game's damage model: angular contact velocity follows the same shortest arc as
    /// posing, including equivalent signed endpoint quaternions. Relative potency relies on it.
    #[test]
    fn point_velocity_uses_the_posing_arc_and_local_lever() {
        let poses = Poses {
            from: Pose {
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
            },
            to: Pose {
                position: Vec3::X * 2.0,
                rotation: -Quat::from_rotation_z(0.5),
            },
        }; // Fixture displacement 2 m, turn 0.5 rad, duration 1 s.
        let lever = poses.at(0.5).rotation * Vec3::X;
        let expected = Vec3::X * 2.0 + Vec3::Z.cross(lever) * 0.5;
        assert!(poses.velocity_at(0.5, Vec3::X, 1.0).distance(expected) < 1e-6);
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<ColliderMotion>()
            .add_systems(Update, snapshot);
        app
    }

    /// The game's damage model: the walk needs successive physics poses even when rendering
    /// never runs. Mutant: reading the child's stale Position loses body, servo and recoil motion.
    #[test]
    fn history_composes_body_and_child_motion_and_expires_removed_colliders() {
        let mut app = app();
        let body = app
            .world_mut()
            .spawn((Position(Vec3::ZERO), Rotation::IDENTITY))
            .id();
        let child = app
            .world_mut()
            .spawn((
                Position(Vec3::ZERO),
                Rotation::IDENTITY,
                Collider::cuboid(1.0, 1.0, 1.0),
                ColliderOf { body },
                ColliderTransform {
                    translation: Vec3::X,
                    ..default()
                },
            ))
            .id();
        app.update();
        let birth = app.world().resource::<ColliderMotion>().colliders[&child];
        assert_eq!(birth.from, birth.to);
        assert_eq!(birth.to.position, Vec3::X);
        let body_turn = Quat::from_rotation_y(0.5);
        app.world_mut()
            .entity_mut(body)
            .insert((Position(Vec3::Z), Rotation(body_turn)));
        let local_turn = Quat::from_rotation_x(0.25);
        app.world_mut().entity_mut(child).insert(ColliderTransform {
            translation: Vec3::X * 2.0,
            rotation: Rotation(local_turn),
            ..default()
        });
        app.update();
        let history = app.world().resource::<ColliderMotion>();
        let moved = history.colliders[&child];
        assert_eq!(moved.from, birth.to);
        assert_eq!(moved.to.position, Vec3::Z + body_turn * (Vec3::X * 2.0));
        assert_eq!(moved.to.rotation, (body_turn * local_turn).normalize());
        assert!(history.largest_motion >= moved.from.position.distance(moved.to.position));
        app.world_mut().despawn(child);
        app.update();
        assert!(
            app.world()
                .resource::<ColliderMotion>()
                .colliders
                .is_empty()
        );
    }

    /// The game's damage model: static geometry must add no broad-phase padding. The walk
    /// relies on exact identity motion to keep the static collector's arithmetic unchanged.
    #[test]
    fn static_endpoints_and_motion_are_bit_exact() {
        let mut app = app();
        let collider = app
            .world_mut()
            .spawn((
                Position(Vec3::new(100.0, 20.0, -40.0)),
                Rotation(Quat::from_rotation_y(0.7)),
                Collider::cuboid(2.0, 3.0, 4.0),
            ))
            .id();
        app.update();
        app.update();
        let history = app.world().resource::<ColliderMotion>();
        let poses = history.colliders[&collider];
        assert_eq!(poses.from, poses.to);
        assert_eq!(history.largest_motion.to_bits(), 0.0f32.to_bits());
    }
    /// The game's damage model: a body born at the boundary may move in its first physics
    /// interval. The walk must retain its birth endpoint, rather than inventing a stationary birth.
    #[test]
    fn a_new_collider_keeps_its_birth_pose_through_its_first_physics_interval() {
        let mut app = app();
        app.add_systems(PreUpdate, seed_births);
        let entity = app
            .world_mut()
            .spawn((
                Position(Vec3::ZERO),
                Rotation::IDENTITY,
                Collider::cuboid(1.0, 1.0, 1.0),
            ))
            .id();
        app.world_mut().run_schedule(PreUpdate);
        app.world_mut().entity_mut(entity).insert(Position(Vec3::X));
        app.world_mut().run_schedule(Update);
        let poses = app.world().resource::<ColliderMotion>().colliders[&entity];
        assert_eq!(poses.from.position, Vec3::ZERO);
        assert_eq!(poses.to.position, Vec3::X);
    }

    /// The game's damage model: the server's walk sees completed physics; topology and
    /// publication see all its consequences. Removing any edge makes that consumer order undefined.
    #[test]
    fn the_walk_and_all_consumers_have_one_post_physics_order() {
        use crate::state::SimPhase;
        use bevy::ecs::schedule::{NodeId, SystemSet};
        let mut app = App::new();
        app.add_plugins(super::super::sim_plugin);
        let schedules = app.world().resource::<Schedules>();
        let graph = schedules.get(FixedPostUpdate).unwrap().graph();
        let sets = [
            avian3d::prelude::PhysicsSystems::Writeback.intern(),
            SimPhase::ColliderPoses.intern(),
            SimPhase::ProjectileMarch.intern(),
            crate::damage::DamageConsequences.intern(),
            SimPhase::RecordedConsequences.intern(),
            crate::buildings::breakage::WallLawSet.intern(),
            crate::trees::TreeFallSet.intern(),
            crate::trees::TreeDerived.intern(),
            SimPhase::Publish.intern(),
        ];
        for pair in sets.windows(2) {
            let from = graph.system_sets.get_key(pair[0]).unwrap();
            let to = graph.system_sets.get_key(pair[1]).unwrap();
            assert!(
                graph
                    .dependency()
                    .graph()
                    .contains_edge(NodeId::Set(from), NodeId::Set(to)),
                "post-physics consumer order is explicit: {:?} before {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    /// The game's damage model: tree-query padding must retain small rotations that an f32
    /// quaternion dot rounds away. The broad phase relies on every swept point fitting this bound.
    #[test]
    fn the_motion_bound_keeps_small_turns_and_contains_the_rotational_sweep() {
        // Fixture dimensions, turns and translations, independent of any authored body.
        let collider = Collider::cuboid(2.0, 4.0, 6.0);
        for angle in [1.0e-4, 0.25, std::f32::consts::PI] {
            for translation in [Vec3::ZERO, Vec3::new(0.3, -0.4, 0.5)] {
                let poses = Poses {
                    from: Pose {
                        position: Vec3::ZERO,
                        rotation: Quat::IDENTITY,
                    },
                    to: Pose {
                        position: translation,
                        rotation: Quat::from_rotation_y(angle),
                    },
                };
                let bound = point_motion_bound(poses, &collider);
                assert!(bound > 0.0, "a real turn has nonzero displacement");
                for x in [-1.0, 1.0] {
                    for y in [-2.0, 2.0] {
                        for z in [-3.0, 3.0] {
                            let local = Vec3::new(x, y, z);
                            for step in 0..=32 {
                                let fraction = step as f32 / 32.0;
                                let at = poses.from.position.lerp(poses.to.position, fraction)
                                    + poses.from.rotation.slerp(poses.to.rotation, fraction)
                                        * local;
                                assert!(
                                    at.distance(local) <= bound,
                                    "every swept fixture corner stays within query padding: {angle}, {translation}, {fraction}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
