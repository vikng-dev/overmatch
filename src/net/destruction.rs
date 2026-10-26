//! The destruction ledger: one globally visible entity whose component lists every tree state
//! transition the authority has made, replicated as diffs — a fall costs one entry on the wire, a
//! late joiner receives the list once, and replicon's per-tick change scan touches one entity
//! instead of one per tree.
//!
//! An entry is `(index, word, tick)`: the tree's `MapInstance.ordinal` (id-sorted, dense, bound
//! into the map digest the handshake compares, so both peers number the same trees the same way
//! or never match), `TreeState::encode`'s word, and the authority tick of the transition, which
//! the fall presentation fuses to the interpolation cursor (ADR-0037). The replica decodes the
//! word against its own copy of the tree and refuses one that names no state of it; everything
//! else about a fallen tree — colliders, pool, pose, view — it derives (ADR-0016).

use std::collections::HashMap;

use bevy::ecs::error::Result;
use bevy::prelude::*;
use bevy_replicon::prelude::{Diffable, EntityCommandsDiffExt};
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::scatter::MapInstance;
use crate::trees::{FallClock, Fell, TreeBody, TreeFallSet, TreeState, derive_systems};

/// One transition: which tree, what state, which authority tick.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LedgerEntry {
    pub(crate) index: u16,
    pub(crate) word: u16,
    pub(crate) tick: u32,
}

/// Every transition so far, in authority order. Appended only through replicon's diff path, so
/// the wire carries the appended entry and a fresh replica the whole list.
#[derive(Component, Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DestructionLedger(pub(crate) Vec<LedgerEntry>);

impl Diffable for DestructionLedger {
    type Diff = LedgerEntry;

    fn apply_diff(&mut self, diff: &LedgerEntry) -> Result<()> {
        self.0.push(*diff);
        Ok(())
    }
}

/// What this replica has applied to its trees: which ledger entity, and how many of its entries.
/// A ledger is one replicated entity per link — lightyear despawns every replicated entity when
/// a link ends (`lightyear_replication::client::on_replication_disconnect`) and the next server's
/// arrives as a new one — so another entity is another authority's record: every tree stands
/// again and the record is consumed from its first entry.
#[derive(Resource, Default)]
struct Applied {
    ledger: Option<Entity>,
    entries: usize,
}

pub(super) fn install_server(app: &mut App) {
    app.add_systems(Startup, spawn_ledger)
        .add_systems(FixedPostUpdate, publish_falls.after(TreeFallSet));
}

pub(super) fn install_client(app: &mut App) {
    install_apply(app);
    app.init_resource::<FallClock>()
        .add_systems(Update, publish_fall_clock);
}

/// The ledger's consumption and the body derive it orders, in `Update`: replicon receives in
/// `PreUpdate` and the fixed loop runs before `Update`, so a replica deriving only in the fixed
/// loop would leave a received fall's colliders standing until the next fixed tick — a frame
/// that runs none leaves them standing a frame longer. The derive places the colliders in the
/// physics world in this frame ([`derive_systems`]), so the march of the next fixed tick reads
/// the received state; the presentation reads it this frame.
pub(crate) fn install_apply(app: &mut App) {
    app.init_resource::<Applied>()
        .add_systems(Update, (apply_ledger, derive_systems()).chain());
}

fn spawn_ledger(mut commands: Commands) {
    commands.spawn((
        Name::new("DestructionLedger"),
        DestructionLedger::default(),
        Replicate::to_clients(NetworkTarget::All),
    ));
}

/// Authority: every tree whose state left `Standing` this tick becomes an entry.
fn publish_falls(
    fallen: Query<(&MapInstance, &TreeState, &Fell), Changed<TreeState>>,
    ledgers: Query<Entity, With<DestructionLedger>>,
    mut commands: Commands,
) {
    let Ok(ledger) = ledgers.single() else {
        return;
    };
    for (identity, state, fell) in &fallen {
        if *state == TreeState::Standing {
            continue;
        }
        commands
            .entity(ledger)
            .apply_diff::<DestructionLedger>(LedgerEntry {
                index: identity.ordinal,
                word: state.encode(),
                tick: fell.tick,
            });
    }
}

