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
//!
//! Each frame keeps its pose in its carrier at both ends of the tick ([`snapshot`]), rewritten only
//! when its node's local pose is written. A part's pose at a fraction of the tick is body × frames
//! × fixed pose, each link lerped and slerped on its own ([`Composed`]); the collider history keeps
//! no part ([`super::motion`]).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use avian3d::prelude::{Collider, RigidBody, SimpleCollider};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use super::motion::{ColliderMotion, Pose, Poses};

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
pub(super) struct Part {
    /// The primitive's entity: its identity for keys, HP pools and records.
    pub collider: Entity,
    /// Its pose in its frame, fixed.
    pub pose: Pose,
    /// Its mesh's box in its own coordinates.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the set's narrow candidate test for server discovery"
        )
    )]
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
    /// Its pose in the carrier's frame at both ends of the last tick (`offset × node local`);
    /// identity for a frame that is its own base's body.
    pub poses: Poses,
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

/// Marks an armour part's collider with its place in its tank's set: the walk poses it from the
/// set, and the collider history keeps no entry for it.
#[derive(Component)]
pub(crate) struct ArmourPart {
    tank: Entity,
    frame: usize,
    part: usize,
}

impl ArmourPart {
    pub(crate) fn tank(&self) -> Entity {
        self.tank
    }
}

/// The game's damage model: an armour part's motion over the tick: body(f) × frames(f) × fixed
/// pose, each link's endpoints lerped and slerped.
///
/// The collider history it replaces lerps and slerps each part's composed world pose instead. Both
/// meet the same endpoint poses; between them a point differs by the chord-to-arc gap of the
/// links' turns, at most `max |w''| / 8` over the tick for the difference `w` of the two paths
/// (DERIVED; the bound and its terms are stated in this module's
/// `the_composed_path_stays_within_its_derived_gap_of_the_per_collider_path`): about
/// `r θ² / 8` for a lever `r` turned `θ` in a tick, 0.09 mm at 3 m and 1 rad/s over 1/64 s.
#[derive(Clone, Copy)]
pub(super) struct Composed<'a> {
    body: Poses,
    set: &'a ArmourSet,
    frame: usize,
    part: Pose,
}

