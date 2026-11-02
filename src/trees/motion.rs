//! A freed trunk's physics body in the game's environment destruction (game state only).
//! Segment membership and HP stay with the original entities on authority. A replica receives
//! the body's pose and builds collision geometry from the same spawn data; it simulates nothing.

use super::*;
use crate::buildings::breakage::piece_layers;
use crate::debris::launch::{Launch, PendingLaunch, launch_colliders};
use avian3d::collider_tree::{ColliderTrees, update_moved_collider_aabbs};
use avian3d::prelude::*;

/// Stable segment membership, independent of which body currently owns a segment.
#[derive(Component)]
pub(crate) struct TreeParts(pub(crate) Vec<Entity>);

/// Immutable source identity on a freed body's pose carrier. Geometry is derived from map data.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TreePiece {
    pub(crate) tree: u16,
    pub(crate) cut: u8,
}

impl TreePiece {
    /// Presented carriers partition the trunk in cut order; arrival order never owns geometry.
    /// The upper bound is exclusive, and an absent upper carrier leaves the prior owner intact.
    pub(crate) fn end<'a>(&self, pieces: impl Iterator<Item = &'a Self>, segments: usize) -> usize {
        pieces
            .filter(|other| other.tree == self.tree && other.cut > self.cut)
            .map(|other| usize::from(other.cut))
            .min()
            .unwrap_or(segments)
    }
}

/// A moving construction owner, also used by static replica collision copies.
#[derive(Component)]
pub(crate) struct LooseTrunk;

/// Pending external loads in the original construction frame.
#[derive(Component, Default, Clone)]
pub(crate) struct TrunkLoads(pub(crate) Vec<Load>);

impl TreeBody {
    /// Centre of mass of the freed segments, in the original tree frame, metres.
    pub(crate) fn piece_centroid(&self, cut: u8) -> Vec3 {
        let chunks = &self.structure.chunks[usize::from(cut)..];
        let mass: f32 = chunks.iter().map(|chunk| chunk.mass).sum();
        chunks
            .iter()
            .map(|chunk| chunk.centroid * chunk.mass)
            .sum::<Vec3>()
            / mass
    }

    /// DERIVED: a regular twelve-sided ring has area 3 r² instead of π r². The density correction
    /// preserves the circular data's fresh mass in the polygonal collision approximation.
    pub(crate) fn collider_density(&self) -> f32 {
        self.kind.fresh_density * PI / 3.0
    }
}

impl TrunkState {
    fn partition(&self, owned: std::ops::Range<usize>) -> Self {
        let mut next = self.clone();
        for (k, fate) in next.state.fate.iter_mut().enumerate() {
            *fate = if owned.contains(&k) {
                fracture::Fate::Standing
            } else {
                fracture::Fate::Fallen
            };
        }
        next.owned = owned;
        next
    }

    fn partitions(&self, moving: bool) -> Vec<std::ops::Range<usize>> {
        let first = self.owned.start + usize::from(moving);
        let mut start = self.owned.start;
        (first..self.owned.end)
            .filter(|&k| self.state.cracked[k])
            .chain([self.owned.end])
            .map(|end| {
                let range = start..end;
                start = end;
                range
            })
            .collect()
    }
}

impl TreeBody {
    /// Carrier origins stay at the original suffix centroid even after later splits. Sorted
    /// cuts alone can therefore change membership without changing another carrier's pose frame.
    pub(crate) fn construction_frame(
        &self,
        piece: Option<&TreePiece>,
        at: Vec3,
        rotation: Quat,
    ) -> Transform {
        let origin = piece.map_or(Vec3::ZERO, |piece| self.piece_centroid(piece.cut));
        Transform::from_translation(at - rotation * origin).with_rotation(rotation)
    }
}

