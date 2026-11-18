//! Moving outer shoe faces for the game's driving physics. Hard contacts stay in Avian;
//! their delivered friction returns to the transmission on the next driving tick.

use std::collections::BTreeMap;
use std::sync::Arc;

use avian3d::prelude::{
    ActiveCollisionHooks, ColliderOf, CollisionHooks, ContactPair, Position, RigidBody, Rotation,
    Sleeping,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::forces::LinkRise;
use super::oracle::ContactOracle;
use super::side::Side;
use super::sim::TrackRise;
use super::terrain::{FieldSurface, TrackField};
use crate::contact_transfer::ContactTransfers;

/// Spawn-built surface kinematics. A side slice supplies only lateral contact across the
/// suspension band; the compressed outer outline owns every radial hard contact.
#[derive(Component, Clone, Debug)]
#[require(ActiveCollisionHooks::MODIFY_CONTACTS)]
pub(crate) struct BeltFace {
    pub(crate) side: Side,
    outline: Arc<[Vec2]>,
    bears: Bearing,
}

/// Which of its collider's contacts a moving face bears.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Bearing {
    /// Every contact: a stop or a run.
    Whole,
    /// A side slice: only contact leaving through its one exposed face, the shoe's true lateral
    /// face, which looks along this sign of the hull's x. Its other lateral face lies inside the
    /// shoe's width and is no surface: matter that reaches it came in through the belt's outer
    /// face, which the stop and the sampled support own.
    Exposed(f32),
}

/// Resolution: tolerance for unit-normal components after hull-frame rotation. It separates
/// lateral planes from radial planes without amplifying roundoff into a belt tangent.
const LATERAL_NORMAL_MIN_X: f32 = 1.0 - 1e-4;

/// Resolution: a slice's contact on the top edge of matter its belt rose over lies at that rise,
/// to the roundoff of world points carried into the hull frame (DERIVED: an f32 ulp is 0.12 mm a
/// kilometre from the origin, and each depth takes a few roundings). A millimetre keeps that edge
/// in the vacated band and is no feature a shoe edge shows.
const VACATED_SLACK_M: f32 = 1e-3;

impl BeltFace {
    pub(crate) fn new(side: Side, mut outline: Vec<Vec2>, bears: Bearing) -> Self {
        let first = *outline
            .first()
            .expect("a certified outer shoe outline has points");
        if outline.last() != Some(&first) {
            outline.push(first);
        }
        let area: f32 = outline.windows(2).map(|w| w[0].perp_dot(w[1])).sum();
        assert!(
            area.is_finite() && area != 0.0,
            "a certified outer shoe outline encloses finite area"
        );
        if area < 0.0 {
            outline.reverse();
        }
        Self {
            side,
            outline: outline.into(),
            bears,
        }
    }

    /// The represented contact surface in the hull frame, shared by solver and sweep witnesses.
    pub(crate) fn accepts_normal(&self, outward: Vec3) -> bool {
        match self.bears {
            Bearing::Whole => true,
            Bearing::Exposed(sign) => outward.x * sign >= LATERAL_NORMAL_MIN_X,
        }
    }

    /// Whether the face is at hull-frame `point`, given its side's `links` from the last driving
    /// pass. A side slice reaches down to the rest belt's outer face, but where the matter under
    /// its shoe edge stands inside that face the belt has risen out of the band between them: the
    /// slice's contact there is the matter the belt rides over, which the stop and the sampled
    /// support own. The nearest link's rise for the slice's edge bounds that band.
    pub(crate) fn stands_at(&self, links: &[LinkRise], point: Vec3) -> bool {
        let Bearing::Exposed(sign) = self.bears else {
            return true; // A stop or a run is rigid at its own outline.
        };
        // The outboard face looks away from the hull's centre, along its side's sign.
        let edge = usize::from(sign * self.side.sign() > 0.0);
        let p = Vec2::new(point.z, point.y);
        let nearest = links.iter().min_by(|a, b| {
            let distance = |link: &LinkRise| {
                let [a, b] = link.face;
                let run = b - a;
                (a + run * ((p - a).dot(run) / run.length_squared()).clamp(0.0, 1.0))
                    .distance_squared(p)
            };
            distance(a).total_cmp(&distance(b))
        });
        let Some(link) = nearest else {
            return true; // No driving pass has measured matter under this belt: none has risen.
        };
        let rise = link.rise[edge];
        let [a, b] = link.face;
        let along = (b - a).normalize();
        let depth = (a - p).dot(Vec2::new(along.y, -along.x));
        rise <= 0.0 || depth > rise + VACATED_SLACK_M
    }

    /// The moving face's world-space surface velocity (m/s) at the manifold centre.
    pub(crate) fn velocity(
        &self,
        centre: Vec3,
        outward: Vec3,
        pose: (Vec3, Quat),
        speed: f32,
    ) -> Vec3 {
        let (position, rotation) = pose;
        rotation
            * self.direction(
                rotation.inverse() * (centre - position),
                rotation.inverse() * outward,
            )
            * speed
    }

