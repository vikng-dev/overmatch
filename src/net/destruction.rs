//! The game's environment destruction ledger (game state only): topology changes replicate as
//! append-only diffs. Freed trunks use the same interpolated pose carrier as wall pieces;
//! replicas construct static collision copies and never integrate their motion (ADR-0037).

use super::debris_stress::DebrisPose;
use crate::debris::PresentationClock;
use crate::scatter::MapInstance;
use crate::trees::{LooseTrunk, TreeBody, TreeFallSet, TreePiece, TreeSegment, TreeState};
use crate::world::{CurrentWorld, WorldGeneration};
use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::ecs::error::Result;
use bevy::prelude::*;
use bevy_replicon::prelude::{Diffable, EntityCommandsDiffExt};
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A topology fact. The body stream carries motion; no fall clock or latched stop travels here.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerEntry {
    State { index: u16, word: u16 },
}
impl LedgerEntry {
    fn index(self) -> u16 {
        let Self::State { index, .. } = self;
        index
    }
}

/// This world's topology transitions, in authority order, replicated by append-only diff.
#[derive(Component, Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DestructionLedger(pub(crate) Vec<LedgerEntry>);
impl Diffable for DestructionLedger {
    type Diff = LedgerEntry;
    fn apply_diff(&mut self, diff: &LedgerEntry) -> Result<()> {
        self.0.push(*diff);
        Ok(())
    }
}

#[derive(Resource, Default)]
struct Applied {
    ledger: Option<Entity>,
    entries: usize,
}

pub(super) fn install_server(app: &mut App) {
    app.init_resource::<CurrentWorld>()
        .add_systems(Startup, spawn_ledger)
        .add_systems(
            FixedPostUpdate,
            publish_falls
                .after(TreeFallSet)
                .in_set(crate::state::SimPhase::Publish),
        );
}
pub(super) fn install_client(app: &mut App) {
    install_apply(app);
    app.init_resource::<PresentationClock>()
        .add_systems(Update, publish_presentation_clock.before(apply_ledger));
}
pub(crate) fn install_apply(app: &mut App) {
    app.init_resource::<CurrentWorld>()
        .init_resource::<Applied>()
        .add_systems(
            Update,
            (
                apply_ledger,
                crate::trees::derive_systems(),
                attach_trunks,
                follow_trunks,
                crate::trees::place_colliders,
                crate::trees::refit_colliders(),
            )
                .chain(),
        );
}
pub(super) fn spawn_ledger(mut commands: Commands, generation: Res<CurrentWorld>) {
    commands.spawn((
        Name::new("DestructionLedger"),
        DestructionLedger::default(),
        generation
            .0
            .expect("an authority constructs its initial world before publishing"),
        Replicate::to_clients(NetworkTarget::All),
    ));
}
fn publish_falls(
    fallen: Query<(&MapInstance, &TreeState), Changed<TreeState>>,
    ledgers: Query<Entity, With<DestructionLedger>>,
    mut commands: Commands,
) {
    let ledger = ledgers.single().expect("authority startup spawns exactly one destruction ledger before publishing tree transitions");
    for (identity, state) in &fallen {
        if *state != TreeState::Standing {
            commands
                .entity(ledger)
                .apply_diff::<DestructionLedger>(LedgerEntry::State {
                    index: identity.ordinal,
                    word: state.encode(),
                });
        }
    }
}

