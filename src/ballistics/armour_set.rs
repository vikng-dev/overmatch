//! The game's damage model: a tank's armour set. Its armour volumes and crew boxes (HP pools at
//! stations) are grouped by the rigid frame that carries them, built at spawn from the tank's data
//! (ADR-0014), so the damage model bounds and poses them per frame instead of per collider.
//!
//! A frame is the tank root, or a node that moves against its parent: a servo, a recoiling barrel,
//! or the assembly a launch detaches. A part belongs to its nearest frame at or above its node, at
//! a fixed pose in that frame; the nodes between a frame and its parts never move. A frame is
//! carried by its nearest frame above it, at a fixed offset to its node's parent. Its base is its
//! nearest rigid-body ancestor: the root, until a launch makes an assembly its own body
//! ([`rebase`]).

use avian3d::prelude::{Collider, RigidBody, SimpleCollider};
use bevy::prelude::*;

use super::motion::Pose;

/// The game's damage model: an axis-aligned box in one frame's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// The box, in the coordinates `pose` is expressed in, that holds this box posed by `pose`:
    /// the posed centre, and the half extents through the rotation's absolute matrix (parry's
    /// `Aabb::transform_by`).
    pub(super) fn posed(self, pose: Pose) -> Self {
        let turn = Mat3::from_quat(pose.rotation);
        let centre = pose.position + turn * ((self.min + self.max) * 0.5);
        let reach = Mat3::from_cols(turn.x_axis.abs(), turn.y_axis.abs(), turn.z_axis.abs())
            * ((self.max - self.min) * 0.5);
        Self {
            min: centre - reach,
            max: centre + reach,
        }
    }
}

/// The game's damage model: one armour volume or crew box primitive of a tank.
#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "a part's pose composes with its frame's interval; its box is the set's narrow \
                  candidate test"
    )
)]
pub(super) struct Part {
    /// The primitive's entity: its identity for keys, HP pools and records.
    pub collider: Entity,
    /// Its pose in its frame, fixed.
    pub pose: Pose,
    /// Its mesh's box in its own coordinates.
    pub bounds: Bounds,
}

/// The game's damage model: one rigid frame of a tank and the parts it carries.
#[derive(Debug)]
pub(super) struct Frame {
    /// The node whose pose the frame follows; the root for the root frame.
    pub node: Entity,
    /// The frame carrying this one; `None` for a frame that is its own base's body.
    pub carrier: Option<usize>,
    /// The rest pose of `node`'s parent in the carrier's frame.
    pub offset: Pose,
    /// The nearest rigid-body ancestor of `node`, itself included.
    pub base: Entity,
    pub parts: Vec<Part>,
    /// The box of every part in this frame's coordinates; `None` for a frame carrying none.
    pub bounds: Option<Bounds>,
}

/// The game's damage model: a tank's armour volumes and crew boxes by rigid frame, frame 0 the
/// root. A carrier always precedes the frames it carries.
#[derive(Component, Debug)]
pub(crate) struct ArmourSet {
    frames: Vec<Frame>,
}

/// Marks a frame node other than the root with the tank whose set holds its frame ([`rebase`]).
#[derive(Component)]
pub(crate) struct ArmourFrame {
    tank: Entity,
}

/// One node of a tank's armour hierarchy, below the root, in parent-first order.
pub(crate) struct Node {
    pub entity: Entity,
    /// Index of its parent in the same list; `None` for a child of the root.
    pub parent: Option<usize>,
    /// Its rest pose in its parent.
    pub transform: Transform,
    /// Whether it moves against its parent ([`ArmourSet`]'s frames).
    pub frame: bool,
}

/// One armour volume or crew box primitive: its collider entity, the node it hangs under (`None`:
/// the root) and its pose there, and its shape.
pub(crate) struct PartSource<'a> {
    pub collider: Entity,
    pub node: Option<usize>,
    pub pose: Transform,
    pub shape: &'a Collider,
}

/// A rest link as a pose. The damage model poses armour through unit-scale links only: the bake
/// certifies unit composed scale on every armour node (`bake::manifold_gate`), and frames and the
/// links between them carry no scale either.
fn link(transform: Transform, what: &str) -> Pose {
    assert_eq!(
        transform.scale.to_array().map(f32::to_bits),
        [1.0f32.to_bits(); 3],
        "game's damage model: armour link {what} carries scale {:?}; armour is posed through \
         unit-scale links",
        transform.scale
    );
    Pose {
        position: transform.translation,
        rotation: transform.rotation,
    }
}

