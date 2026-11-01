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

/// A loose trunk body: its segment pools no longer open a second structural cut.
#[derive(Component)]
pub(crate) struct LooseTrunk;

#[derive(Component)]
pub(super) struct DetachedTrunk(pub(super) Entity);

/// The actual loads that freed this trunk, in its map root's frame.
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

/// The shared launch reads exactly the geometry and density that the dynamic body receives.
pub(super) fn trunk_launch(body: &TreeBody, cut: u8, loads: &[Load]) -> Launch {
    let centroid = body.piece_centroid(cut);
    let colliders = (usize::from(cut)..body.kind.segments.len())
        .map(|k| {
            let offset = Vec3::Y * body.prefix[k] - centroid;
            (
                k,
                Collider::compound(vec![(
                    offset,
                    Quat::IDENTITY,
                    segment_collider(&body.kind, k, body.scale),
                )]),
            )
        })
        .collect();
    let loads: Vec<_> = loads
        .iter()
        .filter(|load| load.chunk >= usize::from(cut))
        .copied()
        .collect();
    launch_colliders(centroid, colliders, |_| body.collider_density(), &loads)
}

/// Change ownership after every walk: all segment pools survive, and the old carrier is removed
/// only after its segments have been reparented. Replica segments stay standing until the
/// authoritative pose becomes present at the interpolation cursor; the network adapter owns
/// that collision handoff and the streamed copy.
fn derive_state(
    trees: Query<
        (
            Entity,
            &MapInstance,
            &TreeState,
            &TreeBody,
            &Transform,
            &TreeParts,
            Option<&DetachedTrunk>,
            &TrunkLoads,
        ),
        Changed<TreeState>,
    >,
    replica: Option<Res<ClientReplica>>,
    mut commands: Commands,
) {
    for (root, identity, state, body, transform, parts, previous, loads) in &trees {
        let carrier = match *state {
            TreeState::Standing => None,
            TreeState::Down { cut, .. } if replica.is_none() => {
                let mut launch = trunk_launch(body, cut, &loads.0);
                // Only the mass calculation uses compound wrappers; every actual collider stays
                // a single convex frustum so the game's damage-model walk reads one solid.
                launch.colliders.clear();
                let at = transform.translation + transform.rotation * launch.centroid;
                let piece = commands
                    .spawn((
                        Name::new("TreePiece"),
                        TreePiece {
                            tree: identity.ordinal,
                            cut,
                        },
                        LooseTrunk,
                        Transform::from_translation(at).with_rotation(transform.rotation),
                        Position(at),
                        Rotation(transform.rotation),
                        Visibility::default(),
                        crate::debris::DebrisPiece,
                        crate::eased_dynamic_body(),
                        LinearVelocity(transform.rotation * launch.velocity),
                        AngularVelocity(transform.rotation * launch.spin),
                        PendingLaunch::new(*transform, launch),
                    ))
                    .id();
                commands.entity(root).insert(DetachedTrunk(piece));
                Some((piece, cut, body.piece_centroid(cut)))
            }
            TreeState::Down { .. } => None,
        };
        for (k, &segment) in parts.0.iter().enumerate() {
            let freed = carrier.filter(|(_, cut, _)| k >= usize::from(*cut));
            let (parent, offset, layers) = match freed {
                Some((piece, _, centroid)) => {
                    (piece, Vec3::Y * body.prefix[k] - centroid, piece_layers())
                }
                None => (root, Vec3::Y * body.prefix[k], segment_layers()),
            };
            let mut entity = commands.entity(segment);
            entity.insert((ChildOf(parent), Transform::from_translation(offset), layers));
            if freed.is_some() {
                entity
                    .insert(ColliderDensity(body.collider_density()))
                    .remove::<ColliderDisabled>();
            } else {
                entity.remove::<ColliderDisabled>();
            }
        }
        if let Some(previous) = previous {
            commands.entity(previous.0).despawn();
        }
        if carrier.is_none() {
            commands.entity(root).remove::<DetachedTrunk>();
        }
        commands.entity(root).insert(TrunkLoads::default());
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
    (derive_state, place_colliders, refit_colliders()).chain()
}

/// The game's damage model: a march delivers its real line and casing momentum to a trunk
/// freed in that march, before body construction. A press-only or unsupported departure carries
/// no invented starting spin. Spall has no sourced fragment mass and supplies no launch blow.
pub(super) fn read_shell(
    marched: On<crate::ballistics::ShellMarched>,
    segments: Query<(&TreeSegment, &ChildOf)>,
    mut trees: Query<(&TreeState, &TreeBody, &Transform, &mut TrunkLoads), Without<DetachedTrunk>>,
    shells: Query<Option<&crate::ballistics::SpallMarks>>,
) {
    for line in &marched.lines {
        let Ok((&TreeSegment(k), parent)) = segments.get(line.crossing.volume) else {
            continue;
        };
        let Ok((state, body, root, mut loads)) = trees.get_mut(parent.parent()) else {
            continue;
        };
        let TreeState::Down { cut, .. } = *state else {
            continue;
        };
        if k < cut {
            continue;
        }
        let into = root.rotation.inverse();
        loads.0.extend(Load::line(
            usize::from(k),
            &body.kind.masonry,
            into * (line.crossing.entry - root.translation),
            into * (line.crossing.end - root.translation),
            0.0,
            into * line.incoming * line.momentum,
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
            let Ok((state, _, root, mut loads)) = trees.get_mut(parent.parent()) else {
                continue;
            };
            let TreeState::Down { cut, .. } = *state else {
                continue;
            };
            if k < cut {
                continue;
            }
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
        app.world()
            .get::<DetachedTrunk>(root)
            .expect("the changed state freed its body")
            .0
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
                    let launch = trunk_launch(&body, cut, &[load]);
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
}
