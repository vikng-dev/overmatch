//! Moving outer shoe faces for the game's driving physics. Hard contacts stay in Avian;
//! their delivered friction returns to the transmission on the next driving tick.

use std::collections::BTreeMap;
use std::sync::Arc;

use avian3d::prelude::{
    ActiveCollisionHooks, Collider, ColliderOf, CollisionHooks, ContactPair, Position, Rotation,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::belt_segment::{BeltSegment, SegmentLaw};
use super::oracle::ContactOracle;
use super::rubble::Surface;
use super::side::Side;
use super::sim::TrackDrive;
use super::terrain::{FieldSurface, TrackField};
use crate::contact_transfer::ContactTransfers;

/// Spawn-built surface kinematics. A side slice supplies only lateral contact across the
/// suspension band; the compressed outer outline owns every radial hard contact.
#[derive(Component, Clone)]
#[require(ActiveCollisionHooks::MODIFY_CONTACTS)]
pub(crate) struct BeltFace {
    pub(crate) side: Side,
    outline: Arc<[Vec2]>,
    lateral_only: bool,
}

/// Resolution: tolerance for unit-normal components after hull-frame rotation. It separates
/// lateral planes from radial planes without amplifying roundoff into a belt tangent.
const NORMAL_EPSILON: f32 = 1e-4;

impl BeltFace {
    pub(crate) fn new(side: Side, mut outline: Vec<Vec2>, lateral_only: bool) -> Self {
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
            lateral_only,
        }
    }

    /// Positive belt travel in the hull frame. Radial contact follows its plane's tangent;
    /// a lateral face reads the closest edge of its closed outer outline in `(z, y)`.
    pub(crate) fn direction(&self, point: Vec3, outward: Vec3) -> Vec3 {
        let radial = outward.cross(Vec3::X);
        if radial.length_squared() > NORMAL_EPSILON * NORMAL_EPSILON {
            return radial.normalize();
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

/// Avian 0.7 permits one hook set. The construction sweep judges speculative points first;
/// surviving shoe contacts carry the belt's motion and the field's grip at the patch, and a belt
/// segment's contacts its support law first.
#[derive(SystemParam)]
pub(crate) struct WorldContacts<'w, 's> {
    sweep: crate::construction::breakage::SweepContacts<'w, 's>,
    faces: Query<'w, 's, (&'static BeltFace, &'static ColliderOf)>,
    segments: Query<'w, 's, &'static BeltSegment>,
    law: Option<Res<'w, SegmentLaw>>,
    belts: Query<'w, 's, (&'static TrackDrive, &'static Position, &'static Rotation)>,
    pieces: Query<'w, 's, (&'static Collider, &'static Position, &'static Rotation)>,
    surfaces: Query<'w, 's, (), With<FieldSurface>>,
    field: Option<Res<'w, TrackField>>,
}

impl CollisionHooks for WorldContacts<'_, '_> {
    fn modify_contacts(&self, contacts: &mut ContactPair, commands: &mut Commands) -> bool {
        if !self.sweep.modify_contacts(contacts, commands) {
            return false;
        }
        // Avian's relative surface velocity is collider 2 minus collider 1.
        for (collider, other, sign) in [
            (contacts.collider1, contacts.collider2, -1.0),
            (contacts.collider2, contacts.collider1, 1.0),
        ] {
            let Ok((face, of)) = self.faces.get(collider) else {
                continue; // Ordinary collider: its surface stands still on its body.
            };
            let (drive, position, rotation) = self
                .belts
                .get(of.body)
                .expect("a moving shoe face hangs on its spawn-built driving body");
            if face.lateral_only {
                contacts
                    .manifolds
                    .retain(|m| (rotation.0.inverse() * m.normal).x.abs() >= 1.0 - NORMAL_EPSILON);
            }
            if let Ok(segment) = self.segments.get(collider) {
                let law = self
                    .law
                    .as_deref()
                    .expect("a belt segment meets a piece only after its support law is published");
                let (shape, at, turn) = self
                    .pieces
                    .get(other)
                    .expect("a belt segment's contact names a collider with its pose");
                let covered = segment.covered_area(
                    Surface::triangles(shape, at.0, turn.0)
                        .into_iter()
                        .flatten(),
                    (position, rotation),
                );
                segment.shape_contacts(
                    &mut contacts.manifolds,
                    (position, rotation),
                    sign,
                    (law.stiffness_per_m, law.damping_per_m),
                    covered,
                );
            }
            for manifold in &mut contacts.manifolds {
                let centre = manifold.points.iter().map(|p| p.point).sum::<Vec3>()
                    / manifold.points.len() as f32;
                let local = rotation.0.inverse() * (centre - position.0);
                let outward = rotation.0.inverse() * (-sign * manifold.normal);
                let direction = rotation.0 * face.direction(local, outward);
                manifold.tangent_velocity +=
                    sign * direction * drive.sides[face.side.index()].speed;
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

/// One solver point's impulse on a moving shoe, with its outward unit normal. The solver
/// already applied it to the hull: only its belt reaction is owed, never another hull force.
#[derive(Clone, Copy)]
pub(crate) struct BeltImpulse {
    collider: Entity,
    point: Vec3,
    outward: Vec3,
    impulse: Vec3,
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
            });
        }
    }
    owners
}

pub(crate) fn reactions(
    points: &[BeltImpulse],
    dt: f32,
    position: Vec3,
    rotation: Quat,
    faces: &Query<(&BeltFace, &ColliderOf)>,
) -> [Option<f32>; 2] {
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
        *reactions[face.side.index()].get_or_insert(0.0) -= friction.dot(direction) / dt;
    }
    reactions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact_transfer::ContactTransfer;
    use avian3d::collision::contact_types::ContactId;
    use avian3d::prelude::{ContactManifold, ContactPoint};
    use bevy::ecs::system::RunSystemOnce;

    fn face(side: Side, lateral: bool) -> BeltFace {
        // Fixture: a square outer outline, bottom traversed toward +z.
        BeltFace::new(
            side,
            vec![
                Vec2::new(-1.0, -1.0),
                Vec2::new(1.0, -1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(-1.0, 1.0),
            ],
            lateral,
        )
    }

    /// Every rigid shoe face must carry the signed belt speed in world space, whichever
    /// collider Avian puts first. Drivers rely on the front wrap moving downward and the
    /// bottom moving rearward, including a lateral contact's nearest outline edge.
    #[test]
    fn the_composed_hook_moves_each_face_at_belt_speed_in_either_pair_order() {
        for rotation in [
            Quat::IDENTITY,
            Quat::from_rotation_y(0.43) * Quat::from_rotation_z(0.31),
        ] {
            for (local, outward, direction, lateral) in [
                (-Vec3::Y, -Vec3::Y, Vec3::Z, false),
                (-Vec3::Z, -Vec3::Z, -Vec3::Y, false),
                (Vec3::Y, Vec3::Y, -Vec3::Z, false),
                (-Vec3::Y, Vec3::X, Vec3::Z, true),
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
                            face(Side::Left, lateral),
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
                        .run_system_once(move |hooks: WorldContacts, mut commands: Commands| {
                            assert!(hooks.modify_contacts(&mut pair, &mut commands));
                            assert_eq!(pair.manifolds[0].friction, 0.37);
                            let expected = sign * (rotation * direction) * 2.0;
                            assert!(
                                (pair.manifolds[0].tangent_velocity - expected).length() < 1e-5
                            );
                        })
                        .unwrap();
                }
            }
        }
    }

    /// Side slices may shove across the shoe width, but their rest-height radial planes
    /// cannot bypass suspension travel or duplicate the compressed outer-face hard limit.
    #[test]
    fn lateral_slices_do_not_supply_radial_hard_support() {
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
                face(Side::Left, true),
                Transform::IDENTITY,
                ColliderOf { body },
            ))
            .id();
        let other = world.spawn_empty().id();
        let mut pair = ContactPair::new(shoe, other, ContactId::PLACEHOLDER);
        pair.manifolds.push(ContactManifold::new(
            [ContactPoint::new(Vec3::ZERO, Vec3::ZERO, -Vec3::Y, 0.01)],
            -Vec3::Y,
        ));
        world
            .run_system_once(move |hooks: WorldContacts, mut commands: Commands| {
                assert!(!hooks.modify_contacts(&mut pair, &mut commands));
                assert!(pair.manifolds.is_empty());
            })
            .unwrap();
    }

    /// The transmission owes only tangential momentum on its own belt. Reversing Avian's
    /// canonical pair orientation must preserve the reaction and exclude the normal impulse.
    #[test]
    fn solver_reaction_is_signed_friction_per_step_on_the_owning_side() {
        for reverse in [false, true] {
            let mut world = World::new();
            let body = world.spawn(Transform::IDENTITY).id();
            let shoe = world
                .spawn((
                    face(Side::Right, false),
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
                    assert_eq!(reaction, [None, Some(12.0)]); // DERIVED 3 N s / 0.25 s.
                })
                .unwrap();
        }
    }
}