impl ArmourSet {
    /// The set of `root`'s armour: `nodes` its hierarchy below the root, parent first, and `parts`
    /// its armour volume and crew box primitives.
    pub(crate) fn build<'a>(
        root: Entity,
        nodes: &[Node],
        parts: impl IntoIterator<Item = PartSource<'a>>,
    ) -> Self {
        let mut frames = vec![Frame {
            node: root,
            carrier: None,
            offset: Pose::IDENTITY,
            base: root,
            parts: Vec::new(),
            bounds: None,
        }];
        // Each node's frame and its pose there.
        let mut placed: Vec<(usize, Pose)> = Vec::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            let (carrier, above) = match node.parent {
                None => (0, Pose::IDENTITY),
                Some(parent) => {
                    assert!(parent < index, "armour nodes are listed parent first");
                    placed[parent]
                }
            };
            if node.frame {
                // A frame's rest pose is a link like any other: unit scale.
                link(node.transform, &format!("frame {}", node.entity));
                frames.push(Frame {
                    node: node.entity,
                    carrier: Some(carrier),
                    offset: above,
                    base: root,
                    parts: Vec::new(),
                    bounds: None,
                });
                placed.push((frames.len() - 1, Pose::IDENTITY));
            } else {
                placed.push((
                    carrier,
                    above * link(node.transform, &format!("node {}", node.entity)),
                ));
            }
        }
        for part in parts {
            let (frame, above) = part.node.map_or((0, Pose::IDENTITY), |node| placed[node]);
            let pose = above * link(part.pose, &format!("part {}", part.collider));
            let local = part.shape.aabb(Vec3::ZERO, Quat::IDENTITY);
            let bounds = Bounds {
                min: local.min,
                max: local.max,
            };
            let frame = &mut frames[frame];
            let posed = bounds.posed(pose);
            frame.bounds = Some(frame.bounds.map_or(posed, |held| held.union(posed)));
            frame.parts.push(Part {
                collider: part.collider,
                pose,
                bounds,
            });
        }
        Self { frames }
    }

    /// Insert the set on `root` and mark its frame nodes, in one command batch.
    pub(crate) fn insert(self, commands: &mut Commands, root: Entity) {
        for frame in &self.frames[1..] {
            commands
                .entity(frame.node)
                .insert(ArmourFrame { tank: root });
        }
        commands.entity(root).insert(self);
    }

    pub(super) fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The root, the body the set's own frames are based on.
    fn root(&self) -> Entity {
        self.frames[0].node
    }

    /// Frame `frame`'s pose in its base's frame, each frame's pose in its carrier given by
    /// `in_carrier`.
    pub(super) fn in_base(&self, frame: usize, in_carrier: &impl Fn(usize) -> Pose) -> Pose {
        match self.frames[frame].carrier {
            None => Pose::IDENTITY,
            Some(carrier) => self.in_base(carrier, in_carrier) * in_carrier(frame),
        }
    }

    /// The box, in the root body's frame, holding every part the root carries, each frame posed by
    /// `in_carrier`: one posed box per frame, the root's own unposed. `None` when the root carries
    /// no part.
    pub(super) fn body_box(&self, in_carrier: &impl Fn(usize) -> Pose) -> Option<Bounds> {
        let root = self.root();
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.base == root)
            .filter_map(|(index, frame)| {
                let bounds = frame.bounds?;
                Some(match frame.carrier {
                    None => bounds,
                    Some(_) => bounds.posed(self.in_base(index, in_carrier)),
                })
            })
            .reduce(Bounds::union)
    }

    /// `body` becomes a rigid body: a frame on that node is its own base from now on, and every
    /// frame it carries is based on it.
    fn rebase(&mut self, body: Entity) {
        for index in 0..self.frames.len() {
            if self.frames[index].node == body {
                let frame = &mut self.frames[index];
                frame.carrier = None;
                frame.offset = Pose::IDENTITY;
                frame.base = body;
            } else if let Some(carrier) = self.frames[index].carrier {
                self.frames[index].base = self.frames[carrier].base;
            }
        }
    }
}

/// The game's damage model: re-resolve a tank's frame bases when one of its frame nodes becomes a
/// rigid body (a launched assembly, `damage::launch_turrets_on_cookoff`).
pub(crate) fn rebase(
    added: On<Add, RigidBody>,
    frames: Query<&ArmourFrame>,
    mut sets: Query<&mut ArmourSet>,
) {
    // Most bodies (a tank root, debris, constructions) are no armour frame.
    let Ok(&ArmourFrame { tank }) = frames.get(added.entity) else {
        return;
    };
    sets.get_mut(tank)
        .expect("game's damage model: an armour frame's tank carries its set")
        .rebase(added.entity);
}