/// Replica: apply every entry not yet applied to the tree its index names. A word no state of
/// that tree encodes to is refused and logged, never clamped into a pose the authority did not
/// set; an index no tree carries is a map mismatch the handshake should have refused. A ledger
/// other than the one applied so far (another link's) first stands every tree back up — the
/// derive then drops its pose and transition tick — so a tree the new record never names stands
/// and one it names falls as it says; the record is then consumed from its first entry.
fn apply_ledger(
    ledgers: Query<(Entity, &DestructionLedger)>,
    mut applied: ResMut<Applied>,
    mut trees: Query<(Entity, &MapInstance, &TreeBody, &mut TreeState)>,
    mut commands: Commands,
) {
    let Ok((ledger, entries)) = ledgers.single() else {
        return;
    };
    if applied.ledger != Some(ledger) || entries.0.len() < applied.entries {
        if applied.ledger == Some(ledger) {
            error!(
                "destruction ledger: {} entries after {} applied; the record shrank",
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
    for entry in &entries.0[applied.entries..] {
        let Some(&tree) = by_ordinal.get(&entry.index) else {
            error!("destruction ledger: entry {entry:?} names no tree of this map");
            continue;
        };
        let Ok((_, _, body, mut state)) = trees.get_mut(tree) else {
            continue;
        };
        match TreeState::decode(entry.word, body.kind.segments.len()) {
            Ok(decoded) => {
                *state = decoded;
                commands.entity(tree).insert(Fell { tick: entry.tick });
            }
            Err(reason) => error!("destruction ledger: entry {entry:?} refused: {reason}"),
        }
    }
    applied.entries = entries.0.len();
}

/// Replica: the presentation clock is the interpolation cursor once it has synced — the time the
/// hulls draw at — and the local tick before that (announcements present at arrival, as the fire
/// queue's no-cursor fallback does).
fn publish_fall_clock(
    cursor: Option<Res<InterpolationTimeline>>,
    local: Res<LocalTimeline>,
    mut clock: ResMut<FallClock>,
) {
    let (tick, overstep) = match cursor.filter(|timeline| timeline.is_synced()) {
        Some(timeline) => (timeline.tick(), f64::from(timeline.overstep().to_f32())),
        None => (local.tick(), 0.0),
    };
    clock.tick = tick.0;
    clock.overstep = overstep;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::{Position, Rotation};
    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::ballistics::{ComponentHealth, MarchProbe, PoolEmptied};
    use crate::trees::Resting;

    /// What stands in for replicon's receive: entries landing on the ledger in `PreUpdate`.
    #[derive(Resource, Default)]
    struct Incoming(Vec<LedgerEntry>);

    fn receive(mut incoming: ResMut<Incoming>, mut ledgers: Query<&mut DestructionLedger>) {
        let Ok(mut ledger) = ledgers.single_mut() else {
            return;
        };
        for entry in incoming.0.drain(..) {
            ledger.apply_diff(&entry).expect("an append");
        }
    }

    /// The trees' sim half on the network's physics — avian as `net::physics` mounts it, and
    /// lightyear's after-loop child collider update — with the fixture grid and `trees` fixture
    /// trees on it, one per ordinal, each yawed 0.7 rad (fixture data: the placement composes a
    /// turned root), and the march probe in `FixedUpdate`. A replica adds the ledger's apply and
    /// its marker; the authority is the composition without them.
    fn composed(replica: bool, trees: u16) -> (App, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            super::super::physics::physics_plugins(),
            super::super::physics::plugin,
            crate::trees::sim_plugin,
            crate::ballistics::mount_march_probe,
        ))
        .init_asset::<Mesh>()
        .insert_resource(crate::trees::tests::fixture_grid(|_, _| 0.0))
        .add_systems(
            RunFixedMainLoop,
            lightyear::avian3d::plugin::LightyearAvianPlugin::update_child_collider_position
                .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
        );
        if replica {
            app.add_plugins(install_apply)
                .init_resource::<crate::ClientReplica>()
                .init_resource::<Incoming>()
                .add_systems(PreUpdate, receive);
        }
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        let kind = crate::trees::tests::fixture_kind();
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        for ordinal in 0..trees {
            crate::trees::spawn(
                &mut commands,
                &kind,
                MapInstance {
                    id: format!("fir_{ordinal}"),
                    ordinal,
                },
                Transform::from_xyz(0.0, 0.0, 20.0 * f32::from(ordinal))
                    .with_rotation(Quat::from_rotation_y(0.7)),
                None,
            );
        }
        queue.apply(world);
        let mut roots: Vec<(u16, Entity)> = world
            .query::<(Entity, &MapInstance)>()
            .iter(world)
            .map(|(entity, identity)| (identity.ordinal, entity))
            .collect();
        roots.sort_unstable();
        (app, roots.into_iter().map(|(_, entity)| entity).collect())
    }

    fn replica(trees: u16) -> (App, Vec<Entity>) {
        composed(true, trees)
    }

    /// Each frame advances virtual time by `micros`: 15 625 is one fixed tick at 64 Hz, 0 none.
    fn frame_time(app: &mut App, micros: u64) {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            micros,
        )));
    }

    /// Each segment's collider pose in avian's world, in segment order, as bit patterns.
    fn placements(app: &App, root: Entity) -> Vec<[u32; 7]> {
        let world = app.world();
        world
            .get::<Children>(root)
            .expect("segments")
            .iter()
            .filter_map(|child| {
                let position = world.get::<Position>(child)?.0;
                let rotation = world.get::<Rotation>(child)?.0;
                Some(
                    [
                        position.x, position.y, position.z, rotation.x, rotation.y, rotation.z,
                        rotation.w,
                    ]
                    .map(f32::to_bits),
                )
            })
            .collect()
    }

    /// Two probe rays against the fixture trunk at the origin, felled `Down { cut: 1, dir: 0 }`
    /// on flat ground (fixture data; DERIVED: the 4 m piece above the cut reaches no ground by
    /// 100°, so it lies at 90°, along +x at 2 m). The first crosses where segment 1 stood, 3 m up,
    /// clear of the lying piece; the second comes down at x = 3 m onto segment 2 where it lies,
    /// clear of the standing trunk. Each is the disc of an 88 mm round (fixture data), so the
    /// probe takes the march's broad-phase test as well as its casts.
    fn aim_probe(app: &mut App) {
        let mut probe = app.world_mut().resource_mut::<MarchProbe>();
        probe.radius = 0.044;
        probe.rays = vec![
            (Vec3::new(0.0, 3.0, -5.0), Dir3::Z, 10.0),
            (Vec3::new(3.0, 10.0, 0.0), Dir3::NEG_Y, 20.0),
        ];
    }

    /// A RECEIVED FALL REACHES THE BODY IN THE FRAME IT LANDS AND THE MARCH AT THE NEXT FIXED
    /// TICK, WHETHER THAT FRAME RUNS A TICK OR NOT: an entry arriving in `PreUpdate` of a frame
    /// that runs none leaves `Update` with the tree's state, its transition tick, its resting
    /// pose and its segments' lying transforms all set, and the march of the next fixed tick —
    /// the live first-contact query, through avian's collider trees — reads the lying piece. A
    /// replica deriving only in the fixed loop reds the first half; one that leaves the colliders
    /// to the next physics step reads the standing trunk in that march and reds the second. The
    /// step that follows rewrites the placed collider poses bit for bit: the placement computes
    /// what avian would, so the sim never sees two poses for one state.
    #[test]
    fn a_received_fall_reaches_the_next_ticks_march() {
        let (mut app, roots) = replica(1);
        aim_probe(&mut app);
        app.world_mut().spawn(DestructionLedger::default());
        frame_time(&mut app, 15_625);
        app.update();
        app.update();
        let settled = app.world().resource::<MarchProbe>().reached.clone();
        assert_eq!(
            settled.last(),
            Some(&vec![Some(roots[0]), None]),
            "the march reads the trunk standing"
        );
        // The receive frame: no time passes, so no fixed tick runs.
        frame_time(&mut app, 0);
        app.world_mut()
            .resource_mut::<Incoming>()
            .0
            .push(LedgerEntry {
                index: 0,
                word: TreeState::Down { cut: 1, dir: 0 }.encode(),
                tick: 9,
            });
        app.update();
        let world = app.world();
        assert_eq!(
            world.get::<TreeState>(roots[0]),
            Some(&TreeState::Down { cut: 1, dir: 0 })
        );
        assert_eq!(world.get::<Fell>(roots[0]), Some(&Fell { tick: 9 }));
        assert!(
            world.get::<Resting>(roots[0]).is_some(),
            "the pose, this frame"
        );
        let tip = world
            .get::<Children>(roots[0])
            .unwrap()
            .iter()
            .last()
            .unwrap();
        let local = world.get::<Transform>(tip).unwrap();
        assert!(
            local.translation.xz().length() > 1.9 && local.rotation != Quat::IDENTITY,
            "the tip's transform lies, this frame: {local:?}"
        );
        assert_eq!(
            world.resource::<MarchProbe>().reached.len(),
            settled.len(),
            "the receive frame ran no fixed tick"
        );
        let placed = placements(&app, roots[0]);
        frame_time(&mut app, 15_625);
        app.update();
        assert_eq!(
            app.world()
                .resource::<MarchProbe>()
                .reached
                .get(settled.len()),
            Some(&vec![None, Some(roots[0])]),
            "the next tick's march reads the piece lying"
        );
        assert_eq!(
            placements(&app, roots[0]),
            placed,
            "the physics step rewrites the placement's own bits"
        );
    }

    /// THE AUTHORITY'S NEXT MARCH READS A FALL: a pool emptied in tick N (a dose's entry 3 m up
    /// the fixture trunk, along +x) fells the tree in that tick; tick N's march, which reads the
    /// body before the derive at the end of the tick, finds the trunk standing, and the march of
    /// tick N + 1 — the live first-contact query, through avian's collider trees — reads the
    /// lying piece. A derive that leaves the colliders to the next physics step reads the
    /// standing trunk at N + 1 and reds this.
    #[test]
    fn the_authoritys_next_march_reads_a_fall() {
        #[derive(Resource)]
        struct Empty(Option<Entity>);

        fn empty(mut pending: ResMut<Empty>, mut commands: Commands) {
            if let Some(tree) = pending.0.take() {
                commands.trigger(PoolEmptied {
                    entity: tree,
                    at: Vec3::new(0.0, 3.0, 0.0),
                    along: Some(Vec3::X),
                });
            }
        }

        let (mut app, roots) = composed(false, 1);
        aim_probe(&mut app);
        app.insert_resource(Empty(None))
            .add_systems(FixedPreUpdate, empty);
        frame_time(&mut app, 15_625);
        app.update();
        app.update();
        let before = app.world().resource::<MarchProbe>().reached.len();
        app.world_mut().resource_mut::<Empty>().0 = Some(roots[0]);
        app.update();
        app.update();
        let world = app.world();
        assert_eq!(
            world.get::<TreeState>(roots[0]),
            Some(&TreeState::Down { cut: 1, dir: 0 })
        );
        let reached = &world.resource::<MarchProbe>().reached;
        assert_eq!(
            reached.get(before),
            Some(&vec![Some(roots[0]), None]),
            "tick N's march reads the trunk standing"
        );
        assert_eq!(
            reached.get(before + 1),
            Some(&vec![None, Some(roots[0])]),
            "tick N + 1's march reads the piece lying"
        );
    }

    /// ANOTHER LEDGER IS ANOTHER AUTHORITY'S RECORD (P2b): a link's ledger fells tree A; the next
    /// link's ledger — a new entity of the same length — names only tree B. Tree A stands again
    /// (its tick, its pose and its lying transforms gone, its pool back) and tree B falls; a
    /// replica that remembers only a length skips B and leaves A down.
    #[test]
    fn a_replaced_ledger_stands_every_tree_it_does_not_name() {
        let (mut app, roots) = replica(2);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        let first = app
            .world_mut()
            .spawn(DestructionLedger(vec![LedgerEntry {
                index: 0,
                word: TreeState::Down { cut: 0, dir: 64 }.encode(),
                tick: 3,
            }]))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<TreeState>(roots[0]),
            Some(&TreeState::Down { cut: 0, dir: 64 }),
            "the first link's ledger fells A"
        );
        let spawned = |app: &App, root: Entity| -> Vec<Transform> {
            let world = app.world();
            world
                .get::<Children>(root)
                .unwrap()
                .iter()
                .filter_map(|child| world.get::<Transform>(child).copied())
                .collect()
        };
        let a_down = spawned(&app, roots[0]);
        let b_standing = spawned(&app, roots[1]);
        // The link ends: lightyear despawns the replicated ledger; the next server's arrives.
        app.world_mut().entity_mut(first).despawn();
        app.world_mut().spawn(DestructionLedger(vec![LedgerEntry {
            index: 1,
            word: TreeState::Down { cut: 1, dir: 0 }.encode(),
            tick: 5,
        }]));
        app.update();
        let world = app.world();
        assert_eq!(
            world.get::<TreeState>(roots[0]),
            Some(&TreeState::Standing),
            "A stands: the new record never names it"
        );
        assert!(
            world.get::<Fell>(roots[0]).is_none() && world.get::<Resting>(roots[0]).is_none(),
            "A's tick and pose are gone"
        );
        assert!(
            world.get::<ComponentHealth>(roots[0]).is_some(),
            "A's pool is back"
        );
        assert_eq!(
            spawned(&app, roots[0]),
            b_standing,
            "A's segments stand as spawned"
        );
        assert_eq!(
            world.get::<TreeState>(roots[1]),
            Some(&TreeState::Down { cut: 1, dir: 0 }),
            "B falls as the new record says"
        );
        assert_eq!(world.get::<Fell>(roots[1]), Some(&Fell { tick: 5 }));
        assert_ne!(spawned(&app, roots[1]), b_standing, "B's segments lie");
        assert_ne!(spawned(&app, roots[0]), a_down);
    }
}
