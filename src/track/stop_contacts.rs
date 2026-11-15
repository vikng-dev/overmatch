//! Moving outer shoe faces for the game's driving physics. Hard contacts stay in Avian;
//! their delivered friction returns to the transmission on the next driving tick.

use std::collections::BTreeMap;
use std::sync::Arc;

use avian3d::prelude::{
    ActiveCollisionHooks, ColliderOf, CollisionHooks, ContactPair, Position, Rotation,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::forces::LinkRise;
use super::oracle::ContactOracle;
use super::side::Side;
use super::sim::{TrackDrive, TrackRise};
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

/// Avian 0.7 permits one hook set. The construction sweep judges speculative points first;
/// surviving shoe contacts carry the belt's motion and the field's grip at the patch.
#[derive(SystemParam)]
pub(crate) struct WorldContacts<'w, 's> {
    sweep: crate::construction::breakage::SweepContacts<'w, 's>,
    faces: Query<'w, 's, (&'static BeltFace, &'static ColliderOf)>,
    belts: Query<
        'w,
        's,
        (
            &'static TrackDrive,
            &'static TrackRise,
            &'static Position,
            &'static Rotation,
        ),
    >,
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
            let (drive, rise, position, rotation) = self
                .belts
                .get(of.body)
                .expect("a moving shoe face hangs on its spawn-built driving body");
            let links = &rise.0[face.side.index()];
            contacts
                .manifolds
                .retain(|m| face.accepts_normal(rotation.0.inverse() * (-sign * m.normal)));
            for manifold in &mut contacts.manifolds {
                (manifold.points).retain(|p| {
                    face.stands_at(links, rotation.0.inverse() * (p.point - position.0))
                });
            }
            contacts.manifolds.retain(|m| !m.points.is_empty());
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