fn launch_range(body: &TreeBody, range: std::ops::Range<usize>, loads: &[Load]) -> Launch {
    let cut = u8::try_from(range.start).expect("tree admission bounds segment indices");
    let centroid = body.piece_centroid(cut);
    let colliders = range
        .clone()
        .map(|k| {
            (
                k,
                Collider::compound(vec![(
                    Vec3::Y * body.prefix[k] - centroid,
                    Quat::IDENTITY,
                    segment_collider(&body.kind, k, body.scale),
                )]),
            )
        })
        .collect();
    let loads: Vec<_> = loads
        .iter()
        .filter(|load| range.contains(&load.chunk))
        .copied()
        .collect();
    launch_colliders(centroid, colliders, |_| body.collider_density(), &loads)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one carrier is born with its identity, construction partition, pose and launch"
)]
fn spawn_range(
    commands: &mut Commands,
    tree: u16,
    frame: Transform,
    body: &TreeBody,
    trunk: &TrunkState,
    parts: &TreeParts,
    range: std::ops::Range<usize>,
    mut launch: Launch,
    exchange: bool,
) -> Entity {
    let cut = u8::try_from(range.start).expect("tree admission bounds segment indices");
    launch.colliders.clear();
    let at = frame.translation + frame.rotation * launch.centroid;
    let piece = commands
        .spawn((
            Name::new("TreePiece"),
            TreePiece { tree, cut },
            LooseTrunk,
            body.clone(),
            trunk.partition(range.clone()),
            TreeParts(parts.0.clone()),
            TrunkLoads::default(),
            Transform::from_translation(at).with_rotation(frame.rotation),
            Position(at),
            Rotation(frame.rotation),
            Visibility::default(),
            crate::debris::DebrisPiece,
            crate::eased_dynamic_body(),
            LinearVelocity(frame.rotation * launch.velocity),
            AngularVelocity(frame.rotation * launch.spin),
        ))
        .id();
    reparent(
        commands,
        piece,
        body,
        parts,
        range,
        launch.centroid,
        piece_layers(),
    );
    if exchange {
        commands
            .entity(piece)
            .insert(PendingLaunch::new(frame, launch));
    }
    piece
}

fn reparent(
    commands: &mut Commands,
    parent: Entity,
    body: &TreeBody,
    parts: &TreeParts,
    range: std::ops::Range<usize>,
    origin: Vec3,
    layers: CollisionLayers,
) {
    for k in range {
        commands
            .entity(parts.0[k])
            .insert((
                ChildOf(parent),
                Transform::from_translation(Vec3::Y * body.prefix[k] - origin),
                layers,
                ColliderDensity(body.collider_density()),
            ))
            .remove::<ColliderDisabled>();
    }
}

/// A topology reset restores ownership before the next physics step can read the old seams.
/// Replicas run this in their topology transaction, including frames without a fixed step.
pub(super) fn reset_standing(
    mut trees: Query<
        (
            Entity,
            &MapInstance,
            &TreeState,
            &TreeBody,
            &TreeParts,
            &mut TrunkState,
        ),
        Changed<TreeState>,
    >,
    pieces: Query<(Entity, &TreePiece), With<LooseTrunk>>,
    mut commands: Commands,
) {
    for (root, identity, state, body, parts, mut trunk) in &mut trees {
        if *state != TreeState::Standing {
            continue;
        }
        reparent(
            &mut commands,
            root,
            body,
            parts,
            0..parts.0.len(),
            Vec3::ZERO,
            segment_layers(),
        );
        for (entity, piece) in &pieces {
            if piece.tree == identity.ordinal {
                commands.entity(entity).despawn();
            }
        }
        *trunk = TrunkState::new(&body.structure);
    }
}

/// Map topology only lowers the stump's cut. Existing moving owners keep their construction;
/// a replica's collision handoff waits for the interpolated carrier in the network adapter.
fn derive_state(
    mut trees: Query<
        (
            &MapInstance,
            &TreeState,
            &TreeBody,
            &Transform,
            &TreeParts,
            &mut TrunkState,
            &mut TrunkLoads,
        ),
        Changed<TreeState>,
    >,
    replica: Option<Res<ClientReplica>>,
    mut commands: Commands,
) {
    if replica.is_some() {
        return;
    } // Replicas construct collision only at a presented pose.
    for (identity, state, body, frame, parts, mut trunk, mut loads) in &mut trees {
        let TreeState::Down { cut, .. } = *state else {
            continue;
        }; // No departure while standing.
        trunk.state.cracked[usize::from(cut)] = true;
        let ranges = trunk.partitions(false);
        for range in ranges.iter().skip(1).filter(|range| !range.is_empty()) {
            let launch = launch_range(body, range.clone(), &loads.0);
            spawn_range(
                &mut commands,
                identity.ordinal,
                *frame,
                body,
                &trunk,
                parts,
                range.clone(),
                launch,
                true,
            );
        }
        *trunk = trunk.partition(ranges[0].clone());
        loads.0.clear();
    }
}