/// The set of a fixture body from its live hierarchy: every descendant collider on the armour or
/// crew box layer is a part, and the body its only frame.
#[cfg(test)]
pub(crate) fn fixture(world: &mut World, root: Entity) {
    use avian3d::prelude::CollisionLayers;
    let mut nodes = Vec::new();
    let mut parts = Vec::new();
    let mut walk = vec![(root, None)];
    while let Some((entity, node)) = walk.pop() {
        let Some(children) = world.get::<Children>(entity) else {
            continue; // A leaf.
        };
        for &child in children {
            let transform = *world
                .get::<Transform>(child)
                .expect("a fixture child is placed");
            let part = world.get::<CollisionLayers>(child).is_some_and(|layers| {
                layers.memberships.has_all(crate::Layer::Armor)
                    || layers.memberships.has_all(crate::Layer::Witness)
            });
            if part {
                let shape = world
                    .get::<Collider>(child)
                    .expect("a layered fixture child is a collider")
                    .clone();
                parts.push((child, node, transform, shape));
            } else {
                nodes.push(Node {
                    entity: child,
                    parent: node,
                    transform,
                    frame: false,
                });
                walk.push((child, Some(nodes.len() - 1)));
            }
        }
    }
    let set = ArmourSet::build(
        root,
        &nodes,
        parts
            .iter()
            .map(|(collider, node, pose, shape)| PartSource {
                collider: *collider,
                node: *node,
                pose: *pose,
                shape,
            }),
    );
    set.insert(&mut world.commands(), root);
    world.flush();
}

