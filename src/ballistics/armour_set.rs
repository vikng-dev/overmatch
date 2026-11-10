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

use std::sync::atomic::{AtomicU64, Ordering};

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

    /// The box grown by `by` on every side.
    pub(super) fn grown(self, by: f32) -> Self {
        Self {
            min: self.min - Vec3::splat(by),
            max: self.max + Vec3::splat(by),
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
    /// The box of its shape in its frame's coordinates.
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
    /// Where its parts can be over the last tick ([`snapshot`]); `None` for a set no tick boundary
    /// has closed yet, which is not in the scene the march reads, or one with no part.
    reach: Option<Reach>,
}

/// The game's damage model: where a set's parts can be over the last tick. `held` is a world box
/// holding every part point at every fraction of it: their box at the tick's end grown by
/// `motion`, the DERIVED bound on any part point's displacement over the tick
/// ([`ArmourSet::motion_bound`]).
#[derive(Clone, Copy, Debug)]
pub(super) struct Reach {
    pub held: Bounds,
    pub motion: f32,
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
/// meet the same endpoint poses; between them a point differs by at most `max |w''| / 8` for the
/// difference `w` of the two paths, the chord-to-arc gap of the links' turns (DERIVED): about
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
pub(crate) struct Node<'a> {
    /// Its name in the tank's model, for a refusal to name.
    pub name: &'a str,
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

/// A link a frame or part composes through, as a pose. The set composes rigidly, so each such link
/// must carry unit scale: the bake certifies it on armour nodes (`bake::manifold_gate`) and on
/// every rigid-pose chain (`L2.UNIT_SCALE`).
fn link(transform: Transform, node: &dyn std::fmt::Display) -> Pose {
    assert_eq!(
        transform.scale.to_array().map(f32::to_bits),
        [1.0f32.to_bits(); 3],
        "game's damage model: the armour set composes rigidly through {node}, which carries scale \
         {:?}: apply the object scale in the .blend (Ctrl+A → Scale, on the node and every \
         parent) and re-export",
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
        nodes: &[Node<'_>],
        parts: impl IntoIterator<Item = PartSource<'a>>,
    ) -> Self {
        let parts: Vec<PartSource<'a>> = parts.into_iter().collect();
        // The nodes a frame or part composes through: each frame and part node and every node above
        // one. A node with none beneath it (a collision proxy) is never composed and may carry any
        // scale the bake permits.
        let mut composed = vec![false; nodes.len()];
        let starts = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.frame)
            .map(|(index, _)| Some(index))
            .chain(parts.iter().map(|part| part.node));
        for mut at in starts {
            while let Some(index) = at.filter(|&index| !composed[index]) {
                composed[index] = true;
                at = nodes[index].parent;
            }
        }
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
        // Each composed node's frame and its pose there.
        let mut placed: Vec<Option<(usize, Pose)>> = Vec::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            if !composed[index] {
                placed.push(None);
                continue;
            }
            let (carrier, above) = match node.parent {
                None => (0, Pose::IDENTITY),
                Some(parent) => {
                    assert!(parent < index, "armour nodes are listed parent first");
                    placed[parent].expect("a node above a composed node is composed")
                }
            };
            if node.frame {
                let rest = above * link(node.transform, &node.name);
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
                placed.push(Some((frames.len() - 1, Pose::IDENTITY)));
            } else {
                placed.push(Some((carrier, above * link(node.transform, &node.name))));
            }
        }
        for part in parts {
            let (frame, above) = part.node.map_or((0, Pose::IDENTITY), |node| {
                placed[node].expect("a part's node is composed")
            });
            let on = part.node.map_or("the root", |node| nodes[node].name);
            let pose = above * link(part.pose, &format_args!("the part on {on}"));
            let local = part.shape.aabb(Vec3::ZERO, Quat::IDENTITY);
            let posed = Bounds {
                min: local.min,
                max: local.max,
            }
            .posed(pose);
            let frame = &mut frames[frame];
            frame.bounds = Some(frame.bounds.map_or(posed, |held| held.union(posed)));
            frame.parts.push(Part {
                collider: part.collider,
                pose,
                bounds: posed,
            });
        }
        Self {
            frames,
            reach: None,
        }
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
    pub(super) fn root(&self) -> Entity {
        self.frames[0].node
    }

    pub(super) fn reach(&self) -> Option<Reach> {
        self.reach
    }

    /// The frame index and part `part`, whose set this is, names.
    pub(super) fn located(&self, part: &ArmourPart) -> (usize, &Part) {
        (part.frame, &self.frames[part.frame].parts[part.part])
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
    /// frame only when that pose was written since the last boundary. A set `born` since the last
    /// boundary has no earlier visible trajectory: each frame starts with identical endpoints at
    /// its live pose, as a born collider does ([`super::motion::ColliderMotion`]). Returns the
    /// frames written.
    fn advance(&mut self, nodes: &Query<Ref<Transform>>, born: bool) -> u64 {
        let mut written = 0;
        for frame in &mut self.frames {
            if frame.carrier.is_none() {
                continue; // A frame on its own base is identity in itself.
            }
            let local = nodes
                .get(frame.node)
                .expect("game's damage model: an armour frame's node lives as long as its tank");
            frame.poses.from = frame.poses.to;
            if born || local.is_changed() {
                frame.poses.to = frame.offset * link(*local, &frame.node);
                written += 1;
            }
            if born {
                frame.poses.from = frame.poses.to;
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

    /// Frame `frame`'s world pose, its base body's given by `body` and each frame's pose in its
    /// carrier by `in_carrier`.
    pub(super) fn in_world(
        &self,
        frame: usize,
        body: &impl Fn(Entity) -> Pose,
        in_carrier: &impl Fn(usize) -> Pose,
    ) -> Pose {
        body(self.frames[frame].base) * self.in_base(frame, in_carrier)
    }

    /// The world box of every part, posed as [`Self::in_world`] poses its frame; `None` for a set
    /// with no part.
    pub(super) fn world_box(
        &self,
        body: &impl Fn(Entity) -> Pose,
        in_carrier: &impl Fn(usize) -> Pose,
    ) -> Option<Bounds> {
        self.frames
            .iter()
            .enumerate()
            .filter_map(|(index, frame)| {
                Some(frame.bounds?.posed(self.in_world(index, body, in_carrier)))
            })
            .reduce(Bounds::union)
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
/// rigid body (a launched assembly, `damage::launch_turrets_on_cookoff`). The new body has no
/// captured endpoints until the next tick's `motion::seed_births`, so nothing may compose a part it
/// carries in between: the launch runs in `DamageConsequences`, after the tick's march.
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

/// The game's damage model: advance every tank's frame poses for the tick just stepped, and record
/// where its parts can be over it ([`Reach`]). Each tank's set is its own: they advance in
/// parallel, sharing only the count of frames written.
pub(super) fn snapshot(
    mut sets: Query<&mut ArmourSet>,
    nodes: Query<Ref<Transform>>,
    history: Res<ColliderMotion>,
    stages: Option<Res<crate::cost::stages::Stages>>,
) {
    let writes = AtomicU64::new(0);
    let bodies = &history.bodies;
    sets.par_iter_mut().for_each(|mut set| {
        let born = set.is_added();
        writes.fetch_add(set.advance(&nodes, born), Ordering::Relaxed);
        let motion = set.motion_bound(bodies);
        let end = set.world_box(
            &|base| {
                bodies
                    .get(&base)
                    .expect(
                        "game's damage model: an armour frame's base body has captured endpoints",
                    )
                    .to
            },
            &|frame| set.frames[frame].poses.to,
        );
        set.reach = end.map(|end| Reach {
            held: end.grown(motion),
            motion,
        });
    });
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
                    name: "a fixture node",
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
            pose = link(*world.get::<Transform>(entity).unwrap(), &entity) * pose;
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
                frame.offset * link(*world.get::<Transform>(frame.node).unwrap(), &frame.node)
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
            let posed = local_box(world, part.collider).posed(part.pose);
            assert!(
                posed.min.cmpge(held.min).all() && posed.max.cmple(held.max).all(),
                "frame {index}'s box holds part {}",
                part.collider
            );
        }
    }
}

/// Test support: the box of part collider `part`'s shape in its own coordinates.
#[cfg(test)]
fn local_box(world: &World, part: Entity) -> Bounds {
    let local = world
        .get::<Collider>(part)
        .expect("a part is a collider")
        .aabb(Vec3::ZERO, Quat::IDENTITY);
    Bounds {
        min: local.min,
        max: local.max,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic tank (fixture data): a hull plate on the root, a hull node carrying a turret
    /// servo, a pitch servo on the turret, a recoiling barrel on the pitch servo, a fixed mantlet
    /// node under the barrel, and a scaled proxy node holding no part; one box part on the root and
    /// on every node but the proxy.
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
    /// Fixture tick, s.
    const DT: f32 = 1.0 / 64.0;

    /// The synthetic nodes' `(name, parent, rest pose, moves)`, parent first (fixture data).
    fn rest() -> [(&'static str, Option<usize>, Transform, bool); 6] {
        [
            ("hull", None, Transform::from_xyz(0.0, 0.5, 0.0), false),
            (
                "turret",
                Some(HULL),
                Transform::from_xyz(0.2, 1.0, -0.4).with_rotation(Quat::from_rotation_y(0.3)),
                true,
            ),
            (
                "pitch",
                Some(TURRET),
                Transform::from_xyz(0.0, 0.4, 1.1),
                true,
            ),
            (
                "barrel",
                Some(PITCH),
                Transform::from_xyz(0.0, 0.0, 0.5),
                true,
            ),
            (
                "mantlet",
                Some(BARREL),
                Transform::from_xyz(0.0, 0.1, 2.0).with_rotation(Quat::from_rotation_x(0.1)),
                false,
            ),
            // A collision proxy's node: the bake permits it any scale, and no part composes it.
            (
                "proxy",
                Some(HULL),
                Transform::from_xyz(1.0, 0.0, 0.0).with_scale(Vec3::splat(2.0)),
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

    /// Spawn the synthetic hierarchy under `root`: its node entities and its set, not inserted.
    fn spawn(world: &mut World, root: Entity) -> (Vec<Entity>, ArmourSet) {
        let mut entities: Vec<Entity> = Vec::new();
        let mut nodes = Vec::new();
        for (name, parent, transform, frame) in rest() {
            let above = parent.map_or(root, |index| entities[index]);
            let entity = world.spawn((transform, ChildOf(above))).id();
            entities.push(entity);
            nodes.push(Node {
                name,
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
        (entities, set)
    }

    fn synthetic() -> Synthetic {
        let mut world = World::new();
        let root = world.spawn(Transform::IDENTITY).id();
        let (nodes, set) = spawn(&mut world, root);
        Synthetic {
            world,
            root,
            nodes,
            set,
        }
    }

    /// The eight corners of `bounds`.
    fn corners(bounds: Bounds) -> [Vec3; 8] {
        std::array::from_fn(|corner| {
            Vec3::select(
                bevy::math::BVec3::new(corner & 1 != 0, corner & 2 != 0, corner & 4 != 0),
                bounds.max,
                bounds.min,
            )
        })
    }

    /// `entity`'s pose in `root`'s frame through the live `Transform` chain.
    fn chain(world: &World, root: Entity, mut entity: Entity) -> Pose {
        let mut pose = Pose::IDENTITY;
        while entity != root {
            let local = world.get::<Transform>(entity).unwrap();
            pose = link(*local, &entity) * pose;
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
                    frame.offset * link(*world.get::<Transform>(frame.node).unwrap(), &frame.node)
                }
            })
            .collect();
        move |frame| poses[frame]
    }

    /// One tick's fixture motion: the body's velocity, m/s, and turn about y, rad/s; the turret's
    /// and pitch servo's turns, rad/s; the barrel's recoil, m/s.
    #[derive(Clone, Copy)]
    struct Motion {
        velocity: Vec3,
        spin: f32,
        yaw: f32,
        pitch: f32,
        recoil: f32,
    }

    /// Fixture motions: each link alone, then every link at once in two senses.
    fn motions() -> [Motion; 7] {
        let still = Motion {
            velocity: Vec3::ZERO,
            spin: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            recoil: 0.0,
        };
        [
            Motion {
                velocity: Vec3::new(8.0, 0.0, 3.0),
                ..still
            },
            Motion { spin: 1.0, ..still },
            Motion { yaw: 0.5, ..still },
            Motion {
                pitch: 0.3,
                ..still
            },
            Motion {
                recoil: 14.0,
                ..still
            },
            Motion {
                velocity: Vec3::new(8.0, 0.0, 3.0),
                spin: 1.0,
                yaw: 0.5,
                pitch: 0.3,
                recoil: 14.0,
            },
            Motion {
                velocity: Vec3::new(-2.0, 1.0, 0.0),
                spin: -0.4,
                yaw: 2.0,
                pitch: -0.6,
                recoil: -3.0,
            },
        ]
    }

    /// Move the synthetic `set` from its rest through `motion` over one tick: the bodies table its
    /// parts compose with, the body starting at a fixture pose. A link at rest keeps identical
    /// endpoints.
    fn moved(set: &mut ArmourSet, root: Entity, motion: Motion) -> HashMap<Entity, Poses> {
        let turns = [
            Pose {
                position: Vec3::ZERO,
                rotation: Quat::from_rotation_y(motion.yaw * DT),
            },
            Pose {
                position: Vec3::ZERO,
                rotation: Quat::from_rotation_x(motion.pitch * DT),
            },
            Pose {
                position: Vec3::Z * (motion.recoil * DT),
                rotation: Quat::IDENTITY,
            },
        ];
        // The turret's, the pitch servo's and the barrel's frames follow the root's.
        for (frame, turn) in set.frames[1..].iter_mut().zip(turns) {
            let poses = &mut frame.poses;
            poses.to = if turn == Pose::IDENTITY {
                poses.from
            } else {
                poses.from * turn
            };
        }
        let from = Pose {
            position: Vec3::new(1.0, 0.0, 2.0),
            rotation: Quat::from_rotation_y(0.4),
        };
        let to = Pose {
            position: from.position + motion.velocity * DT,
            rotation: Quat::from_rotation_y(motion.spin * DT) * from.rotation,
        };
        HashMap::from_iter([(root, Poses { from, to })])
    }

    /// The game's damage model: the set's frames are the root and exactly the moving nodes, and
    /// every part sits in its nearest frame at or above its node, at the pose its rest chain gives
    /// it, a scaled node no part composes through included. Every reader composes a part's pose
    /// from them (body, frames, fixed pose); a part put in a farther frame, or a fixed pose missing
    /// a link, poses it wrongly once that frame moves.
    #[test]
    fn parts_sit_in_their_nearest_frame_at_their_rest_pose() {
        let Synthetic {
            mut world,
            root,
            nodes,
            set,
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
                    for local in corners(local_box(&world, part.collider)) {
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

    /// The game's damage model: over a tick a part point moves at most the set's motion bound and
    /// lies in the swept box at both ends, and its part is still exactly when no link of its chain
    /// moves. A broad query grows the set's boxes by the bound, source clearance reads the swept
    /// box, and discovery casts a still part as a static ray; a link left out of any of them misses
    /// a moving part. Fixture motions: each link alone, then every link at once.
    #[test]
    fn a_parts_motion_stays_within_the_sets_bound_and_swept_box() {
        // Resolution: f32 rounding of metre-scale pose compositions, m.
        const ROUNDING: f32 = 1.0e-5;
        let Synthetic {
            world,
            root,
            mut set,
            ..
        } = synthetic();
        for motion in motions() {
            let bodies = moved(&mut set, root, motion);
            let bound = set.motion_bound(&bodies);
            let swept = set.swept_box().expect("the synthetic root carries parts");
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
                    let (start, end) = (composed.at(0.0), composed.at(1.0));
                    assert_eq!(
                        composed.still(),
                        (start == end).then_some(end),
                        "part {} is still exactly when its chain is",
                        part.collider
                    );
                    for local in corners(local_box(&world, part.collider)) {
                        let at = |fraction: f32| {
                            let pose = composed.at(fraction);
                            pose.position + pose.rotation * local
                        };
                        for step in 0..=16 {
                            let fraction = step as f32 / 16.0;
                            let travel = at(fraction).distance(at(1.0));
                            assert!(
                                travel <= bound + ROUNDING,
                                "part {} at {fraction}: {travel} m past the bound {bound} m",
                                part.collider
                            );
                        }
                        for fraction in [0.0, 1.0] {
                            let pose =
                                set.in_base(index, &|frame| set.at(frame, fraction)) * part.pose;
                            let at = pose.position + pose.rotation * local;
                            assert!(
                                at.cmpge(swept.min - ROUNDING).all()
                                    && at.cmple(swept.max + ROUNDING).all(),
                                "the swept box holds part {} at {fraction}",
                                part.collider
                            );
                        }
                    }
                }
            }
        }
    }

    /// The game's damage model: a part's contact velocity is its composed path's own derivative,
    /// a central difference of that path, through every link's motion. The crossing chain reads it
    /// for a struck point's velocity; a link left out of the product rule, or turned in the wrong
    /// axes, leaves the difference. Fixture motions.
    #[test]
    fn a_parts_contact_velocity_is_its_composed_paths_derivative() {
        // Resolution: the central difference's half step, in tick fractions, and its f32 rounding
        // at metre scale over that step, m/s (its truncation error is below a micrometre per s).
        const STEP: f32 = 1.0 / 32.0;
        const SPEED_ROUNDING: f32 = 1.0e-2;
        let Synthetic {
            world,
            root,
            mut set,
            ..
        } = synthetic();
        for motion in motions() {
            let bodies = moved(&mut set, root, motion);
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
                    for local in corners(local_box(&world, part.collider)) {
                        let at = |fraction: f32| {
                            let pose = composed.at(fraction);
                            pose.position + pose.rotation * local
                        };
                        for step in 1..16 {
                            let fraction = step as f32 / 16.0;
                            let difference =
                                (at(fraction + STEP) - at(fraction - STEP)) / (2.0 * STEP * DT);
                            let velocity = composed.velocity_at(fraction, local, DT);
                            assert!(
                                velocity.distance(difference) < SPEED_ROUNDING,
                                "part {} at {fraction}: velocity {velocity} against {difference}",
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

    /// `root`'s physics pose and each of `tracked`'s local poses, at a tick end.
    struct End {
        body: Pose,
        locals: HashMap<Entity, Transform>,
    }

    fn tick_end(world: &World, root: Entity, tracked: &[Entity]) -> End {
        use avian3d::prelude::{Position, Rotation};
        End {
            body: Pose {
                position: world.get::<Position>(root).unwrap().0,
                rotation: world.get::<Rotation>(root).unwrap().0,
            },
            locals: tracked
                .iter()
                .map(|&entity| (entity, *world.get::<Transform>(entity).unwrap()))
                .collect(),
        }
    }

    /// Where the point `local` of part `part`, under `root`, is at `fraction` of the tick from
    /// `start` to `finish`: the body's pose and each link's local pose lerped and slerped on its
    /// own, read from avian and the hierarchy rather than from the set (fixture derivation).
    fn derived(
        world: &World,
        root: Entity,
        (start, finish): (&End, &End),
        part: Entity,
        local: Vec3,
        fraction: f32,
    ) -> Vec3 {
        let lerp = |a: Pose, b: Pose| Pose {
            position: a.position.lerp(b.position, fraction),
            rotation: a.rotation.slerp(b.rotation, fraction),
        };
        let pose = |transform: &Transform| Pose {
            position: transform.translation,
            rotation: transform.rotation,
        };
        let (mut point, mut entity) = (local, part);
        while entity != root {
            let link = lerp(pose(&start.locals[&entity]), pose(&finish.locals[&entity]));
            point = link.position + link.rotation * point;
            entity = world.get::<ChildOf>(entity).unwrap().parent();
        }
        let body = lerp(start.body, finish.body);
        body.position + body.rotation * point
    }

    /// For each `(part, point, fraction)`: whether a broad query of a 0.1 mm box about the world
    /// `point` returns `part` at the moment `fraction` and over the tick's sweep, and whether
    /// `point` lies outside the box of `part`'s frame at the tick's end.
    fn found(app: &mut App, root: Entity, queries: Vec<(Entity, Vec3, f32)>) -> Vec<[bool; 3]> {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once(move |world: super::super::ProjectileMarchWorld| {
                let set = world.sets.get(root).unwrap();
                let sweep = super::super::motion::Sweep {
                    from: 0.0,
                    to: 1.0,
                    dt: 1.0,
                };
                queries
                    .iter()
                    .map(|&(part, point, fraction)| {
                        let probe = super::super::collect::Probe::Region(
                            avian3d::prelude::ColliderAabb::from_min_max(
                                point - Vec3::splat(1.0e-4),
                                point + Vec3::splat(1.0e-4),
                            ),
                        );
                        world.moment.set(Some(fraction));
                        let mut returned = [false; 2];
                        for (clock, sweep) in [None, Some(sweep)].into_iter().enumerate() {
                            world.sweep.set(sweep);
                            world.near(probe, &mut |entity, _| returned[clock] |= entity == part);
                        }
                        let frame = set.located(world.parts.get(part).unwrap()).0;
                        let held = set.frames()[frame].bounds.unwrap().posed(world.frame_pose(
                            set,
                            frame,
                            super::super::SetClock::Swept,
                        ));
                        let outside = point.cmplt(held.min).any() || point.cmpgt(held.max).any();
                        [returned[0], returned[1], outside]
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap()
    }

    /// Whether `root`'s held box, and each of its `parts`' boxes from a broad query over a sweep
    /// and at the moment `fraction`, hold the part's `inner` points at `fraction`.
    fn boxed(app: &mut App, root: Entity, parts: &[Entity], inner: &[Vec3], fraction: f32) -> bool {
        use bevy::ecs::system::RunSystemOnce;
        let (parts, inner) = (parts.to_vec(), inner.to_vec());
        app.world_mut()
            .run_system_once(move |world: super::super::ProjectileMarchWorld| {
                let everywhere = super::super::collect::Probe::Region(
                    avian3d::prelude::ColliderAabb::from_min_max(
                        Vec3::splat(-1.0e4),
                        Vec3::splat(1.0e4),
                    ),
                );
                let holds = |min: Vec3, max: Vec3, pose: Pose| {
                    inner.iter().all(|&local| {
                        let at = pose.position + pose.rotation * local;
                        at.cmpge(min).all() && at.cmple(max).all()
                    })
                };
                world.moment.set(Some(fraction));
                let poses: Vec<Pose> = parts
                    .iter()
                    .map(|&part| world.pose(part).unwrap())
                    .collect();
                let held = world.sets.get(root).unwrap().reach().unwrap().held;
                let mut ok = poses.iter().all(|&pose| holds(held.min, held.max, pose));
                let sweep = super::super::motion::Sweep {
                    from: 0.0,
                    to: 1.0,
                    dt: 1.0,
                };
                for clock in [Some(sweep), None] {
                    world.sweep.set(clock);
                    let mut found = Vec::new();
                    world.near(everywhere, &mut |entity, bounds| {
                        found.push((entity, bounds))
                    });
                    ok &= parts.iter().zip(&poses).all(|(part, &pose)| {
                        found.iter().any(|(entity, bounds)| {
                            entity == part && holds(bounds.min, bounds.max, pose)
                        })
                    });
                }
                ok
            })
            .unwrap()
    }

    /// The game's damage model: a set is born with identical endpoints at its live pose; at every
    /// tick end a part's composed pose is the physics pose its collider holds, the start of the
    /// tick the pose it held at the last one, the tank box at either end holds every part, and a
    /// broad query's boxes (the set's held box, each part's box over a sweep and at a moment) hold
    /// every part at every fraction, before and after a launch rebases its frame, and until it a
    /// 0.1 mm query about a point of a turret, gun or barrel part, its path derived from avian and
    /// the hierarchy, returns that part at the start, middle and end of the tick, at the moment and
    /// over the sweep, one such point lying outside its frame's end-pose box; while the collider
    /// history keeps no entry for any part. The walk reads parts only through the set; a birth
    /// swept from the blueprint rest, a frame left at an old pose or read at the wrong end, a
    /// carrier's motion dropped, a box not grown by the set's motion, a frame or tank box rejecting
    /// at the end pose alone, or a part kept in the history fails it. Fixture: the synthetic tank on a kinematic body driving and
    /// turning, its turret and pitch servos turning and its barrel moving every tick at fixture
    /// rates, its turret launched as a kinematic body midway.
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
        let (entities, set) = spawn(app.world_mut(), root);
        let colliders: Vec<Entity> = set
            .frames
            .iter()
            .flat_map(|frame| frame.parts.iter().map(|part| part.collider))
            .collect();
        // The parts the servos and the recoil move against the hull.
        let articulated: Vec<Entity> = set.frames[1..]
            .iter()
            .flat_map(|frame| frame.parts.iter().map(|part| part.collider))
            .collect();
        let tracked: Vec<Entity> = entities.iter().chain(&colliders).copied().collect();
        let mut outside = false;
        set.insert(&mut app.world_mut().commands(), root);
        app.world_mut().flush();
        // Every fixture part has one shape; its corners are taken 0.1 % inside its box, clear of
        // f32 rounding in the box test (Resolution).
        let inner = {
            let held = local_box(app.world(), colliders[0]);
            let (centre, half) = ((held.min + held.max) * 0.5, (held.max - held.min) * 0.4995);
            corners(Bounds {
                min: centre - half,
                max: centre + half,
            })
        };
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
        let physics = |world: &World| -> Vec<Pose> {
            colliders
                .iter()
                .map(|&collider| Pose {
                    position: world.get::<Position>(collider).unwrap().0,
                    rotation: world.get::<Rotation>(collider).unwrap().0,
                })
                .collect()
        };
        // Each part's composed pose at `fraction`, and whether its tank's box then holds it.
        let composed = |app: &mut App, fraction: f32| -> Vec<(Pose, bool)> {
            let parts = colliders.clone();
            app.world_mut()
                .run_system_once(move |world: super::super::ProjectileMarchWorld| {
                    world.moment.set(Some(fraction));
                    let held = world.tank_box(root);
                    parts
                        .iter()
                        .map(|&part| {
                            let pose = world.pose(part).unwrap();
                            let inside = held.as_ref().is_some_and(|bounds| {
                                inner.iter().all(|&local| {
                                    bounds.holds(pose.position + pose.rotation * local)
                                })
                            });
                            (pose, inside)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };
        let near = |a: &[(Pose, bool)], b: &[Pose], what: &str| {
            for (index, ((a, _), b)) in a.iter().zip(b).enumerate() {
                assert!(
                    a.position.distance(b.position) < 1.0e-4
                        && a.rotation.angle_between(b.rotation) < 1.0e-4,
                    "{what}, part {index}: {a:?} against {b:?}"
                );
            }
        };
        // The first update that steps is the set's first boundary, and the fixture moves the pitch
        // servo off its rest in that same tick.
        for _ in 0..4 {
            app.update();
            if *app.world().get::<Transform>(pitch).unwrap() != rest()[PITCH].2 {
                break;
            }
        }
        let world = app.world();
        assert_ne!(*world.get::<Transform>(pitch).unwrap(), rest()[PITCH].2);
        for frame in &world.get::<ArmourSet>(root).unwrap().frames[1..] {
            let live =
                frame.offset * link(*world.get::<Transform>(frame.node).unwrap(), &frame.node);
            assert_eq!(
                (frame.poses.from, frame.poses.to),
                (live, live),
                "a set is born with identical endpoints at its live pose"
            );
        }
        for _ in 0..2 {
            app.update();
        }
        let mut last = physics(app.world());
        for tick in 0..24 {
            let start_end = tick_end(app.world(), root, &tracked);
            if tick == 12 {
                // A launch, as `damage::launch_turrets_on_cookoff` makes one: the assembly leaves
                // the hierarchy as its own body at its world pose.
                let world = app.world_mut();
                let mut pose = Pose::IDENTITY;
                let mut entity = turret;
                while entity != root {
                    pose = link(*world.get::<Transform>(entity).unwrap(), &entity) * pose;
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
            let now = physics(app.world());
            let (start, end) = (composed(&mut app, 0.0), composed(&mut app, 1.0));
            near(&end, &now, &format!("tick {tick} end"));
            for fraction in [0.0, 0.5, 1.0] {
                assert!(
                    boxed(&mut app, root, &colliders, &inner, fraction),
                    "tick {tick}: the set's boxes hold every part at fraction {fraction}"
                );
            }
            if tick != 12 {
                // The launch tick's turret parts start on a body born that tick.
                near(&start, &last, &format!("tick {tick} start"));
            }
            if tick < 12 {
                // Until the launch the root carries every part.
                assert!(
                    start.iter().chain(&end).all(|&(_, inside)| inside),
                    "tick {tick}: the tank box at each end holds every part"
                );
                let (world, finish) = (app.world(), tick_end(app.world(), root, &tracked));
                let mut queries = Vec::new();
                for (&part, fraction) in articulated
                    .iter()
                    .flat_map(|part| [(part, 0.0), (part, 0.5), (part, 1.0)])
                {
                    for &local in &inner {
                        let ends = (&start_end, &finish);
                        queries.push((
                            part,
                            derived(world, root, ends, part, local, fraction),
                            fraction,
                        ));
                    }
                }
                for (query, [moment, swept, beyond]) in
                    queries.iter().zip(found(&mut app, root, queries.clone()))
                {
                    assert!(
                        moment && swept,
                        "tick {tick}: a query about {query:?} returns its part"
                    );
                    outside |= beyond;
                }
            }
            last = now;
            let history = app
                .world()
                .resource::<super::super::motion::ColliderMotion>();
            assert!(
                colliders
                    .iter()
                    .all(|collider| !history.colliders.contains_key(collider)),
                "the collider history keeps no armour part"
            );
        }
        assert!(
            outside,
            "an articulated part's point lies outside its frame's box at the tick's end"
        );
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