/// All same-tick source returns land before a moving carrier changes its occupied mass.
/// DERIVED: each child inherits v + omega cross r at its COM and the parent's omega.
/// The lower carrier survives, retaining its stable cut and pose origin on the wire.
pub(super) fn split_moving(
    mut pieces: Query<
        (
            Entity,
            &TreePiece,
            &TreeBody,
            &mut TrunkState,
            &TreeParts,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &ComputedCenterOfMass,
        ),
        Changed<TrunkState>,
    >,
    mut commands: Commands,
) {
    for (entity, piece, body, mut trunk, parts, at, rotation, velocity, spin, centre) in &mut pieces
    {
        let ranges = trunk.partitions(true);
        if ranges.len() == 1 {
            continue;
        }
        let frame = body.construction_frame(Some(piece), at.0, rotation.0);
        let origin = body.piece_centroid(piece.cut);
        let into = frame.rotation.inverse();
        for range in ranges.iter().skip(1) {
            let mut launch = launch_range(body, range.clone(), &[]);
            launch.velocity =
                into * velocity.0 + (into * spin.0).cross(launch.centre - origin - centre.0);
            launch.spin = into * spin.0;
            spawn_range(
                &mut commands,
                piece.tree,
                frame,
                body,
                &trunk,
                parts,
                range.clone(),
                launch,
                false,
            );
        }
        let lower = launch_range(body, ranges[0].clone(), &[]);
        commands.entity(entity).insert(LinearVelocity(
            velocity.0
                + spin
                    .0
                    .cross(frame.rotation * (lower.centre - origin - centre.0)),
        ));
        *trunk = trunk.partition(ranges[0].clone());
        commands.entity(entity).remove::<Sleeping>();
    }
}

/// A newly derived body's child colliders must be placed before the next cast, even in a frame
/// with no physics step. The expressions match Avian's child-pose propagation.
pub(crate) fn place_colliders(
    roots: Query<(&Transform, &Position, &Rotation, &Children), MovedRoots>,
    mut segments: Query<
        (
            &Transform,
            &mut ColliderTransform,
            &mut Position,
            &mut Rotation,
        ),
        With<TreeSegment>,
    >,
) {
    for (root, body_position, body_rotation, children) in &roots {
        for &child in children {
            let Ok((local, mut collider, mut position, mut rotation)) = segments.get_mut(child)
            else {
                continue; // A map tree's optional view root is not a segment collider.
            };
            let local = ColliderTransform::from(*local);
            let placed = ColliderTransform {
                translation: root.scale * local.translation,
                rotation: local.rotation,
                scale: root.scale * local.scale,
            };
            collider.set_if_neq(placed);
            position.set_if_neq(Position(
                body_position.0 + body_rotation * placed.translation,
            ));
            rotation.set_if_neq((body_rotation.0 * placed.rotation.0).normalize().into());
        }
    }
}

/// A body whose topology or physical pose changed since this placement/refit pass.
type MovedRoots = (
    Without<TreeSegment>,
    Or<(With<TreeBody>, With<LooseTrunk>)>,
    Or<(Changed<TreeState>, Changed<Position>, Changed<Rotation>)>,
);

/// Refit only when a tree or trunk moved; a static forest starts no duplicate global AABB pass.
pub(crate) fn refit_colliders() -> ScheduleConfigs<ScheduleSystem> {
    update_moved_collider_aabbs::<Collider>
        .run_if(resource_exists::<ColliderTrees>)
        .run_if(any_match_filter::<MovedRoots>)
}

pub(super) fn derive_systems() -> ScheduleConfigs<ScheduleSystem> {
    (
        reset_standing,
        derive_state,
        place_colliders,
        refit_colliders(),
    )
        .chain()
}

/// The game's damage model: a march delivers its real line and casing momentum to a trunk
/// freed in that march, before body construction. A press-only or unsupported departure carries
/// no invented starting spin. Spall has no sourced fragment mass and supplies no launch blow.
pub(super) fn read_shell(
    marched: On<crate::ballistics::ShellMarched>,
    segments: Query<(&TreeSegment, &ChildOf)>,
    mut trees: Query<(
        Option<&TreeState>,
        &TreeBody,
        &Position,
        &Rotation,
        Option<&TreePiece>,
        &mut TrunkLoads,
    )>,
    shells: Query<Option<&crate::ballistics::SpallMarks>>,
) {
    for line in &marched.lines {
        let Ok((&TreeSegment(k), parent)) = segments.get(line.crossing.volume) else {
            continue; // A crossing of anything but a trunk segment is another law's.
        };
        let (state, body, position, rotation, piece, mut loads) = trees
            .get_mut(parent.parent())
            .expect("a trunk segment's current owner retains its construction and pending loads");
        if state.is_some_and(|state| *state == TreeState::Standing) {
            continue; // Standing construction receives the line through its segment pools.
        }
        let root = body.construction_frame(piece, position.0, rotation.0);
        let into = root.rotation.inverse();
        loads.0.extend(Load::line(
            usize::from(k),
            &body.kind.masonry,
            into * (line.crossing.entry - root.translation),
            into * (line.crossing.end - root.translation),
            0.0,
            into * (line.momentum - line.entrance_momentum),
        ));
        loads.0.extend(Load::field(
            &[(
                usize::from(k),
                into * (line.crossing.entry - root.translation),
                0.0,
                into * line.entrance_momentum,
            )],
            |_| body.kind.masonry.crush,
        ));
    }
    let Some(burst) = marched.burst else {
        return;
    };
    let spall = shells
        .get(marched.shell)
        .expect("ShellMarched precedes its shell's despawn");
    if let Some(field) = spall.and_then(|spall| spall.burst.as_ref()) {
        let total: f64 = field.rays.iter().map(|ray| f64::from(ray.strength)).sum();
        assert!(
            total.is_finite() && total > 0.0,
            "a burst field has a positive finite angular profile"
        );
        for ray in &field.rays {
            let Some((&TreeSegment(k), parent)) = ray
                .doses
                .iter()
                .find_map(|&(volume, _)| segments.get(volume).ok())
            else {
                continue; // This ray's hard stop is not a trunk segment.
            };
            let (state, body, position, rotation, piece, mut loads) =
                trees.get_mut(parent.parent()).expect(
                    "a trunk segment's current owner retains its construction and pending loads",
                );
            if state.is_some_and(|state| *state == TreeState::Standing) {
                continue; // Standing construction receives the field through its segment pools.
            }
            let root = body.construction_frame(piece, position.0, rotation.0);
            let energy = f64::from(burst.energy) * f64::from(ray.strength) / total;
            let blow = ray.direction * (2.0 * energy / f64::from(burst.sideways_speed)) as f32;
            loads.0.push(Load {
                chunk: usize::from(k),
                at: root.rotation.inverse() * (ray.end - root.translation),
                blow: root.rotation.inverse() * blow,
                press: Vec3::ZERO,
                energy: 0.0,
                crushed: 0.0,
                peak: None,
                origin: Origin::Field,
            });
        }
    }
}