/// Apply topology from the active authority. Invalid peer words are refused visibly; replacing
/// the ledger first resets every map tree so a reconnect cannot retain another world's cuts.
fn apply_ledger(
    ledgers: Query<(Entity, &DestructionLedger, &WorldGeneration)>,
    generation: Res<CurrentWorld>,
    mut applied: ResMut<Applied>,
    mut trees: Query<(Entity, &MapInstance, &TreeBody, &mut TreeState)>,
) {
    let mut current = ledgers
        .iter()
        .filter(|(_, _, belongs)| Some(**belongs) == generation.0);
    let (ledger, entries, _) = match current.next() {
        Some(ledger) => ledger,
        // Before the first snapshot, or between links, no authority ledger has arrived yet.
        None => return,
    };
    assert!(
        current.next().is_none(),
        "a replica has one active authority ledger; disconnect cleanup removes the old \
             link's replicated entities before a replacement arrives"
    );
    if applied.ledger != Some(ledger) || entries.0.len() < applied.entries {
        // The authority only appends and a replicon snapshot carries its whole list, so a shorter
        // list on the same entity is a malformed peer record: logged, then consumed from its first
        // entry as another link's would be.
        if applied.ledger == Some(ledger) {
            error!(
                "destruction ledger: malformed peer record: {} entries after {} applied from the \
                 same ledger; standing every tree and applying it from its first entry",
                entries.0.len(),
                applied.entries
            );
        }
        for (_, _, _, mut state) in &mut trees {
            state.set_if_neq(TreeState::Standing);
        }
        *applied = Applied {
            ledger: Some(ledger),
            entries: 0,
        };
    }
    if entries.0.len() == applied.entries {
        return;
    }
    let by_ordinal: HashMap<u16, Entity> = trees
        .iter()
        .map(|(entity, identity, _, _)| (identity.ordinal, entity))
        .collect();
    for &entry in &entries.0[applied.entries..] {
        let Some(&tree) = by_ordinal.get(&entry.index()) else {
            error!("destruction ledger: entry {entry:?} names no tree of this map");
            continue;
        };
        let (_, _, body, mut state) = trees
            .get_mut(tree)
            .expect("by_ordinal enumerates this same tree query in this system call");
        let LedgerEntry::State { word, .. } = entry;
        match TreeState::decode(word, body.kind.segments.len()) {
            Ok(decoded) => {
                *state = decoded;
            }
            Err(reason) => error!("destruction ledger: entry {entry:?} refused: {reason}"),
        }
    }
    applied.entries = entries.0.len();
}

fn publish_presentation_clock(
    cursor: Option<Res<InterpolationTimeline>>,
    local: Res<LocalTimeline>,
    mut clock: ResMut<PresentationClock>,
) {
    let (tick, overstep) = match cursor.filter(|timeline| timeline.is_synced()) {
        Some(timeline) => (timeline.tick(), f64::from(timeline.overstep().to_f32())),
        None => (local.tick(), 0.0),
    };
    clock.tick = tick.0;
    clock.overstep = overstep;
}

/// The static collision copy of a streamed trunk; its carrier owns its lifetime.
#[derive(Component)]
struct ReplicaTrunk {
    carrier: Entity,
    tree: Entity,
    cut: u8,
}
#[derive(Component)]
struct TrunkAttached;

/// A peer carrier is refused visibly at its map-data boundary, before collision construction.
#[derive(Debug, PartialEq, Eq)]
enum TrunkCarrierError {
    UnknownTree(u16),
    CutOutside { cut: u8, segments: usize },
    TopologyMismatch { cut: u8, recorded: u8 },
}
impl std::fmt::Display for TrunkCarrierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTree(tree) => write!(f, "map tree {tree} is absent"),
            Self::CutOutside { cut, segments } => {
                write!(f, "cut {cut} is outside the tree's {segments} segments")
            }
            Self::TopologyMismatch { cut, recorded } => {
                write!(f, "carrier cut {cut} differs from recorded cut {recorded}")
            }
        }
    }
}
impl std::error::Error for TrunkCarrierError {}

fn resolve_carrier(
    piece: TreePiece,
    tree: Option<(&TreeBody, TreeState)>,
) -> std::result::Result<Option<&TreeBody>, TrunkCarrierError> {
    let (body, state) = tree.ok_or(TrunkCarrierError::UnknownTree(piece.tree))?;
    if usize::from(piece.cut) >= body.kind.segments.len() {
        return Err(TrunkCarrierError::CutOutside {
            cut: piece.cut,
            segments: body.kind.segments.len(),
        });
    }
    match state {
        TreeState::Standing => Ok(None), // Carrier arrival may precede the topology diff.
        TreeState::Down { cut, .. } if cut == piece.cut => Ok(Some(body)),
        TreeState::Down { cut, .. } => Err(TrunkCarrierError::TopologyMismatch {
            cut: piece.cut,
            recorded: cut,
        }),
    }
}