/// Test support: `root`'s set against its live hierarchy. Its parts are every descendant collider
/// on the armour or crew box layer, once each; its frames the root and every descendant node
/// `is_frame` names; each part sits in its nearest frame above it, at the pose its rest chain of
/// `Transform`s gives it; and each frame's box holds its parts. Class invariants only.
#[cfg(test)]
pub(crate) fn assert_matches(
    world: &World,
    root: Entity,
    is_frame: impl Fn(&World, Entity) -> bool,
) {
    use avian3d::prelude::CollisionLayers;
    let mut layered = Vec::new();
    let mut framed = vec![root];
    let mut walk = vec![root];
    while let Some(entity) = walk.pop() {
        for &child in world.get::<Children>(entity).into_iter().flatten() {
            let on = |layer| {
                world
                    .get::<CollisionLayers>(child)
                    .is_some_and(|layers| layers.memberships.has_all(layer))
            };
            if on(crate::Layer::Armor) || on(crate::Layer::Witness) {
                layered.push(child);
            }
            if is_frame(world, child) {
                framed.push(child);
            }
            walk.push(child);
        }
    }
    let set = world
        .get::<ArmourSet>(root)
        .expect("the tank carries its armour set");
    let mut nodes: Vec<Entity> = set.frames().iter().map(|frame| frame.node).collect();
    let mut parts: Vec<Entity> = set
        .frames()
        .iter()
        .flat_map(|frame| frame.parts.iter().map(|part| part.collider))
        .collect();
    nodes.sort();
    framed.sort();
    parts.sort();
    layered.sort();
    assert_eq!(
        nodes, framed,
        "the frames are the root and every moving node"
    );
    assert_eq!(
        parts, layered,
        "every armour and crew box collider is one part"
    );
    // A pose through the live `Transform` chain, from `entity` up to (excluding) `top`.
    let chain = |top: Entity, mut entity: Entity| {
        let mut pose = Pose::IDENTITY;
        while entity != top {
            pose = link(*world.get::<Transform>(entity).unwrap(), "fixture") * pose;
            entity = world.get::<ChildOf>(entity).unwrap().parent();
        }
        pose
    };
    for (index, frame) in set.frames().iter().enumerate() {
        let bounds = frame.bounds;
        for part in &frame.parts {
            let mut above = world.get::<ChildOf>(part.collider).unwrap().parent();
            while above != root && !is_frame(world, above) {
                above = world.get::<ChildOf>(above).unwrap().parent();
            }
            assert_eq!(
                frame.node, above,
                "part {} is in its nearest frame",
                part.collider
            );
            let live = |frame: usize| {
                let frame = &set.frames()[frame];
                frame.offset * link(*world.get::<Transform>(frame.node).unwrap(), "fixture")
            };
            let (composed, rest) = (
                set.in_base(index, &live) * part.pose,
                chain(root, part.collider),
            );
            assert!(
                composed.position.distance(rest.position) < 1.0e-4
                    && composed.rotation.angle_between(rest.rotation) < 1.0e-4,
                "part {} composes to its rest pose: {composed:?} against {rest:?}",
                part.collider
            );
            let held = bounds.expect("a frame carrying a part has a box");
            let posed = part.bounds.posed(part.pose);
            assert!(
                posed.min.cmpge(held.min).all() && posed.max.cmple(held.max).all(),
                "frame {index}'s box holds part {}",
                part.collider
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic tank (fixture data): a hull plate on the root, a hull node carrying a turret
    /// servo, a pitch servo on the turret, a recoiling barrel on the pitch servo, and a fixed
    /// mantlet node under the barrel; one box part on every node and one on the root.
    struct Synthetic {
        world: World,
        root: Entity,
        nodes: Vec<Entity>,
        set: ArmourSet,
    }

    const HULL: usize = 0;
    const TURRET: usize = 1;
    const PITCH: usize = 2;
    const BARREL: usize = 3;
    const MANTLET: usize = 4;

    fn synthetic() -> Synthetic {
        let mut world = World::new();
        let root = world.spawn(Transform::IDENTITY).id();
        let rest = [
            (None, Transform::from_xyz(0.0, 0.5, 0.0), false),
            (
                Some(HULL),
                Transform::from_xyz(0.2, 1.0, -0.4).with_rotation(Quat::from_rotation_y(0.3)),
                true,
            ),
            (Some(TURRET), Transform::from_xyz(0.0, 0.4, 1.1), true),
            (Some(PITCH), Transform::from_xyz(0.0, 0.0, 0.5), true),
            (
                Some(BARREL),
                Transform::from_xyz(0.0, 0.1, 2.0).with_rotation(Quat::from_rotation_x(0.1)),
                false,
            ),
        ];
        let mut entities: Vec<Entity> = Vec::new();
        let mut nodes = Vec::new();
        for (parent, transform, frame) in rest {
            let above = parent.map_or(root, |index| entities[index]);
            let entity = world.spawn((transform, ChildOf(above))).id();
            entities.push(entity);
            nodes.push(Node {
                entity,
                parent,
                transform,
                frame,
            });
        }
        let shape = Collider::cuboid(0.6, 0.3, 0.9);
        let mut parts = Vec::new();
        for (node, pose) in [
            (None, Transform::from_xyz(0.0, -0.2, 0.0)),
            (Some(HULL), Transform::from_xyz(1.0, 0.0, 0.3)),
            (
                Some(TURRET),
                Transform::from_xyz(0.0, 0.3, 0.0).with_rotation(Quat::from_rotation_z(0.2)),
            ),
            (Some(PITCH), Transform::IDENTITY),
            (Some(BARREL), Transform::from_xyz(0.0, 0.0, 1.5)),
            (Some(MANTLET), Transform::IDENTITY),
        ] {
            let above = node.map_or(root, |index| entities[index]);
            let collider = world
                .spawn((
                    pose,
                    ChildOf(above),
                    shape.clone(),
                    avian3d::prelude::CollisionLayers::new(
                        [crate::Layer::Armor],
                        avian3d::prelude::LayerMask::NONE,
                    ),
                ))
                .id();
            parts.push((collider, node, pose));
        }
        let set = ArmourSet::build(
            root,
            &nodes,
            parts.iter().map(|&(collider, node, pose)| PartSource {
                collider,
                node,
                pose,
                shape: &shape,
            }),
        );
        Synthetic {
            world,
            root,
            nodes: entities,
            set,
        }
    }

    /// `entity`'s pose in `root`'s frame through the live `Transform` chain.
    fn chain(world: &World, root: Entity, mut entity: Entity) -> Pose {
        let mut pose = Pose::IDENTITY;
        while entity != root {
            let local = world.get::<Transform>(entity).unwrap();
            pose = link(*local, "fixture") * pose;
            entity = world.get::<ChildOf>(entity).unwrap().parent();
        }
        pose
    }

    /// The live pose of each frame in its carrier, as the march reads it: the offset, then the
    /// frame node's own `Transform`.
    fn live(world: &World, set: &ArmourSet) -> impl Fn(usize) -> Pose {
        let poses: Vec<Pose> = set
            .frames()
            .iter()
            .map(|frame| match frame.carrier {
                None => Pose::IDENTITY,
                Some(_) => {
                    frame.offset * link(*world.get::<Transform>(frame.node).unwrap(), "fixture")
                }
            })
            .collect();
        move |frame| poses[frame]
    }

    /// The game's damage model: the set's frames are the root and exactly the moving nodes, and
    /// every part sits in its nearest frame at or above its node, at the pose its rest chain gives
    /// it. Every reader composes a part's pose from them (body, frames, fixed pose); a part put in
    /// a farther frame, or a fixed pose missing a link, poses it wrongly once that frame moves.
    #[test]
    fn parts_sit_in_their_nearest_frame_at_their_rest_pose() {
        let Synthetic {
            mut world,
            root,
            nodes,
            set,
            ..
        } = synthetic();
        set.insert(&mut world.commands(), root);
        world.flush();
        let moving = [nodes[TURRET], nodes[PITCH], nodes[BARREL]];
        assert_matches(&world, root, |_, node| moving.contains(&node));
    }

    /// The game's damage model: a frame's box holds its parts however the frames move, and the
    /// body box holds them all. Field containment and source clearance read only these boxes; a
    /// box built from the rest pose instead of the frame's own, or a carrier's motion left out,
    /// lets a turned turret's part outside. Fixture angles and recoil travel.
    #[test]
    fn frame_and_body_boxes_hold_their_parts_under_articulation() {
        let Synthetic {
            mut world,
            root,
            nodes,
            set,
            ..
        } = synthetic();
        for (yaw, pitch, recoil) in [(0.0, 0.0, 0.0), (2.5, -0.15, 0.4), (-1.2, 0.25, 0.1)] {
            world.get_mut::<Transform>(nodes[TURRET]).unwrap().rotation =
                Quat::from_rotation_y(0.3 + yaw);
            world.get_mut::<Transform>(nodes[PITCH]).unwrap().rotation =
                Quat::from_rotation_x(pitch);
            world
                .get_mut::<Transform>(nodes[BARREL])
                .unwrap()
                .translation = Vec3::new(0.0, 0.0, 0.5 - recoil);
            let in_carrier = live(&world, &set);
            let body = set.body_box(&in_carrier).expect("the fixture has parts");
            for (index, frame) in set.frames().iter().enumerate() {
                let posed = frame.bounds.unwrap().posed(set.in_base(index, &in_carrier));
                for part in &frame.parts {
                    let pose = chain(&world, root, part.collider);
                    for corner in 0..8 {
                        let pick = |bit: usize, axis: usize| {
                            if corner & bit == 0 {
                                part.bounds.min[axis]
                            } else {
                                part.bounds.max[axis]
                            }
                        };
                        let local = Vec3::new(pick(1, 0), pick(2, 1), pick(4, 2));
                        let at = pose.position + pose.rotation * local;
                        for (what, held) in [("frame", posed), ("body", body)] {
                            assert!(
                                at.cmpge(held.min - 1.0e-5).all()
                                    && at.cmple(held.max + 1.0e-5).all(),
                                "{what} box holds part {} at {yaw}, {pitch}, {recoil}",
                                part.collider
                            );
                        }
                    }
                }
            }
        }
    }

    /// The game's damage model: a frame node that becomes a rigid body (a launched turret) is its
    /// own base, and every frame it carries is based on it: the root's box no longer holds them.
    /// A tank box still holding the launched assembly contains fields in an empty hull's frame.
    #[test]
    fn a_launched_frame_rebases_itself_and_every_frame_it_carries() {
        let Synthetic {
            mut world,
            root,
            nodes,
            set,
            ..
        } = synthetic();
        world.add_observer(rebase);
        set.insert(&mut world.commands(), root);
        world.flush();
        let turret = nodes[TURRET];
        world.entity_mut(turret).remove::<ChildOf>();
        world.entity_mut(turret).insert(RigidBody::Dynamic);
        world.flush();
        let set = world.get::<ArmourSet>(root).unwrap();
        let bases: Vec<(Entity, Entity)> = set
            .frames()
            .iter()
            .map(|frame| (frame.node, frame.base))
            .collect();
        assert_eq!(
            bases,
            [
                (root, root),
                (turret, turret),
                (nodes[PITCH], turret),
                (nodes[BARREL], turret)
            ]
        );
        assert_eq!(set.frames()[1].carrier, None);
        let body = set.body_box(&|_| Pose::IDENTITY).unwrap();
        let root_frame = set.frames()[0].bounds.unwrap();
        assert_eq!(body, root_frame, "the root's box holds only its own frame");
    }
}