#[cfg(test)]
pub(super) fn fixture_blow(kind: &Arc<TreeKind>, blow: Vec3) -> Load {
    let body = TreeBody::new(kind.clone(), 1.0);
    Load {
        chunk: 2,
        at: Vec3::Y * body.prefix[2],
        blow,
        press: Vec3::ZERO,
        energy: 0.0,
        crushed: 0.0,
        peak: None,
        origin: Origin::Field,
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture_kind;
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// ASSUMED fixture: one fixed step per frame, with the production contact reader and laws.
    fn world(gravity: Vec3) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            crate::contact_transfer::sim_plugin,
            sim_plugin,
            crate::debris::sim_plugin,
            crate::buildings::breakage::sim_plugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity(gravity))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    fn tree(app: &mut App, ordinal: u16, at: Vec3) -> Entity {
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, app.world());
        let root = spawn(
            &mut commands,
            &fixture_kind(),
            MapInstance {
                id: format!("fixture_{ordinal}"),
                ordinal,
            },
            Transform::from_translation(at),
        );
        queue.apply(app.world_mut());
        root
    }

    fn release(app: &mut App, root: Entity, cut: u8, velocity: Vec3) -> Entity {
        let body = app.world().get::<TreeBody>(root).unwrap();
        let mass: f32 = body.structure.chunks[usize::from(cut)..]
            .iter()
            .map(|chunk| chunk.mass)
            .sum();
        let at = body.piece_centroid(cut);
        let chunk = usize::from(cut);
        app.world_mut().entity_mut(root).insert((
            TreeState::Down { cut, dir: 0 },
            TrunkLoads(vec![Load {
                chunk,
                at,
                blow: velocity * mass,
                press: Vec3::ZERO,
                energy: 0.0,
                crushed: 0.0,
                peak: None,
                origin: Origin::Field,
            }]),
        ));
        // MinimalPlugins initializes its clock on the first update.
        for _ in 0..2 {
            app.update();
        }
        app.world_mut()
            .query::<(Entity, &TreePiece)>()
            .iter(app.world())
            .find(|(_, piece)| {
                piece.tree == app.world().get::<MapInstance>(root).unwrap().ordinal
                    && piece.cut == cut
            })
            .expect("the changed state freed its body")
            .0
    }

    /// The game's damage model: moving construction must open another emptied segment's
    /// seam. Fixture: the bottom cut frees all three segments, then the upper pool empties.
    #[test]
    fn a_moving_trunk_opens_a_second_emptied_segments_joint() {
        let mut app = world(Vec3::ZERO);
        let root = tree(&mut app, 0, Vec3::ZERO);
        let parts = app.world().get::<TreeParts>(root).unwrap().0.clone();
        let piece = release(&mut app, root, 0, Vec3::ZERO);
        app.world_mut().trigger(PoolEmptied {
            entity: parts[2],
            at: Vec3::Y * 4.0,
            along: Some(Vec3::X),
        });
        assert!(
            app.world()
                .get::<TrunkState>(piece)
                .is_some_and(|trunk| trunk.state.cracked[2]),
            "a moving trunk retains structural state and opens the second joint"
        );
        app.update();
        let upper = app.world().get::<ChildOf>(parts[2]).unwrap().parent();
        assert_ne!(
            upper, piece,
            "the opened joint separates its actual colliders"
        );
        assert_eq!(app.world().get::<TreePiece>(upper).unwrap().cut, 2);
        assert_eq!(app.world().get::<TrunkState>(piece).unwrap().owned, 0..2);
        assert_eq!(app.world().get::<TrunkState>(upper).unwrap().owned, 2..3);
        app.world_mut().trigger(PoolEmptied {
            entity: parts[1],
            at: Vec3::Y * 2.0,
            along: Some(Vec3::X),
        });
        app.update();
        let middle = app.world().get::<ChildOf>(parts[1]).unwrap().parent();
        assert_ne!(middle, piece);
        assert_ne!(middle, upper);
        for (k, &part) in parts.iter().enumerate() {
            let owner = app.world().get::<ChildOf>(part).unwrap().parent();
            let trunk = app.world().get::<TrunkState>(owner).unwrap();
            assert_eq!(
                trunk.owned,
                k..k + 1,
                "every segment has one construction owner"
            );
        }
    }

    /// Contact receivers must retain construction after release. Fixture: a fast finite-mass
    /// cube strikes the upper segment of an unsupported trunk; the common seam laws open it.
    #[test]
    fn a_moving_trunk_assesses_later_contact_loads() {
        let mut app = world(Vec3::ZERO);
        // Fixture: weak seams make this a routing invariant, independent of shipped grain facts.
        let mut kind = (*fixture_kind()).clone();
        kind.grain.tensile = 1.0e3;
        kind.grain.fracture_energy = 1.0;
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let root = spawn(
            &mut Commands::new(&mut queue, app.world()),
            &Arc::new(kind),
            MapInstance {
                id: "fixture".into(),
                ordinal: 0,
            },
            Transform::IDENTITY,
        );
        queue.apply(app.world_mut());
        let piece = release(&mut app, root, 0, Vec3::ZERO);
        app.world_mut().spawn((
            RigidBody::Dynamic,
            Position(Vec3::new(-2.0, 5.0, 0.0)),
            Rotation::default(),
            Collider::cuboid(0.5, 0.5, 0.5),
            ColliderDensity(10000.0),
            LinearVelocity(Vec3::X * 60.0),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        ));
        assert!(
            !app.world().get::<TrunkState>(piece).unwrap().state.cracked[1..]
                .iter()
                .any(|open| *open)
        );
        let checked = app
            .world()
            .entity(piece)
            .get_ref::<TrunkState>()
            .unwrap()
            .last_changed();
        let mut assessed = false;
        let mut delivered = 0.0_f32;
        let mut opened = false;
        for _ in 0..16 {
            app.update();
            delivered += app
                .world()
                .resource::<ContactTransfers>()
                .split()
                .filter(|(point, ..)| point.structure == piece)
                .map(|(_, blow, _, _)| blow.length())
                .sum::<f32>();
            assessed |= app
                .world()
                .entity(piece)
                .get_ref::<TrunkState>()
                .unwrap()
                .last_changed()
                != checked;
            opened |= app
                .world()
                .get::<TrunkState>(piece)
                .is_some_and(|trunk| trunk.state.cracked[1..].iter().any(|open| *open));
        }
        assert!(delivered > 0.0, "the solver delivered the fixture contact");
        assert!(
            assessed,
            "the contact must run the moving owner's structural assessment"
        );
        assert!(
            opened,
            "later solver contacts assess the moving trunk's remaining seams: {delivered} N s"
        );
    }

    /// Later stump cuts partition only the wood still grounded. Fixture cuts descend from
    /// the tip; already moving owners and each segment's HP component must survive unchanged.
    #[test]
    fn lower_stump_cuts_keep_existing_moving_owners() {
        let mut app = world(Vec3::ZERO);
        let root = tree(&mut app, 0, Vec3::ZERO);
        let parts = app.world().get::<TreeParts>(root).unwrap().0.clone();
        let upper = release(&mut app, root, 2, Vec3::ZERO);
        let pools: Vec<_> = parts
            .iter()
            .map(|&part| *app.world().get::<ComponentHealth>(part).unwrap())
            .collect();
        for cut in [1, 0] {
            app.world_mut().trigger(PoolEmptied {
                entity: parts[cut],
                at: Vec3::ZERO,
                along: Some(Vec3::X),
            });
            app.update();
            assert_eq!(
                app.world().get::<ChildOf>(parts[2]).unwrap().parent(),
                upper
            );
            assert_eq!(
                app.world().get::<TreeState>(root),
                Some(&TreeState::Down {
                    cut: cut as u8,
                    dir: 0
                })
            );
        }
        for (k, &part) in parts.iter().enumerate() {
            let owner = app.world().get::<ChildOf>(part).unwrap().parent();
            assert_eq!(app.world().get::<TreePiece>(owner).unwrap().cut, k as u8);
            assert_eq!(*app.world().get::<ComponentHealth>(part).unwrap(), pools[k]);
        }
    }

    /// A split preserves the parent's rigid velocity field and occupied geometry. Physics users
    /// rely on no invented energy or teleport at release. Fixture: a rotating, translating trunk
    /// opens both remaining seams in one deposit, with a nonidentity construction rotation.
    #[test]
    fn a_split_inherits_the_rigid_velocity_field_without_moving_any_segment() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = world(Vec3::ZERO);
        let root = tree(&mut app, 0, Vec3::ZERO);
        let parts = app.world().get::<TreeParts>(root).unwrap().0.clone();
        let parent = release(&mut app, root, 0, Vec3::ZERO);
        let at = Vec3::new(8.0, 3.0, -2.0);
        let turn = Quat::from_rotation_x(0.7);
        let velocity = Vec3::new(2.0, 3.0, -1.0);
        let spin = Vec3::new(0.1, -0.2, 0.3);
        let com = app.world().get::<ComputedCenterOfMass>(parent).unwrap().0;
        app.world_mut().entity_mut(parent).insert((
            Position(at),
            Rotation(turn),
            LinearVelocity(velocity),
            AngularVelocity(spin),
        ));
        let body = app.world().get::<TreeBody>(parent).unwrap().clone();
        let before: Vec<_> = parts
            .iter()
            .map(|&part| at + turn * app.world().get::<Transform>(part).unwrap().translation)
            .collect();
        for &part in &parts[1..] {
            app.world_mut().trigger(PoolEmptied {
                entity: part,
                at: Vec3::ZERO,
                along: Some(Vec3::X),
            });
        }
        app.world_mut().run_system_once(split_moving).unwrap();
        for (k, &part) in parts.iter().enumerate() {
            let owner = app.world().get::<ChildOf>(part).unwrap().parent();
            let position = app.world().get::<Position>(owner).unwrap().0;
            let rotation = app.world().get::<Rotation>(owner).unwrap().0;
            let local = app.world().get::<Transform>(part).unwrap().translation;
            assert!((position + rotation * local - before[k]).length() < 1e-5);
            // DERIVED from the authored frustum centroid; each resulting owner has one segment.
            let centre = at + turn * (body.structure.chunks[k].centroid - body.piece_centroid(0));
            let expected = velocity + spin.cross(centre - (at + turn * com));
            assert!(
                (app.world().get::<LinearVelocity>(owner).unwrap().0 - expected).length() < 1e-4
            );
            assert!((app.world().get::<AngularVelocity>(owner).unwrap().0 - spin).length() < 1e-6);
            assert_eq!(
                app.world().get::<TrunkState>(owner).unwrap().owned,
                k..k + 1
            );
        }
    }

    /// Every freed trunk leaves with the plastic share it was given, using fresh occupied mass.
    /// Physics and the source's give-back rely on this: whole-blow launch, dry density or a fixed
    /// tipping spin fails. ASSUMED fixture mass ratios, cuts, scales and a central horizontal blow.
    #[test]
    fn a_freed_trunks_momentum_is_the_plastic_share_of_its_blow() {
        for scale in [0.5, 1.0, 2.0] {
            let body = TreeBody::new(fixture_kind(), scale);
            for cut in [0, 1, 2] {
                let mass: f32 = body.structure.chunks[usize::from(cut)..]
                    .iter()
                    .map(|chunk| chunk.mass)
                    .sum();
                for ratio in [0.01, 1.0, 100.0] {
                    let source_mass = mass * ratio;
                    let blow = Vec3::X * source_mass * 3.0;
                    let load = Load {
                        chunk: usize::from(cut),
                        at: body.piece_centroid(cut),
                        blow,
                        press: Vec3::ZERO,
                        energy: 0.0,
                        crushed: 0.0,
                        peak: None,
                        origin: Origin::Body {
                            body: Entity::PLACEHOLDER,
                            mass: source_mass,
                            speed: 3.0,
                            step: 1.0 / 64.0,
                        },
                    };
                    let launch =
                        launch_range(&body, usize::from(cut)..body.kind.segments.len(), &[load]);
                    let expected = blow * mass / (source_mass + mass);
                    assert!((launch.mass - mass).abs() < mass * 1e-5);
                    assert!(
                        (launch.velocity * launch.mass - expected).length()
                            < expected.length() * 1e-5
                    );
                    assert!(
                        launch.spin.length() < 1e-4,
                        "a central blow invents no spin"
                    );
                    assert!(
                        (launch.returns[0].impulse + launch.velocity * launch.mass - blow).length()
                            < blow.length() * 1e-5
                    );
                }
            }
        }
    }

    /// A freed trunk is an ordinary finite-mass contact source. Standing structures rely on the
    /// shared reader seeing its momentum; a static scripted trunk or a body-kind filter cannot
    /// deliver this contact or open the second trunk's structural state. ASSUMED launch fixture.
    #[test]
    fn a_dynamic_trunk_loads_and_fells_a_standing_structure_through_the_contact_reader() {
        let mut app = world(Vec3::ZERO);
        let source = tree(&mut app, 0, Vec3::ZERO);
        let target = tree(&mut app, 1, Vec3::X * 4.0);
        let piece = release(&mut app, source, 0, Vec3::X * 20.0);
        let mut delivered = false;
        for _ in 0..32 {
            app.update();
            delivered |= app.world().resource::<ContactTransfers>().split().any(
                |(point, blow, press, _)| {
                    point.structure == target
                        && point.body == piece
                        && (blow != Vec3::ZERO || press != Vec3::ZERO)
                },
            );
        }
        assert!(
            delivered,
            "the falling body's solver transfer reaches the standing structure"
        );
        assert!(
            matches!(
                app.world().get::<TreeState>(target),
                Some(TreeState::Down { .. })
            ),
            "that contact opens the target's structural cut"
        );
        assert_eq!(
            app.world().get::<RigidBody>(piece),
            Some(&RigidBody::Dynamic)
        );
    }

    /// Rest uses the library's island sleep, and removal of static support wakes the same trunk.
    /// World persistence relies on this: a latched pose, static conversion or missing debris wake
    /// leaves it suspended. ASSUMED horizontal trunk and slab fixture; no rest pose is prescribed.
    #[test]
    fn a_trunk_at_rest_sleeps_and_wakes_when_its_support_goes() {
        let mut app = world(Vec3::NEG_Y * GRAVITY);
        let support = app
            .world_mut()
            .spawn((
                RigidBody::Static,
                Position(Vec3::Y * 0.5),
                Collider::cuboid(20.0, 1.0, 20.0),
                CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
            ))
            .id();
        let source = tree(&mut app, 0, Vec3::Y * 8.0);
        let piece = release(&mut app, source, 0, Vec3::ZERO);
        app.world_mut().entity_mut(piece).insert((
            Position(Vec3::Y * 1.4),
            Rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
        ));
        (0..640)
            .find(|_| {
                app.update();
                app.world().get::<Sleeping>(piece).is_some()
            })
            .expect("the physical trunk settles and sleeps on its support");
        assert_eq!(
            app.world().get::<RigidBody>(piece),
            Some(&RigidBody::Dynamic)
        );
        let before = app.world().get::<Position>(piece).unwrap().0.y;
        app.world_mut().entity_mut(support).despawn();
        app.update();
        assert!(
            app.world().get::<Sleeping>(piece).is_none(),
            "the support's removal wakes it"
        );
        for _ in 0..16 {
            app.update();
        }
        assert!(
            app.world().get::<Position>(piece).unwrap().0.y < before - 0.1,
            "gravity resumes its motion"
        );
    }

    /// Reparenting retains the actual HP components and one collider per segment. The game's
    /// damage model relies on the same pool after a break; copies, refills and lost membership
    /// fail this test, as does a reconnect leaving an old body owning restored segments.
    #[test]
    fn segment_ownership_changes_without_copying_or_refilling_its_pools() {
        let mut app = world(Vec3::ZERO);
        let root = tree(&mut app, 0, Vec3::ZERO);
        let parts = app.world().get::<TreeParts>(root).unwrap().0.clone();
        app.world_mut()
            .get_mut::<ComponentHealth>(parts[2])
            .unwrap()
            .take(5);
        let pools: Vec<_> = parts
            .iter()
            .map(|&part| *app.world().get::<ComponentHealth>(part).unwrap())
            .collect();
        let piece = release(&mut app, root, 1, Vec3::X);
        for (k, &part) in parts.iter().enumerate() {
            assert_eq!(
                app.world().get::<ChildOf>(part).unwrap().parent(),
                if k < 1 { root } else { piece }
            );
            assert_eq!(*app.world().get::<ComponentHealth>(part).unwrap(), pools[k]);
        }
        app.world_mut().entity_mut(root).insert(TreeState::Standing);
        app.update();
        assert!(app.world().get_entity(piece).is_err());
        for (k, &part) in parts.iter().enumerate() {
            assert_eq!(app.world().get::<ChildOf>(part).unwrap().parent(), root);
            assert_eq!(*app.world().get::<ComponentHealth>(part).unwrap(), pools[k]);
            assert_eq!(
                *app.world().get::<CollisionLayers>(part).unwrap(),
                segment_layers()
            );
        }
    }
    /// The world blast reader supplies trunk loads only when its ray reaches the wood. The
    /// shared laws rely on terrain and walls shading those loads; independently sampling a
    /// tree's faces would load the shaded fixture too. ASSUMED source and shield geometry.
    #[test]
    fn a_tree_takes_only_the_blast_loads_the_world_reader_can_reach() {
        #[derive(Resource, Default)]
        struct Observed(Vec<Load>);
        fn observe(trees: Query<&TrunkLoads>, mut seen: ResMut<Observed>) {
            seen.0
                .extend(trees.iter().flat_map(|loads| loads.0.iter().copied()));
        }
        let sample = |shielded: bool| {
            let mut app = world(Vec3::ZERO);
            app.init_resource::<Observed>().add_systems(
                FixedPostUpdate,
                observe
                    .after(crate::buildings::breakage::WallLawSet)
                    .before(TreeFallSet),
            );
            tree(&mut app, 0, Vec3::ZERO);
            if shielded {
                app.world_mut().spawn((
                    RigidBody::Static,
                    Position(Vec3::new(-0.5, 1.0, 0.0)),
                    Collider::cuboid(0.1, 20.0, 20.0),
                    CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
                ));
            }
            for _ in 0..3 {
                app.update();
            }
            app.world_mut()
                .trigger(crate::buildings::breakage::WallCharge {
                    at: Vec3::new(-1.0, 1.0, 0.0),
                    blast: fracture::Blast::of(1.0e6, 0.05, fracture::FIT_ONLY_PRODUCTS),
                });
            app.update();
            std::mem::take(&mut app.world_mut().resource_mut::<Observed>().0)
        };
        let exposed = sample(false);
        assert!(!exposed.is_empty());
        assert!(
            exposed
                .iter()
                .any(|load| load.blow != Vec3::ZERO && load.peak.is_some())
        );
        assert!(
            sample(true).is_empty(),
            "the intervening static surface shades the tree"
        );
    }
    /// The game's damage model delivers each nonzero marched line to a freed trunk once in its
    /// root frame. Trunk launch relies on both the frame turn and the entrance-only impulse.
    /// Fixture: one down trunk, with a rotated root and distinct axial and entrance impulses.
    #[test]
    fn a_marched_line_reaches_the_freed_trunks_loads_once() {
        use crate::ballistics::{DeliveredLine, ShellMarched, VolumeCrossing};
        let mut app = world(Vec3::ZERO);
        let root = tree(&mut app, 0, Vec3::new(3.0, 2.0, 1.0));
        let rotation = Quat::from_rotation_z(0.4);
        app.world_mut().entity_mut(root).insert((
            Transform::from_xyz(3.0, 2.0, 1.0).with_rotation(rotation),
            Position(Vec3::new(3.0, 2.0, 1.0)),
            Rotation(rotation),
        ));
        app.world_mut()
            .entity_mut(root)
            .insert(TreeState::Down { cut: 0, dir: 0 });
        let segment = app
            .world_mut()
            .query::<(Entity, &TreeSegment, &ChildOf)>()
            .iter(app.world())
            .find(|(_, index, parent)| index.0 == 0 && parent.parent() == root)
            .map(|(entity, ..)| entity)
            .unwrap();
        let root_transform = *app.world().get::<Transform>(root).unwrap();
        let entry = Vec3::Y;
        let end = Vec3::Y * 1.2;
        let entrance = Vec3::X * 7.0;
        let axial = Vec3::Y * 11.0;
        let shell = app
            .world_mut()
            .spawn(crate::ballistics::PenetrationMarks::default())
            .id();
        app.world_mut().trigger(ShellMarched {
            shell,
            marks_from: 0,
            cones_from: 0,
            burst: None,
            lines: vec![DeliveredLine {
                crossing: VolumeCrossing {
                    volume: segment,
                    entry: root_transform.translation + rotation * entry,
                    end: root_transform.translation + rotation * end,
                    chord: entry.distance(end),
                    cost: 0.0,
                    energy: 0.0,
                    perforated: true,
                },
                momentum: rotation * (entrance + axial),
                entrance_energy: 0.0,
                entrance_momentum: rotation * entrance,
            }],
        });
        let loads = &app.world().get::<TrunkLoads>(root).unwrap().0;
        assert!(!loads.is_empty());
        assert!(
            (loads.iter().map(|load| load.blow).sum::<Vec3>() - entrance - axial).length() < 1e-4
        );
        let moment: Vec3 = loads.iter().map(|load| load.at.cross(load.blow)).sum();
        assert!(
            (moment - entry.cross(entrance) - ((entry + end) * 0.5).cross(axial)).length() < 1e-4
        );
    }
}