fn attach_trunks(
    carriers: Query<(Entity, &TreePiece, &DebrisPose, &WorldGeneration), Without<TrunkAttached>>,
    generation: Res<CurrentWorld>,
    trees: Query<(Entity, &MapInstance, &TreeBody, &TreeState)>,
    mut commands: Commands,
) {
    for (carrier, piece, pose, belongs) in &carriers {
        if Some(*belongs) != generation.0 {
            continue; // Another world's carrier can arrive before its phase or after its retirement.
        }
        let tree = trees
            .iter()
            .find(|(_, identity, _, _)| identity.ordinal == piece.tree);
        let body = match resolve_carrier(*piece, tree.map(|(_, _, body, state)| (body, *state))) {
            Ok(Some(body)) => body,
            Ok(None) => continue, // The topology diff has not arrived yet.
            Err(error) => {
                error!("tree carrier {carrier:?} refused: {error}");
                commands.entity(carrier).insert(TrunkAttached);
                continue;
            }
        };
        let tree = tree
            .expect("carrier resolution succeeded for this map tree")
            .0;
        let centroid = body.piece_centroid(piece.cut);
        let root = commands
            .spawn((
                ReplicaTrunk {
                    carrier,
                    tree,
                    cut: piece.cut,
                },
                LooseTrunk,
                pose.transform(),
                Position(pose.position),
                Rotation(pose.rotation),
                RigidBody::Static,
            ))
            .id();
        for k in usize::from(piece.cut)..body.kind.segments.len() {
            commands.spawn((
                ChildOf(root),
                Transform::from_translation(Vec3::Y * body.prefix[k] - centroid),
                crate::trees::replica_segment_collider(body, k),
                TreeSegment(k as u8),
                crate::buildings::breakage::piece_layers(),
                body.kind.friction,
                crate::ballistics::BallisticVolume {
                    material: body.kind.material,
                    substance: body.kind.substance.clone(),
                },
            ));
        }
        commands.entity(carrier).insert(TrunkAttached);
    }
}