impl Composed<'_> {
    /// The part's pose at `fraction` of the tick.
    pub(super) fn at(self, fraction: f32) -> Pose {
        self.body.at(fraction)
            * self
                .set
                .in_base(self.frame, &|frame| self.set.at(frame, fraction))
            * self.part
    }

    /// The velocity of the part-local point `local` at `fraction`: the product rule over the
    /// chain, each link's own lerp/slerp velocity ([`Poses::velocity_at`]) in world axes.
    pub(super) fn velocity_at(self, fraction: f32, local: Vec3, dt: f32) -> Vec3 {
        let in_frame = self.part.position + self.part.rotation * local;
        let frame = self
            .set
            .in_base(self.frame, &|frame| self.set.at(frame, fraction));
        let in_body = frame.position + frame.rotation * in_frame;
        self.body.velocity_at(fraction, in_body, dt)
            + self.body.at(fraction).rotation
                * self.set.chain_velocity(self.frame, fraction, in_frame, dt)
    }

    /// The part's pose, when neither its body nor any frame of its chain moves over the tick.
    pub(super) fn still(self) -> Option<Pose> {
        let mut moving = self.body.from != self.body.to;
        let mut link = self.frame;
        while let Some(carrier) = self.set.frames[link].carrier {
            let poses = self.set.frames[link].poses;
            moving |= poses.from != poses.to;
            link = carrier;
        }
        (!moving).then(|| self.at(1.0))
    }
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
fn link(transform: Transform, what: &str, entity: Entity) -> Pose {
    assert_eq!(
        transform.scale.to_array().map(f32::to_bits),
        [1.0f32.to_bits(); 3],
        "game's damage model: armour {what} {entity} carries scale {:?}; armour is posed through \
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
            poses: Poses {
                from: Pose::IDENTITY,
                to: Pose::IDENTITY,
            },
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
                let rest = above * link(node.transform, "frame", node.entity);
                frames.push(Frame {
                    node: node.entity,
                    carrier: Some(carrier),
                    offset: above,
                    poses: Poses {
                        from: rest,
                        to: rest,
                    },
                    base: root,
                    parts: Vec::new(),
                    bounds: None,
                });
                placed.push((frames.len() - 1, Pose::IDENTITY));
            } else {
                placed.push((carrier, above * link(node.transform, "node", node.entity)));
            }
        }
        for part in parts {
            let (frame, above) = part.node.map_or((0, Pose::IDENTITY), |node| placed[node]);
            let pose = above * link(part.pose, "part", part.collider);
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

    /// Insert the set on `root` and mark its frame nodes and part colliders, in one command batch.
    pub(crate) fn insert(self, commands: &mut Commands, root: Entity) {
        for (index, frame) in self.frames.iter().enumerate() {
            if index > 0 {
                commands
                    .entity(frame.node)
                    .insert(ArmourFrame { tank: root });
            }
            for (part, held) in frame.parts.iter().enumerate() {
                commands.entity(held.collider).insert(ArmourPart {
                    tank: root,
                    frame: index,
                    part,
                });
            }
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

    /// Frame `frame`'s pose in its carrier at `fraction` of the tick.
    fn at(&self, frame: usize, fraction: f32) -> Pose {
        self.frames[frame].poses.at(fraction)
    }

    /// The motion of `part`, whose set this is, over the tick, its base body's endpoints read
    /// from `bodies`.
    pub(super) fn composed<'a>(
        &'a self,
        part: &ArmourPart,
        bodies: &HashMap<Entity, Poses>,
    ) -> Composed<'a> {
        let frame = &self.frames[part.frame];
        Composed {
            body: *bodies
                .get(&frame.base)
                .expect("game's damage model: an armour frame's base body has captured endpoints"),
            set: self,
            frame: part.frame,
            part: frame.parts[part.part].pose,
        }
    }

    /// The velocity, in the axes of `frame`'s base, of the point at `point` in `frame`'s
    /// coordinates, from the motion of `frame` and every carrier above it.
    fn chain_velocity(&self, frame: usize, fraction: f32, point: Vec3, dt: f32) -> Vec3 {
        let link = &self.frames[frame];
        match link.carrier {
            // A frame on its own base moves only with that body.
            None => Vec3::ZERO,
            Some(carrier) => {
                let at = link.poses.at(fraction);
                let turn = self
                    .in_base(carrier, &|frame| self.at(frame, fraction))
                    .rotation;
                turn * link.poses.velocity_at(fraction, point, dt)
                    + self.chain_velocity(carrier, fraction, at.position + at.rotation * point, dt)
            }
        }
    }

    /// The root body's box over the whole tick: [`Self::body_box`] at both ends.
    pub(super) fn swept_box(&self) -> Option<Bounds> {
        let start = self.body_box(&|frame| self.frames[frame].poses.from)?;
        let end = self.body_box(&|frame| self.frames[frame].poses.to)?;
        Some(start.union(end))
    }

    /// Close each frame's last tick and record this one from its node's local pose, rewriting a
    /// frame only when that pose was written since the last boundary. Returns the frames written.
    fn advance(&mut self, nodes: &Query<Ref<Transform>>) -> u64 {
        let mut written = 0;
        for frame in &mut self.frames {
            if frame.carrier.is_none() {
                continue; // A frame on its own base is identity in itself.
            }
            let local = nodes
                .get(frame.node)
                .expect("game's damage model: an armour frame's node lives as long as its tank");
            frame.poses.from = frame.poses.to;
            if local.is_changed() {
                frame.poses.to = frame.offset * link(*local, "frame", frame.node);
                written += 1;
            }
        }
        written
    }

    /// DERIVED conservative displacement over the tick of any part point, for tree-query padding.
    /// A part point's path is a product of link paths, so its displacement between two fractions
    /// is at most the sum over links of each link's [`super::motion::chord_bound`] at the point's
    /// lever about that link's origin. A lever is at most the frame box's radius plus each inner
    /// link's larger endpoint translation: rotations keep lengths, and a lerp stays within its
    /// endpoints' larger length.
    fn motion_bound(&self, bodies: &HashMap<Entity, Poses>) -> f32 {
        let mut largest = 0.0f64;
        for (index, frame) in self.frames.iter().enumerate() {
            let Some(bounds) = frame.bounds else {
                continue; // A frame carrying no part moves no point.
            };
            let mut lever = super::motion::box_radius(bounds.min, bounds.max);
            let mut sum = 0.0;
            let mut link = index;
            while let Some(carrier) = self.frames[link].carrier {
                let poses = self.frames[link].poses;
                sum += super::motion::chord_bound(poses, lever);
                lever += f64::from(poses.from.position.length().max(poses.to.position.length()));
                link = carrier;
            }
            let body = bodies
                .get(&frame.base)
                .expect("game's damage model: an armour frame's base body has captured endpoints");
            largest = largest.max(sum + super::motion::chord_bound(*body, lever));
        }
        super::motion::outward(largest)
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
                frame.poses = Poses {
                    from: Pose::IDENTITY,
                    to: Pose::IDENTITY,
                };
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

/// The game's damage model: advance every tank's frame poses for the tick just stepped, and widen
/// the tree-query padding [`super::motion::snapshot`] set by the sets' motion. Each tank's set is
/// its own: they advance in parallel and meet only at the padding's maximum.
pub(super) fn snapshot(
    mut sets: Query<&mut ArmourSet>,
    nodes: Query<Ref<Transform>>,
    mut history: ResMut<ColliderMotion>,
    stages: Option<Res<crate::cost::stages::Stages>>,
) {
    // Non-negative f32 bounds order as their bit patterns.
    let largest = AtomicU32::new(history.largest_motion.to_bits());
    let writes = AtomicU64::new(0);
    let bodies = &history.bodies;
    sets.par_iter_mut().for_each(|mut set| {
        writes.fetch_add(set.advance(&nodes), Ordering::Relaxed);
        largest.fetch_max(set.motion_bound(bodies).to_bits(), Ordering::Relaxed);
    });
    history.largest_motion = f32::from_bits(largest.into_inner());
    if let Some(stages) = stages {
        stages.add(
            crate::cost::stages::Work::FramePoseWrites,
            writes.into_inner(),
        );
    }
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
            pose = link(*world.get::<Transform>(entity).unwrap(), "fixture", entity) * pose;
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
                frame.offset
                    * link(
                        *world.get::<Transform>(frame.node).unwrap(),
                        "fixture",
                        frame.node,
                    )
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

    /// The synthetic nodes' `(parent, rest pose, moves)`, parent first (fixture data).
    fn rest() -> [(Option<usize>, Transform, bool); 5] {
        [
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
        ]
    }

    /// The synthetic parts' `(node, pose there)` (fixture data).
    fn part_rest() -> [(Option<usize>, Transform); 6] {
        [
            (None, Transform::from_xyz(0.0, -0.2, 0.0)),
            (Some(HULL), Transform::from_xyz(1.0, 0.0, 0.3)),
            (
                Some(TURRET),
                Transform::from_xyz(0.0, 0.3, 0.0).with_rotation(Quat::from_rotation_z(0.2)),
            ),
            (Some(PITCH), Transform::IDENTITY),
            (Some(BARREL), Transform::from_xyz(0.0, 0.0, 1.5)),
            (Some(MANTLET), Transform::IDENTITY),
        ]
    }

    fn synthetic() -> Synthetic {
        let mut world = World::new();
        let root = world.spawn(Transform::IDENTITY).id();
        let mut entities: Vec<Entity> = Vec::new();
        let mut nodes = Vec::new();
        for (parent, transform, frame) in rest() {
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
        for (node, pose) in part_rest() {
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
            pose = link(*local, "fixture", entity) * pose;
            entity = world.get::<ChildOf>(entity).unwrap().parent();
        }
        pose
    }

    /// The live pose of each frame in its carrier: the offset, then the frame node's own
    /// `Transform`.
    fn live(world: &World, set: &ArmourSet) -> impl Fn(usize) -> Pose {
        let poses: Vec<Pose> = set
            .frames()
            .iter()
            .map(|frame| match frame.carrier {
                None => Pose::IDENTITY,
                Some(_) => {
                    frame.offset
                        * link(
                            *world.get::<Transform>(frame.node).unwrap(),
                            "fixture",
                            frame.node,
                        )
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

    /// The rotation angle, rad, from `poses.from` to `poses.to` along the shorter arc.
    fn turn(poses: Poses) -> f32 {
        let delta = poses.to.rotation * poses.from.rotation.inverse();
        2.0 * delta.xyz().length().atan2(delta.w.abs())
    }

    /// The game's damage model: a part's composed path stays within its DERIVED gap of the path
    /// the per-collider history gave it, the lerp and slerp of the part's composed endpoint poses.
    /// Both paths meet those endpoints, so their difference `w` at a part point vanishes at both
    /// ends of the tick, and `|w(f)| ≤ f (1 - f) max |w''| / 2 ≤ max |w''| / 8` (the scalar
    /// interpolation bound, along any unit axis). The per-collider path turns a point `y` from the
    /// part's origin at a constant `θ` per tick: `|v''| ≤ θ² |y|`. The composed path applies its
    /// links from the innermost out, each moving the point's origin `d` and turning it `a` per
    /// tick at lever `ρ`; with `s` and `c` the bounds on the point's first and second derivative
    /// inside the link, `c ← a² ρ + 2 a s + c`, `s ← d + a ρ + s`, `ρ ← ρ + max |t|` (a lerped
    /// translation has no second derivative). The gap is at most `(c + θ² |y|) / 8`. For a body
    /// turning 1 rad/s over a 1/64 s tick and a part point 3 m out it is `r θ² / 8`, 0.09 mm.
    /// The contact velocity is the composed path's own derivative, a central difference of it.
    /// Discovery, the crossing chain and every field walk read the composed path and its velocity;
    /// a composition reading a frame at the wrong fraction, or a link left out of the pose or the
    /// velocity's product rule, leaves the bound. Fixture
    /// motions: the synthetic tank's body turning and driving, its servos turning and its barrel
    /// recoiling at fixture rates.
    #[test]
    fn the_composed_path_stays_within_its_derived_gap_of_the_per_collider_path() {
        // Fixture tick, s.
        const DT: f32 = 1.0 / 64.0;
        // Resolution: f32 rounding of metre-scale pose compositions, m.
        const ROUNDING: f32 = 4.0e-6;
        // Resolution: the central difference's half step, in tick fractions, and its f32 rounding
        // at metre scale over that step, m/s (its truncation error is below a micrometre per s).
        const STEP: f32 = 1.0 / 32.0;
        const SPEED_ROUNDING: f32 = 1.0e-2;
        const {
            assert!(
                3.0 * DT * DT / 8.0 < 1.0e-4,
                "the headline gap is under 0.1 mm"
            )
        };
        let Synthetic { root, mut set, .. } = synthetic();
        // (body velocity m/s, body rad/s, turret rad/s, pitch rad/s, recoil m/s), fixture data.
        for (velocity, spin, yaw, pitch, recoil) in [
            (Vec3::ZERO, 1.0, 0.0, 0.0, 0.0),
            (Vec3::new(8.0, 0.0, 3.0), 1.0, 0.5, 0.3, 14.0),
            (Vec3::new(-2.0, 1.0, 0.0), -0.4, 2.0, -0.6, 3.0),
        ] {
            for (frame, motion) in [
                (
                    TURRET,
                    Pose {
                        position: Vec3::ZERO,
                        rotation: Quat::from_rotation_y(yaw * DT),
                    },
                ),
                (
                    PITCH,
                    Pose {
                        position: Vec3::ZERO,
                        rotation: Quat::from_rotation_x(pitch * DT),
                    },
                ),
                (
                    BARREL,
                    Pose {
                        position: Vec3::Z * (recoil * DT),
                        rotation: Quat::IDENTITY,
                    },
                ),
            ] {
                // Frames follow node order: the turret's is 1, the pitch servo's 2, the barrel's 3.
                let index = frame - TURRET + 1;
                let rest = set.frames[index].poses.from;
                set.frames[index].poses.to = rest * motion;
            }
            let from = Pose {
                position: Vec3::new(1.0, 0.0, 2.0),
                rotation: Quat::from_rotation_y(0.4),
            };
            let body = Poses {
                from,
                to: Pose {
                    position: from.position + velocity * DT,
                    rotation: Quat::from_rotation_y(spin * DT) * from.rotation,
                },
            };
            let bodies = HashMap::from_iter([(root, body)]);
            for (index, frame) in set.frames.iter().enumerate() {
                for (number, part) in frame.parts.iter().enumerate() {
                    let composed = set.composed(
                        &ArmourPart {
                            tank: root,
                            frame: index,
                            part: number,
                        },
                        &bodies,
                    );
                    let old = Poses {
                        from: composed.at(0.0),
                        to: composed.at(1.0),
                    };
                    for corner in 0..8 {
                        let pick = |bit: usize, axis: usize| {
                            if corner & bit == 0 {
                                part.bounds.min[axis]
                            } else {
                                part.bounds.max[axis]
                            }
                        };
                        let y = Vec3::new(pick(1, 0), pick(2, 1), pick(4, 2));
                        let (mut lever, mut s, mut c) = (
                            (part.pose.position + part.pose.rotation * y).length(),
                            0.0,
                            0.0,
                        );
                        let mut links = Vec::new();
                        let mut link = index;
                        while let Some(carrier) = set.frames[link].carrier {
                            links.push(set.frames[link].poses);
                            link = carrier;
                        }
                        links.push(body);
                        for poses in links {
                            let (a, d) =
                                (turn(poses), poses.from.position.distance(poses.to.position));
                            c += a * a * lever + 2.0 * a * s;
                            s += d + a * lever;
                            lever += poses.from.position.length().max(poses.to.position.length());
                        }
                        let theta = turn(old);
                        let bound = (c + theta * theta * y.length()) / 8.0;
                        for step in 0..=32 {
                            let fraction = step as f32 / 32.0;
                            let (was, now) = (old.at(fraction), composed.at(fraction));
                            let gap = (was.position + was.rotation * y)
                                .distance(now.position + now.rotation * y);
                            assert!(
                                gap <= bound + ROUNDING,
                                "part {} corner {corner} at {fraction}: {gap} m against {bound} m",
                                part.collider
                            );
                        }
                        let point = |fraction: f32| {
                            let pose = composed.at(fraction);
                            pose.position + pose.rotation * y
                        };
                        for step in 1..16 {
                            let fraction = step as f32 / 16.0;
                            let difference = (point(fraction + STEP) - point(fraction - STEP))
                                / (2.0 * STEP * DT);
                            let velocity = composed.velocity_at(fraction, y, DT);
                            assert!(
                                velocity.distance(difference) < SPEED_ROUNDING,
                                "part {} corner {corner} at {fraction}: velocity {velocity} \
                                 against {difference}",
                                part.collider
                            );
                        }
                    }
                }
            }
        }
    }

    /// Avian and the shipped collider history ([`super::super::mount_collider_motion`]), after
    /// physics as `sim_plugin` orders it, with the set's launch observer: one fixed step per update.
    fn physics_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            avian3d::prelude::PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(bevy::time::TimeUpdateStrategy::FixedTimesteps(1))
        .configure_sets(
            FixedPostUpdate,
            (
                avian3d::prelude::PhysicsSystems::Writeback,
                crate::state::SimPhase::ColliderPoses,
            )
                .chain(),
        )
        .add_observer(rebase);
        super::super::mount_collider_motion(&mut app);
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// The game's damage model: at every tick end a part's composed pose is the physics pose its
    /// collider holds, the start of the tick the pose it held at the last one, before and after a
    /// launch rebases its frame, while the collider history keeps no entry for any part. The walk
    /// reads parts only through the set; a frame left at an old pose, a carrier's motion dropped,
    /// or a part kept in the history fails it. Fixture: the synthetic tank on a kinematic body
    /// driving and turning, its turret and pitch servos turning and its barrel moving every tick
    /// at fixture rates, its turret launched as a kinematic body midway.
    #[test]
    fn a_parts_composed_pose_meets_its_colliders_physics_pose_at_every_tick_end() {
        use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
        use bevy::ecs::system::RunSystemOnce;
        let mut app = physics_app();
        let root = app
            .world_mut()
            .spawn((
                Transform::from_xyz(2.0, 1.0, -3.0),
                RigidBody::Kinematic,
                LinearVelocity(Vec3::new(3.0, 0.0, 1.0)),
                AngularVelocity(Vec3::new(0.0, 0.8, 0.2)),
            ))
            .id();
        // The synthetic hierarchy, in the app's world.
        let world = app.world_mut();
        let mut entities: Vec<Entity> = Vec::new();
        let mut listed = Vec::new();
        for (parent, transform, frame) in rest() {
            let above = parent.map_or(root, |index| entities[index]);
            let entity = world.spawn((transform, ChildOf(above))).id();
            entities.push(entity);
            listed.push(Node {
                entity,
                parent,
                transform,
                frame,
            });
        }
        let shape = Collider::cuboid(0.6, 0.3, 0.9);
        let mut colliders = Vec::new();
        for (node, pose) in part_rest() {
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
            colliders.push((collider, node, pose));
        }
        ArmourSet::build(
            root,
            &listed,
            colliders.iter().map(|&(collider, node, pose)| PartSource {
                collider,
                node,
                pose,
                shape: &shape,
            }),
        )
        .insert(&mut world.commands(), root);
        world.flush();
        let (turret, pitch, barrel) = (entities[TURRET], entities[PITCH], entities[BARREL]);
        app.add_systems(
            FixedUpdate,
            move |mut locals: Query<&mut Transform, Without<RigidBody>>, mut tick: Local<u32>| {
                *tick += 1;
                let t = *tick as f32;
                // A launched turret's pose is its body's.
                if let Ok(mut local) = locals.get_mut(turret) {
                    local.rotation *= Quat::from_rotation_y(0.02);
                }
                locals.get_mut(pitch).unwrap().rotation =
                    Quat::from_rotation_x(0.1 * (0.3 * t).sin());
                locals.get_mut(barrel).unwrap().translation.z = 0.5 - 0.2 * (0.7 * t).sin().abs();
            },
        );
        let physics = |world: &mut World| -> Vec<Pose> {
            colliders
                .iter()
                .map(|&(collider, ..)| Pose {
                    position: world.get::<Position>(collider).unwrap().0,
                    rotation: world.get::<Rotation>(collider).unwrap().0,
                })
                .collect()
        };
        let composed = |app: &mut App, fraction: f32| -> Vec<Pose> {
            let parts: Vec<Entity> = colliders.iter().map(|&(collider, ..)| collider).collect();
            app.world_mut()
                .run_system_once(move |world: super::super::ProjectileMarchWorld| {
                    world.moment.set(Some(fraction));
                    parts
                        .iter()
                        .map(|&part| world.pose(part).unwrap())
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };
        let near = |a: &[Pose], b: &[Pose], what: &str| {
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                assert!(
                    a.position.distance(b.position) < 1.0e-4
                        && a.rotation.angle_between(b.rotation) < 1.0e-4,
                    "{what}, part {index}: {a:?} against {b:?}"
                );
            }
        };
        for _ in 0..4 {
            app.update();
        }
        let mut last = physics(app.world_mut());
        for tick in 0..24 {
            if tick == 12 {
                // A launch, as `damage::launch_turrets_on_cookoff` makes one: the assembly leaves
                // the hierarchy as its own body at its world pose.
                let world = app.world_mut();
                let mut pose = Pose::IDENTITY;
                let mut entity = turret;
                while entity != root {
                    pose = link(*world.get::<Transform>(entity).unwrap(), "fixture", entity) * pose;
                    entity = world.get::<ChildOf>(entity).unwrap().parent();
                }
                let body = Pose {
                    position: world.get::<Position>(root).unwrap().0,
                    rotation: world.get::<Rotation>(root).unwrap().0,
                } * pose;
                world.entity_mut(turret).remove::<ChildOf>().insert((
                    Position(body.position),
                    Rotation(body.rotation),
                    Transform::from_translation(body.position).with_rotation(body.rotation),
                    RigidBody::Kinematic,
                    AngularVelocity(Vec3::new(0.5, 1.0, 0.0)),
                ));
            }
            app.update();
            let now = physics(app.world_mut());
            near(&composed(&mut app, 1.0), &now, &format!("tick {tick} end"));
            if tick != 12 {
                // The launch tick's turret parts start on a body born that tick.
                near(
                    &composed(&mut app, 0.0),
                    &last,
                    &format!("tick {tick} start"),
                );
            }
            last = now;
            let history = app
                .world()
                .resource::<super::super::motion::ColliderMotion>();
            assert!(
                colliders
                    .iter()
                    .all(|(collider, ..)| !history.colliders.contains_key(collider)),
                "the collider history keeps no armour part"
            );
        }
        let set = app.world().get::<ArmourSet>(root).unwrap();
        assert_eq!(
            set.frames[1].base, turret,
            "the launch rebased the turret frame"
        );
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