    /// Positive belt travel in the hull frame. Radial contact follows its plane's tangent;
    /// a lateral face reads the closest edge of its closed outer outline in `(z, y)`.
    pub(crate) fn direction(&self, point: Vec3, outward: Vec3) -> Vec3 {
        if outward.x.abs() < LATERAL_NORMAL_MIN_X {
            return outward.cross(Vec3::X).normalize();
        }
        let p = Vec2::new(point.z, point.y);
        let (_, tangent) = self
            .outline
            .windows(2)
            .filter_map(|w| {
                let edge = w[1] - w[0];
                let length_sq = edge.length_squared();
                if length_sq == 0.0 {
                    return None; // Route joins may repeat an endpoint; it has no tangent.
                }
                let nearest = w[0] + edge * ((p - w[0]).dot(edge) / length_sq).clamp(0.0, 1.0);
                Some((nearest.distance_squared(p), edge / length_sq.sqrt()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .expect("a finite-area outer outline contains a nonzero edge");
        Vec3::new(0.0, tangent.y, tangent.x)
    }
}

/// Avian 0.8 permits one hook set. The construction sweep judges speculative points first;
/// surviving shoe contacts carry the field's grip. Substep preparation owns surface velocity.
#[derive(SystemParam)]
pub(crate) struct WorldContacts<'w, 's> {
    sweep: crate::construction::breakage::SweepContacts<'w, 's>,
    faces: Query<'w, 's, (&'static BeltFace, &'static ColliderOf)>,
    belts: Query<
        'w,
        's,
        (
            &'static TrackRise,
            &'static Position,
            &'static Rotation,
            Has<Sleeping>,
        ),
    >,
    bodies: Query<'w, 's, (&'static RigidBody, Has<Sleeping>)>,
    surfaces: Query<'w, 's, (), With<FieldSurface>>,
    field: Option<Res<'w, TrackField>>,
}

impl CollisionHooks for WorldContacts<'_, '_> {
    fn modify_contacts(&self, contacts: &mut ContactPair, commands: &mut Commands) -> bool {
        if !self.sweep.modify_contacts(contacts, commands) {
            return false;
        }
        // Avian's relative surface velocity is collider 2 minus collider 1.
        for (collider, other, body, sign) in [
            (contacts.collider1, contacts.collider2, contacts.body2, -1.0),
            (contacts.collider2, contacts.collider1, contacts.body1, 1.0),
        ] {
            let Ok((face, of)) = self.faces.get(collider) else {
                continue; // Ordinary collider: its surface stands still on its body.
            };
            let (rise, position, rotation, resting) = self
                .belts
                .get(of.body)
                .expect("a moving shoe face hangs on its spawn-built driving body");
            // A resting tank's band may be stale (`TrackRise`). Past the construction sweep's
            // filter, the hook runs for each fresh candidate (recycling is disabled by the shared
            // physics policy). NarrowPhaseSystems::Update's ParallelCommands flush at the automatic
            // ApplyDeferred sync point before PhysicsStepSystems::Solver: removing Sleeping
            // queues WakeBody through its on-remove hook, waking the island before solver bodies
            // and constraints prepare, whatever face filtering keeps.
            let moves = |body: Entity| {
                let (body, rests) = self.bodies.get(body).expect(
                    "a contact's body keeps its `RigidBody`: Avian drops its pairs with it",
                );
                !body.is_static() && !rests
            };
            if resting && body.is_some_and(moves) {
                commands.entity(of.body).remove::<Sleeping>();
            }
            contacts
                .manifolds
                .retain(|m| face.accepts_normal(rotation.0.inverse() * (-sign * m.normal)));
            let links = &rise.0[face.side.index()];
            for manifold in &mut contacts.manifolds {
                (manifold.points).retain(|p| {
                    face.stands_at(links, rotation.0.inverse() * (p.point - position.0))
                });
            }
            contacts.manifolds.retain(|m| !m.points.is_empty());
            for manifold in &mut contacts.manifolds {
                let centre = manifold.points.iter().map(|p| p.point).sum::<Vec3>()
                    / manifold.points.len() as f32;
                if self.surfaces.contains(other) {
                    manifold.friction = self
                        .field
                        .as_deref()
                        .and_then(|field| field.field.as_ref())
                        .expect("field contact exists only after its ground field is published")
                        .ground_at(centre)
                        .expect("the ground field states material at every field contact")
                        .grip;
                }
            }
        }
        !contacts.manifolds.is_empty()
    }
}

/// One prepared constraint, owned by the lower Entity of its belt owners.
struct SurfaceConstraint {
    color: usize,
    index: usize,
    faces: Vec<(Entity, Side, Vec3)>,
}

/// Tank-owned lists rebuilt after PrepareContactConstraints, including the overflow color.
#[derive(Resource, Default)]
pub(crate) struct BeltConstraints {
    owners: BTreeMap<Entity, Vec<SurfaceConstraint>>,
    order: Vec<(Entity, usize)>,
}

/// Avian 0.8 retains geometry and constraint indices throughout its substep loop
/// (`solver/plugin.rs`), so only belt speeds need substep refresh.
pub(crate) fn prepare_belt_surfaces(
    graph: Res<avian3d::dynamics::solver::constraint_graph::ConstraintGraph>,
    contacts: Res<avian3d::prelude::ContactGraph>,
    faces: Query<(&BeltFace, &avian3d::prelude::ColliderOf)>,
    poses: Query<(&Position, &Rotation)>,
    mut belts: ResMut<BeltConstraints>,
) {
    belts.prepare(&graph, &contacts, &faces, &poses);
}

impl BeltConstraints {
    pub(crate) fn prepare(
        &mut self,
        graph: &avian3d::dynamics::solver::constraint_graph::ConstraintGraph,
        contacts: &avian3d::prelude::ContactGraph,
        faces: &Query<(&BeltFace, &ColliderOf)>,
        poses: &Query<(&Position, &Rotation)>,
    ) {
        use bevy::tasks::{ComputeTaskPool, ParallelSlice};
        self.owners.clear();
        self.order.clear();
        let work: Vec<_> = graph
            .colors
            .iter()
            .enumerate()
            .flat_map(|(color, group)| {
                group
                    .contact_constraints
                    .iter()
                    .enumerate()
                    .map(move |(index, constraint)| (color, index, constraint))
            })
            .collect();
        if work.is_empty() {
            return; // No active constraints need lookup or task dispatch.
        }
        let prepared = work.par_splat_map(ComputeTaskPool::get(), None, |_, batch| {
            batch
                .iter()
                .filter_map(|(color, index, constraint)| {
                    let (_, pair) = contacts
                        .get_by_id(constraint.contact_id)
                        .expect("a prepared constraint retains its contact pair");
                    let surfaces: Vec<_> = [(pair.collider1, -1.0), (pair.collider2, 1.0)]
                        .into_iter()
                        .filter_map(|(collider, sign)| {
                            match faces.get(collider) {
                                Ok((face, of)) => Some((face, of.body, sign)),
                                Err(_) => None, // Ordinary surface: no belt motion on its body.
                            }
                        })
                        .collect();
                    // Ordinary surfaces have no belt owner or moving-face velocity to publish.
                    let owner = surfaces.iter().map(|(_, body, _)| *body).min()?;
                    let points = &pair.manifolds[constraint.manifold_index].points;
                    let centre = points.iter().map(|p| p.point).sum::<Vec3>() / points.len() as f32;
                    let faces = surfaces
                        .iter()
                        .map(|(face, body, sign)| {
                            let (position, rotation) = poses
                                .get(*body)
                                .expect("a belt face has its body's physics pose");
                            (
                                *body,
                                face.side,
                                *sign
                                    * face.velocity(
                                        centre,
                                        -*sign * constraint.normal,
                                        (position.0, rotation.0),
                                        1.0,
                                    ),
                            )
                        })
                        .collect();
                    Some((
                        owner,
                        SurfaceConstraint {
                            color: *color,
                            index: *index,
                            faces,
                        },
                    ))
                })
                .collect::<Vec<_>>()
        });
        // Commit in graph order: substeps split disjoint mutable borrows by these indices.
        for (owner, row) in prepared.into_iter().flatten() {
            let owned = self.owners.entry(owner).or_default();
            self.order.push((owner, owned.len()));
            owned.push(row);
        }
    }

    /// Avian reads world-space v2-v1 (m/s) during Relax. Split only listed constraints into
    /// disjoint borrows at the graph's atomic boundary; batch calculations by constraint count,
    /// across owners and within colors. Both belt owners are immutable after their advances.
    pub(crate) fn update(
        &self,
        graph: &mut avian3d::dynamics::solver::constraint_graph::ConstraintGraph,
        speeds: impl Fn(Entity) -> [f32; 2] + Sync + Send,
    ) {
        use bevy::tasks::{ComputeTaskPool, ParallelSliceMut};
        if self.order.is_empty() {
            return; // No active belt constraints: no substep dispatch or world scan.
        }
        let mut listed = self
            .order
            .iter()
            .map(|(owner, at)| &self.owners[owner][*at])
            .peekable();
        let mut writes = Vec::with_capacity(self.order.len());
        for (color, group) in graph.colors.iter_mut().enumerate() {
            let mut tail = group.contact_constraints.as_mut_slice();
            let mut cursor = 0;
            while listed.peek().is_some_and(|row| row.color == color) {
                let row = listed
                    .next()
                    .expect("the listed color has a next constraint");
                let (_, after) = tail.split_at_mut(row.index - cursor);
                let (constraint, rest) = after
                    .split_first_mut()
                    .expect("a prepared belt constraint survives every substep");
                writes.push((row, constraint));
                tail = rest;
                cursor = row.index + 1;
            }
        }
        writes.par_splat_map_mut(ComputeTaskPool::get(), None, |_, batch| {
            for (row, constraint) in batch {
                constraint.tangent_velocity = row
                    .faces
                    .iter()
                    .map(|(body, side, direction)| *direction * speeds(*body)[side.index()])
                    .sum();
            }
        });
    }
}

/// One solver point's impulse on a moving shoe, with its outward unit normal. The solver
/// already applied it to the hull: only its belt reaction is owed, never another hull force.
#[derive(Clone, Copy)]
pub(crate) struct BeltImpulse {
    collider: Entity,
    point: Vec3,
    outward: Vec3,
    impulse: Vec3,
    friction: f32,
}

/// Index the last step by owner in solver emission order. This boundary assigns ownership only;
/// each tank evaluates its own point kinematics and reaction in the parallel driving pass.
pub(crate) fn by_owner(
    transfers: &ContactTransfers,
    faces: &Query<(&BeltFace, &ColliderOf)>,
) -> BTreeMap<Entity, Vec<BeltImpulse>> {
    let mut owners: BTreeMap<Entity, Vec<BeltImpulse>> = BTreeMap::new();
    for point in transfers.solver_groups().flatten() {
        for (collider, sign) in [
            (point.reference_collider, -1.0),
            (
                point
                    .other_collider
                    .expect("a solver constraint joins two colliders"),
                1.0,
            ),
        ] {
            let Ok((_, of)) = faces.get(collider) else {
                continue; // The contact is on another material surface.
            };
            owners.entry(of.body).or_default().push(BeltImpulse {
                collider,
                point: point.point,
                outward: -sign * point.normal,
                impulse: sign * point.impulse,
                friction: point.friction,
            });
        }
    }
    owners
}

/// One side's solver contact over the last physics step (N): the friction it delivered to the
/// hull, in the world frame, that friction's reaction along the belt's travel, and its grip limit,
/// the solver's coefficient times the normal force at each point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HardReaction {
    pub(crate) belt: f32,
    pub(crate) friction: Vec3,
    pub(crate) grip: f32,
}

pub(crate) fn reactions(
    points: &[BeltImpulse],
    dt: f32,
    position: Vec3,
    rotation: Quat,
    faces: &Query<(&BeltFace, &ColliderOf)>,
) -> [Option<HardReaction>; 2] {
    let mut reactions = [None; 2];
    for point in points {
        if point.impulse == Vec3::ZERO {
            continue; // A speculative row that transferred nothing did not load a moving face.
        }
        let (face, _) = faces
            .get(point.collider)
            .expect("the tick-start contact snapshot retains its shoe collider through driving");
        assert!(dt > 0.0, "a solver impulse has a positive physics step");
        let friction = point.impulse - point.outward * point.impulse.dot(point.outward);
        let direction = rotation
            * face.direction(
                rotation.inverse() * (point.point - position),
                rotation.inverse() * point.outward,
            );
        let reaction = reactions[face.side.index()].get_or_insert(HardReaction {
            belt: 0.0,
            friction: Vec3::ZERO,
            grip: 0.0,
        });
        reaction.belt -= friction.dot(direction) / dt;
        reaction.friction += friction / dt;
        reaction.grip += point.friction * point.impulse.dot(point.outward).abs() / dt;
    }
    reactions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact_transfer::ContactTransfer;
    use crate::track::sim::TrackDrive;
    use avian3d::collision::contact_types::ContactId;
    use avian3d::prelude::{ContactManifold, ContactPoint};
    use bevy::ecs::system::RunSystemOnce;

    fn face(side: Side, bears: Bearing) -> BeltFace {
        // Fixture: a square outer outline, bottom traversed toward +z.
        BeltFace::new(
            side,
            vec![
                Vec2::new(-1.0, -1.0),
                Vec2::new(1.0, -1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(-1.0, 1.0),
            ],
            bears,
        )
    }

    /// Every rigid shoe face must carry the signed belt speed in world space, whichever
    /// collider Avian puts first. Drivers rely on the front wrap moving downward and the
    /// bottom moving rearward, including a lateral contact's nearest outline edge.
    #[test]
    fn each_substep_moves_each_face_at_its_own_speed_in_either_pair_order() {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        for rotation in [
            Quat::IDENTITY,
            Quat::from_rotation_y(0.43) * Quat::from_rotation_z(0.31),
        ] {
            for (local, outward, direction, lateral) in [
                (-Vec3::Y, -Vec3::Y, Vec3::Z, false),
                (-Vec3::Z, -Vec3::Z, -Vec3::Y, false),
                (Vec3::Y, Vec3::Y, -Vec3::Z, false),
                (-Vec3::Y, Vec3::X, Vec3::Z, true),
                // Fixture: tilted lateral normals remain inside the side-contact band.
                (
                    -Vec3::Y,
                    Vec3::new(1.0, 0.0, 0.01).normalize(),
                    Vec3::Z,
                    true,
                ),
                (
                    -Vec3::Y,
                    Vec3::new(-1.0, 0.01, 0.0).normalize(),
                    Vec3::Z,
                    true,
                ),
            ] {
                for first in [false, true] {
                    let mut world = World::new();
                    world.init_resource::<Time<Fixed>>();
                    let mut drive = TrackDrive::default();
                    drive.sides[Side::Left.index()].speed = 2.0; // Fixture surface speed, m/s.
                    let body = world
                        .spawn((
                            drive,
                            Position::default(),
                            Rotation(rotation),
                            Transform::IDENTITY,
                        ))
                        .id();
                    let shoe = world
                        .spawn((
                            face(
                                Side::Left,
                                if lateral {
                                    Bearing::Exposed(outward.x.signum())
                                } else {
                                    Bearing::Whole
                                },
                            ),
                            Transform::IDENTITY,
                            ColliderOf { body },
                        ))
                        .id();
                    world.insert_resource(TrackField {
                        revision: None,
                        field: Some(super::super::oracle::BlockField::new(vec![]).with_ground(
                            Some(crate::ground_field::GroundField::uniform(
                                crate::ground_field::ContactGround {
                                    grip: 0.37,
                                    rolling_resistance: 0.0, // Fixture ground.
                                    shear_length_m: 0.010,
                                },
                            )),
                        )),
                    });
                    let other = world.spawn(FieldSurface).id();
                    let sign = if first { -1.0 } else { 1.0 };
                    let mut pair = if first {
                        ContactPair::new(shoe, other, ContactId::PLACEHOLDER)
                    } else {
                        ContactPair::new(other, shoe, ContactId::PLACEHOLDER)
                    };
                    pair.manifolds.push(ContactManifold::new(
                        [ContactPoint::new(
                            Vec3::ZERO,
                            Vec3::ZERO,
                            rotation * local,
                            0.01,
                        )],
                        -sign * (rotation * outward),
                    ));
                    world
                        .run_system_once(move |hooks: WorldContacts, mut commands: Commands,
                            faces: Query<(&BeltFace, &ColliderOf)>, poses: Query<(&Position, &Rotation)>| {
                            use avian3d::collision::contact_types::{ContactEdge, ContactGraph};
                            use avian3d::dynamics::solver::{contact::ContactConstraint, constraint_graph::ConstraintGraph, solver_body::SolverBodyIndex};
                            assert!(hooks.modify_contacts(&mut pair, &mut commands));
                            assert_eq!(pair.manifolds[0].friction, 0.37);
                            assert_eq!(pair.manifolds[0].tangent_velocity, Vec3::ZERO, "the hook never publishes tick-start belt speed");
                            let mut contacts = ContactGraph::default();
                            let id = contacts.add_edge_with(ContactEdge::new(pair.collider1, pair.collider2), |p| p.manifolds.clone_from(&pair.manifolds)).unwrap();
                            let mut graph = ConstraintGraph::default();
                            graph.colors[0].contact_constraints.push(ContactConstraint {
                                body_index1: SolverBodyIndex::INVALID, body_index2: SolverBodyIndex::INVALID,
                                relative_dominance: 0, friction: 0.37, restitution: 0.0,
                                tangent_velocity: Vec3::ZERO, normal: pair.manifolds[0].normal,
                                tangent1: Vec3::X, points: Vec::new(), contact_id: id, manifold_index: 0,
                            });
                            let mut belts = BeltConstraints::default();
                            belts.prepare(&graph, &contacts, &faces, &poses);
                            // Fixture: distinct consecutive substep speeds; overwrites must not accumulate.
                            for speed in [2.0, -3.0, 0.0] {
                                belts.update(&mut graph, |_| [speed; 2]);
                                let expected = sign * (rotation * direction) * speed;
                                // Resolution: repeated world/local rotations may differ by f32 roundoff.
                                assert!((graph.colors[0].contact_constraints[0].tangent_velocity - expected).length() < 1e-5);
                            }
                        })
                        .unwrap();
                }
            }
        }
    }

    /// Belt-on-belt surfaces read both owners but write once, including Avian's overflow color.
    /// Unrelated constraints keep their velocity; an empty list must never request owner speeds.
    #[test]
    fn shared_belt_constraints_have_one_owner_and_leave_other_surfaces_untouched() {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        use avian3d::collision::contact_types::{ContactEdge, ContactGraph};
        use avian3d::dynamics::solver::{
            constraint_graph::{COLOR_OVERFLOW_INDEX, ConstraintGraph},
            contact::ContactConstraint,
            solver_body::SolverBodyIndex,
        };
        let mut world = World::new();
        let bodies = [0, 1].map(|_| world.spawn((Position::default(), Rotation::default())).id());
        let shoes = bodies.map(|body| {
            world
                .spawn((face(Side::Left, Bearing::Whole), ColliderOf { body }))
                .id()
        });
        let other = [0, 1].map(|_| world.spawn_empty().id());
        world
            .run_system_once(
                move |faces: Query<(&BeltFace, &ColliderOf)>,
                      poses: Query<(&Position, &Rotation)>| {
                    let mut contacts = ContactGraph::default();
                    let mut pair = |colliders: [Entity; 2]| {
                        contacts
                            .add_edge_with(ContactEdge::new(colliders[0], colliders[1]), |p| {
                                p.manifolds.push(ContactManifold::new(
                                    [ContactPoint::new(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, 0.0)],
                                    -Vec3::Y,
                                ))
                            })
                            .unwrap()
                    };
                    let belt = pair(shoes);
                    let ordinary = pair(other);
                    // Fixture: a sentinel velocity for ordinary surfaces, distinct from either belt.
                    let constraint = |contact_id| ContactConstraint {
                        body_index1: SolverBodyIndex::INVALID,
                        body_index2: SolverBodyIndex::INVALID,
                        relative_dominance: 0,
                        friction: 1.0,
                        restitution: 0.0,
                        tangent_velocity: Vec3::X,
                        normal: -Vec3::Y,
                        tangent1: Vec3::X,
                        points: Vec::new(),
                        contact_id,
                        manifold_index: 0,
                    };
                    for color in [0, COLOR_OVERFLOW_INDEX] {
                        let mut graph = ConstraintGraph::default();
                        graph.colors[color].contact_constraints =
                            vec![constraint(ordinary), constraint(belt), constraint(ordinary)];
                        let mut belts = BeltConstraints::default();
                        belts.prepare(&graph, &contacts, &faces, &poses);
                        assert_eq!(belts.owners.len(), 1);
                        assert_eq!(belts.owners[&bodies[0].min(bodies[1])].len(), 1);
                        // Fixture: opposite exposed faces carry 3 and 5 m/s; v2-v1 sums their tangents.
                        belts.update(&mut graph, |body| {
                            if body == bodies[0] {
                                [3.0; 2]
                            } else {
                                [5.0; 2]
                            }
                        });
                        let rows = &graph.colors[color].contact_constraints;
                        assert_eq!(rows[0].tangent_velocity, Vec3::X);
                        assert_eq!(rows[1].tangent_velocity, -8.0 * Vec3::Z);
                        assert_eq!(rows[2].tangent_velocity, Vec3::X);
                        graph.colors[color].contact_constraints.remove(1);
                        belts.prepare(&graph, &contacts, &faces, &poses);
                        assert!(belts.owners.is_empty());
                        belts.update(&mut graph, |_| {
                            panic!("no belt constraints may read an owner")
                        });
                    }
                },
            )
            .unwrap();
    }

    /// A side slice bears only lateral contact through its exposed face ([`Bearing::Exposed`]): its
    /// rest-height radial planes would bypass suspension travel and duplicate the stop's limit. The
    /// sweep's witnesses rely on the same admission (`construction::sweep::source`). Fails if a slice
    /// bears a radial normal or a lateral one of either sign, or ignores its collider's pair order.
    #[test]
    fn a_side_slice_bears_only_lateral_contact_through_its_exposed_face() {
        for (outward, bears) in [(-Vec3::X, true), (Vec3::X, false), (-Vec3::Y, false)] {
            for first in [false, true] {
                let mut world = World::new();
                world.init_resource::<Time<Fixed>>();
                let body = world
                    .spawn((
                        TrackDrive::default(),
                        Position::default(),
                        Rotation::default(),
                        Transform::IDENTITY,
                    ))
                    .id();
                let shoe = world
                    .spawn((
                        face(Side::Left, Bearing::Exposed(-1.0)),
                        Transform::IDENTITY,
                        ColliderOf { body },
                    ))
                    .id();
                let other = world.spawn_empty().id();
                let (mut pair, normal) = if first {
                    (
                        ContactPair::new(shoe, other, ContactId::PLACEHOLDER),
                        outward,
                    )
                } else {
                    (
                        ContactPair::new(other, shoe, ContactId::PLACEHOLDER),
                        -outward,
                    )
                };
                pair.manifolds.push(ContactManifold::new(
                    [ContactPoint::new(Vec3::ZERO, Vec3::ZERO, -Vec3::Y, 0.01)],
                    normal,
                ));
                world
                    .run_system_once(move |hooks: WorldContacts, mut commands: Commands| {
                        let case =
                            format!("a normal leaving along {outward}, slice first: {first}");
                        assert_eq!(
                            hooks.modify_contacts(&mut pair, &mut commands),
                            bears,
                            "{case}"
                        );
                        assert_eq!(pair.manifolds.is_empty(), !bears, "{case}");
                    })
                    .unwrap();
            }
        }
    }

    /// A side slice bears no contact in the band its belt has risen out of over matter under its
    /// edge: there it meets the matter the belt rides over, and a lateral normal would shove the
    /// shoe sideways off what it stands on (`track-contact.md`). Above that band the shoe is there,
    /// and an edge with nothing under it bears through its exposed face. Each edge reads its own
    /// rise from its nearest link, on either side, in either pair order, whichever way its belt
    /// runs; a point on the matter's top edge, at the rise itself, is in the band. Fails if the
    /// hook ignores the rise, reads the other edge's or the wrong link's, or keeps or drops a whole
    /// manifold for one point.
    #[test]
    fn a_side_slice_bears_nothing_in_the_band_its_belt_has_vacated() {
        // Fixture: the square's rest outer face along its bottom and its top, both runs travelling
        // as the outline does; only the bottom ever rises.
        let link = |face: [Vec2; 2], rise: [f32; 2]| LinkRise { face, rise };
        let (bottom, top) = (
            [Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0)],
            [Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)],
        );
        let rise = 0.2; // Fixture rise under the risen edge, m.
        let (position, rotation) = (
            Vec3::new(3.0, 1.0, -2.0),
            Quat::from_rotation_y(0.43) * Quat::from_rotation_z(0.31),
        );
        for side in Side::ALL {
            for outboard in [false, true] {
                let (edge, sign) = (usize::from(outboard), side.sign());
                let exposed = if outboard { sign } else { -sign };
                let mut own = [0.0; 2];
                own[edge] = rise;
                let mut other = [0.0; 2];
                other[1 - edge] = rise;
                // (edge rises, depths of the points into the rest face (m), points that bear).
                for (rises, depths, kept) in [
                    (own, vec![0.1], 0),
                    (own, vec![rise], 0),
                    (own, vec![0.3], 1),
                    (own, vec![0.1, 0.3], 1),
                    (other, vec![0.1], 1),
                    ([0.0; 2], vec![0.1, 0.3], 2),
                ] {
                    for (first, speed) in [(false, 2.0), (true, 2.0), (true, -2.0)] {
                        let mut world = World::new();
                        world.init_resource::<Time<Fixed>>();
                        let mut drive = TrackDrive::default();
                        drive.sides[side.index()].speed = speed; // Fixture belt speed, m/s.
                        let mut links = TrackRise::default();
                        links.0[side.index()] = vec![link(bottom, rises), link(top, [0.0; 2])];
                        let body = world
                            .spawn((
                                drive,
                                links,
                                Position(position),
                                Rotation(rotation),
                                Transform::IDENTITY,
                            ))
                            .id();
                        let shoe = world
                            .spawn((
                                face(side, Bearing::Exposed(exposed)),
                                Transform::IDENTITY,
                                ColliderOf { body },
                            ))
                            .id();
                        let other = world.spawn_empty().id();
                        let outward = rotation * (Vec3::X * exposed);
                        let (mut pair, normal) = if first {
                            (
                                ContactPair::new(shoe, other, ContactId::PLACEHOLDER),
                                outward,
                            )
                        } else {
                            (
                                ContactPair::new(other, shoe, ContactId::PLACEHOLDER),
                                -outward,
                            )
                        };
                        let points = depths.iter().map(|depth| {
                            let local = Vec3::new(exposed * 0.5, -1.0 + depth, 0.0);
                            ContactPoint::new(
                                Vec3::ZERO,
                                Vec3::ZERO,
                                position + rotation * local,
                                0.01,
                            )
                        });
                        pair.manifolds.push(ContactManifold::new(points, normal));
                        let case = format!(
                            "{side:?} outboard {outboard}: rises {rises:?}, depths {depths:?}, \
                             slice first {first}, belt {speed} m/s"
                        );
                        world
                            .run_system_once(move |hooks: WorldContacts, mut commands: Commands| {
                                assert_eq!(
                                    hooks.modify_contacts(&mut pair, &mut commands),
                                    kept > 0,
                                    "{case}"
                                );
                                let left: usize =
                                    pair.manifolds.iter().map(|m| m.points.len()).sum();
                                assert_eq!(kept, left, "{case}");
                                assert!(pair.manifolds.iter().all(|m| !m.points.is_empty()));
                            })
                            .unwrap();
                    }
                }
            }
        }
    }

    /// Sleep never changes which points a slice bears. A dynamic or kinematic body's candidate
    /// contact wakes a resting belt whatever either face's filtering keeps (a moving belt's face
    /// whose band erases the pair first, or a normal the slice rejects); static matter, a resting
    /// body or none does not. The belt's next pass then rewrites the band (`TrackRise`). Each case
    /// runs in both pair orders.
    #[test]
    fn sleep_keeps_the_band_and_a_moving_body_wakes_the_belt() {
        let (side, rise) = (Side::Left, 0.2); // Fixture rise under the outboard edge, m.
        let band = |rise| {
            let mut links = TrackRise::default();
            links.0[side.index()] = vec![LinkRise {
                face: [Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0)],
                rise,
            }];
            links
        };
        // (the belt rests; the other body's kind, whether it rests, and whether it is a belt whose
        // inboard face's band holds both points; the candidate's normal is lateral; the belt
        // wakes).
        let cases = [
            (true, Some((RigidBody::Dynamic, false, false)), true, true),
            (true, Some((RigidBody::Dynamic, false, true)), true, true),
            (true, Some((RigidBody::Kinematic, false, false)), true, true),
            (true, Some((RigidBody::Dynamic, true, false)), true, false),
            (true, Some((RigidBody::Static, false, false)), true, false),
            (true, None, true, false),
            (false, Some((RigidBody::Dynamic, false, false)), true, false),
            (true, Some((RigidBody::Dynamic, false, false)), false, true),
        ];
        for ((resting, other, lateral, wakes), reversed) in
            (cases.into_iter()).flat_map(|case| [(case, false), (case, true)])
        {
            let mut world = World::new();
            world.init_resource::<Time<Fixed>>();
            let spawned = (
                TrackDrive::default(),
                band([0.0, rise]),
                Position::default(),
                Rotation::default(),
            );
            let mut belt = world.spawn((spawned, RigidBody::Dynamic, Transform::IDENTITY));
            if resting {
                belt.insert(Sleeping);
            }
            let belt = belt.id();
            let shoe = world
                .spawn((
                    face(side, Bearing::Exposed(side.sign())),
                    Transform::IDENTITY,
                    ColliderOf { body: belt },
                ))
                .id();
            let body = other.map(|(kind, rests, belted)| {
                let mut other = world.spawn((kind, Position::default(), Rotation::default()));
                if rests {
                    other.insert(Sleeping);
                }
                if belted {
                    let belt = (TrackDrive::default(), band([2.0 * rise, 0.0]));
                    other.insert((belt, Transform::IDENTITY));
                }
                other.id()
            });
            let belted = other.is_some_and(|(_, _, belted)| belted);
            let struck = match body {
                Some(body) if belted => world.spawn((
                    face(side, Bearing::Exposed(-side.sign())),
                    Transform::IDENTITY,
                    ColliderOf { body },
                )),
                _ => world.spawn_empty(),
            };
            let struck = struck.id();
            let outward = if lateral {
                Vec3::X * side.sign()
            } else {
                Vec3::NEG_Y
            };
            let (mut pair, normal) = if reversed {
                let mut pair = ContactPair::new(struck, shoe, ContactId::PLACEHOLDER);
                (pair.body1, pair.body2) = (body, Some(belt));
                (pair, -outward)
            } else {
                let mut pair = ContactPair::new(shoe, struck, ContactId::PLACEHOLDER);
                (pair.body1, pair.body2) = (Some(belt), body);
                (pair, outward)
            };
            // Through the outboard slice's exposed face: halfway up the band, and as far above it.
            let [inside, above] =
                [0.5, 1.5].map(|up| Vec3::new(side.sign() * 0.5, -1.0 + up * rise, 0.0));
            pair.manifolds.push(ContactManifold::new(
                [inside, above].map(|point| ContactPoint::new(Vec3::ZERO, Vec3::ZERO, point, 0.01)),
                normal,
            ));
            let borne = world
                .run_system_once(move |hooks: WorldContacts, mut commands: Commands| {
                    let bears = hooks.modify_contacts(&mut pair, &mut commands);
                    let points = (pair.manifolds.iter().flat_map(|m| &m.points)).map(|p| p.point);
                    (bears, points.collect::<Vec<_>>())
                })
                .unwrap();
            let kept = if belted || !lateral {
                vec![]
            } else {
                vec![above]
            };
            assert_eq!(
                (borne, world.entity(belt).contains::<Sleeping>()),
                ((!kept.is_empty(), kept), resting && !wakes),
                "belt rests {resting}, other {other:?}, lateral {lateral}, reversed {reversed}"
            );
        }
    }

    /// The transmission owes only tangential momentum on its own belt, and the Driver's ledger
    /// feels that friction as the hull received it. Reversing Avian's canonical pair orientation
    /// must preserve both and exclude the normal impulse.
    #[test]
    fn solver_reaction_is_signed_friction_per_step_on_the_owning_side() {
        for reverse in [false, true] {
            let mut world = World::new();
            let body = world.spawn(Transform::IDENTITY).id();
            let shoe = world
                .spawn((
                    face(Side::Right, Bearing::Whole),
                    Transform::IDENTITY,
                    ColliderOf { body },
                ))
                .id();
            let other = world.spawn_empty().id();
            let mut point = ContactTransfer {
                reference: body,
                reference_collider: shoe,
                other,
                other_collider: Some(other),
                point: -Vec3::Y,
                normal: -Vec3::Y,
                impulse: Vec3::new(0.0, -10.0, 3.0), // Fixture N s into the other body.
                effective_mass: 1.0,
                closing_speed: 0.0,
                friction: 0.5, // Fixture solver coefficient.,
            };
            if reverse {
                point = ContactTransfer {
                    reference: other,
                    reference_collider: other,
                    other: body,
                    other_collider: Some(shoe),
                    normal: -point.normal,
                    impulse: -point.impulse,
                    ..point
                };
            }
            let transfers = ContactTransfers::fixture(vec![vec![point]], 0.25);
            world
                .run_system_once(move |faces: Query<(&BeltFace, &ColliderOf)>| {
                    let owners = by_owner(&transfers, &faces);
                    assert_eq!(owners.len(), 1);
                    let reaction = reactions(
                        &owners[&body],
                        transfers.dt(),
                        Vec3::ZERO,
                        Quat::IDENTITY,
                        &faces,
                    );
                    // DERIVED 3 N s / 0.25 s, driving the hull forward (-Z); the grip limit
                    // 0.5 × 10 N s / 0.25 s.
                    let right = HardReaction {
                        belt: 12.0,
                        friction: Vec3::new(0.0, 0.0, -12.0),
                        grip: 20.0,
                    };
                    assert_eq!(reaction, [None, Some(right)]);
                })
                .unwrap();
        }
    }
}