/// Lightyear inserts an interpolated DebrisPose only when the cursor reaches its first sample
/// (`lightyear_interpolation::interpolate`, `update_confirmed_history`). Until then the topology
/// can already be known, but the standing colliders must remain present. The copy takes over in
/// the same frame its pose becomes present, including frames with no physics tick.
fn follow_trunks(
    carriers: Query<(&DebrisPose, &WorldGeneration)>,
    generation: Res<CurrentWorld>,
    trees: Query<(&TreeState, &crate::trees::TreeParts)>,
    mut trunks: Query<(
        Entity,
        &ReplicaTrunk,
        &Children,
        &mut Transform,
        &mut Position,
        &mut Rotation,
    )>,
    mut commands: Commands,
) {
    for (entity, trunk, children, mut transform, mut position, mut rotation) in &mut trunks {
        let Ok((pose, belongs)) = carriers.get(trunk.carrier) else {
            commands.entity(entity).despawn(); // Disconnect removed the authority's carrier.
            continue;
        };
        if Some(*belongs) != generation.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let Ok((state, parts)) = trees.get(trunk.tree) else {
            commands.entity(entity).despawn(); // The local map was unloaded with its tree roots.
            continue;
        };
        let active = matches!(*state, TreeState::Down { cut, .. } if cut == trunk.cut);
        if active {
            for &part in parts.0.iter().skip(usize::from(trunk.cut)) {
                commands
                    .entity(part)
                    .insert_if_new(avian3d::prelude::ColliderDisabled);
            }
        }
        for &child in children {
            if active {
                commands
                    .entity(child)
                    .remove::<avian3d::prelude::ColliderDisabled>();
            } else {
                commands
                    .entity(child)
                    .insert_if_new(avian3d::prelude::ColliderDisabled);
            }
        }
        transform.set_if_neq(pose.transform());
        position.set_if_neq(Position(pose.position));
        rotation.set_if_neq(Rotation(pose.rotation));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::prelude::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn replica() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            crate::contact_transfer::sim_plugin,
            crate::trees::sim_plugin,
            crate::debris::sim_plugin,
            install_apply,
        ))
        .init_asset::<Mesh>()
        .init_resource::<crate::ClientReplica>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, app.world());
        let root = crate::trees::spawn(
            &mut commands,
            &crate::trees::tests::fixture_kind(),
            MapInstance {
                id: "fixture".into(),
                ordinal: 3,
            },
            Transform::from_xyz(2.0, 0.0, 1.0),
        );
        queue.apply(app.world_mut());
        (app, root)
    }

    /// The authority has one startup ledger. Losing it is a composition bug, never a silent
    /// missed topology publication that leaves clients' trees standing.
    #[test]
    #[should_panic(expected = "authority startup spawns exactly one destruction ledger")]
    fn publication_requires_the_authoritys_ledger() {
        let mut app = App::new();
        app.add_systems(Update, publish_falls);
        app.update();
    }

    /// Before a snapshot there is no record to apply. A second authority record is a bug; a
    /// single-query skip would otherwise hide the mixed-authority state from replica users.
    #[test]
    #[should_panic(expected = "a replica has one active authority ledger")]
    fn two_authority_ledgers_are_a_bug() {
        let mut app = App::new();
        app.init_resource::<CurrentWorld>()
            .init_resource::<Applied>()
            .add_systems(Update, apply_ledger);
        app.update();
        app.world_mut()
            .spawn((DestructionLedger::default(), WorldGeneration::default()));
        app.world_mut()
            .spawn((DestructionLedger::default(), WorldGeneration::default()));
        app.update();
    }

    /// Topology diffs keep their exact word, malformed peer words are refused, and reconnect
    /// starts from the replacement authority. World geometry relies on no old cut surviving.
    #[test]
    fn topology_applies_once_refuses_invalid_words_and_resets_on_reconnect() {
        let (mut app, root) = replica();
        app.update();
        assert_eq!(
            app.world().get::<TreeState>(root),
            Some(&TreeState::Standing)
        );
        let state = TreeState::Down { cut: 1, dir: 64 };
        let ledger = app
            .world_mut()
            .spawn((
                DestructionLedger(vec![LedgerEntry::State {
                    index: 3,
                    word: state.encode(),
                }]),
                WorldGeneration::default(),
            ))
            .id();
        app.update();
        assert_eq!(app.world().get::<TreeState>(root), Some(&state));
        app.world_mut()
            .get_mut::<DestructionLedger>(ledger)
            .unwrap()
            .apply_diff(&LedgerEntry::State {
                index: 3,
                word: 0xffff,
            })
            .unwrap();
        app.update();
        assert_eq!(app.world().get::<TreeState>(root), Some(&state));
        assert_eq!(app.world().resource::<Applied>().entries, 2);
        app.world_mut().entity_mut(ledger).despawn();
        app.world_mut()
            .spawn((DestructionLedger::default(), WorldGeneration::default()));
        app.update();
        assert_eq!(
            app.world().get::<TreeState>(root),
            Some(&TreeState::Standing)
        );
        let parts = &app.world().get::<crate::trees::TreeParts>(root).unwrap().0;
        assert!(
            parts
                .iter()
                .all(|&part| app.world().get::<ColliderDisabled>(part).is_none())
        );
    }

    /// Malformed carrier identity is external input, refused before creating collision state.
    /// Replica construction relies on each cause remaining distinct from a legitimate wait.
    #[test]
    fn carrier_identity_refusals_are_typed_and_a_pre_topology_carrier_waits() {
        let body = TreeBody::new(crate::trees::tests::fixture_kind(), 1.0);
        let piece = TreePiece { tree: 3, cut: 1 };
        assert_eq!(
            resolve_carrier(piece, None).unwrap_err(),
            TrunkCarrierError::UnknownTree(3)
        );
        assert!(
            resolve_carrier(piece, Some((&body, TreeState::Standing)))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            resolve_carrier(
                TreePiece { cut: 3, ..piece },
                Some((&body, TreeState::Standing))
            )
            .unwrap_err(),
            TrunkCarrierError::CutOutside {
                cut: 3,
                segments: 3
            }
        );
        assert_eq!(
            resolve_carrier(piece, Some((&body, TreeState::Down { cut: 2, dir: 0 }))).unwrap_err(),
            TrunkCarrierError::TopologyMismatch {
                cut: 1,
                recorded: 2
            }
        );
    }

    /// Topology can arrive before the cursor reaches a trunk's birth. Client casts must still
    /// meet the standing wood then, and switch once, in the frame the first pose is inserted.
    /// A replica that disables on ledger arrival creates an invisible collision gap.
    #[test]
    fn standing_colliders_remain_until_the_first_pose_is_present() {
        let (mut app, root) = replica();
        app.update();
        let marker = TreePiece { tree: 3, cut: 1 };
        let carrier = app
            .world_mut()
            .spawn((marker, WorldGeneration::default()))
            .id();
        app.world_mut().spawn((
            DestructionLedger(vec![LedgerEntry::State {
                index: 3,
                word: TreeState::Down { cut: 1, dir: 0 }.encode(),
            }]),
            WorldGeneration::default(),
        ));
        app.update();
        let parts = app
            .world()
            .get::<crate::trees::TreeParts>(root)
            .unwrap()
            .0
            .clone();
        assert!(
            parts
                .iter()
                .all(|&part| app.world().get::<ColliderDisabled>(part).is_none())
        );
        assert_eq!(
            app.world_mut()
                .query::<&ReplicaTrunk>()
                .iter(app.world())
                .count(),
            0
        );
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        app.world_mut().entity_mut(carrier).insert(DebrisPose {
            position: Vec3::new(8.0, 2.0, -1.0),
            rotation: Quat::from_rotation_z(-0.8),
        });
        app.update();
        assert!(app.world().get::<ColliderDisabled>(parts[0]).is_none());
        assert!(
            parts
                .iter()
                .skip(1)
                .all(|&part| app.world().get::<ColliderDisabled>(part).is_some())
        );
        assert_eq!(
            app.world_mut()
                .query::<&ReplicaTrunk>()
                .iter(app.world())
                .count(),
            1
        );
    }

    /// The replicated pose is the sole motion source, including on a render frame with no fixed
    /// tick and a late join straight into a moving trunk. Client casts rely on child poses being
    /// placed in that frame. No client body becomes dynamic, and losing the carrier removes its
    /// collision copy. ASSUMED fixture poses and cut, independent of a trajectory or asset name.
    #[test]
    fn a_late_replica_follows_only_the_pose_stream_even_without_a_physics_tick() {
        let (mut app, root) = replica();
        app.update();
        let first = DebrisPose {
            position: Vec3::new(8.0, 3.0, -2.0),
            rotation: Quat::from_rotation_z(-1.1),
        };
        let carrier = app
            .world_mut()
            .spawn((
                TreePiece { tree: 3, cut: 1 },
                first,
                WorldGeneration::default(),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&ReplicaTrunk>()
                .iter(app.world())
                .count(),
            0,
            "carrier waits for topology"
        );
        app.world_mut().spawn((
            DestructionLedger(vec![LedgerEntry::State {
                index: 3,
                word: TreeState::Down { cut: 1, dir: 0 }.encode(),
            }]),
            WorldGeneration::default(),
        ));
        app.update();
        let ghost = app
            .world_mut()
            .query_filtered::<Entity, With<ReplicaTrunk>>()
            .single(app.world())
            .unwrap();
        assert_eq!(
            app.world().get::<Position>(ghost),
            Some(&Position(first.position))
        );
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        let next = DebrisPose {
            position: Vec3::new(-4.0, 2.0, 5.0),
            rotation: Quat::from_rotation_x(0.7),
        };
        app.world_mut().entity_mut(carrier).insert(next);
        app.update();
        assert_eq!(
            app.world().get::<Position>(ghost),
            Some(&Position(next.position))
        );
        assert_eq!(
            app.world().get::<Rotation>(ghost),
            Some(&Rotation(next.rotation))
        );
        let centroid = app.world().get::<TreeBody>(root).unwrap().piece_centroid(1);
        let prefix = app.world().get::<TreeBody>(root).unwrap().prefix.clone();
        let children = app.world().get::<Children>(ghost).unwrap();
        for &child in children {
            let k = usize::from(app.world().get::<TreeSegment>(child).unwrap().0);
            let expected = next.position + next.rotation * (Vec3::Y * prefix[k] - centroid);
            assert!(
                app.world()
                    .get::<Position>(child)
                    .unwrap()
                    .0
                    .abs_diff_eq(expected, 1e-5)
            );
        }
        assert!(
            app.world_mut()
                .query::<&RigidBody>()
                .iter(app.world())
                .all(RigidBody::is_static)
        );
        let parts = &app.world().get::<crate::trees::TreeParts>(root).unwrap().0;
        assert!(
            parts
                .iter()
                .skip(1)
                .all(|&part| app.world().get::<ColliderDisabled>(part).is_some())
        );
        app.world_mut().entity_mut(carrier).despawn();
        app.update();
        assert!(
            app.world().get_entity(ghost).is_err(),
            "disconnect removes the static stream copy"
        );
    }
}
