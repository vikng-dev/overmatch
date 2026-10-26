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
use crate::trees::{FallClock, Fell, TreeBody, TreeFallSet, TreeState, derive_state};

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
/// that runs none leaves them standing a frame longer. Avian consumes the moved transforms at
/// the next fixed tick either way; the presentation reads them this frame.
pub(crate) fn install_apply(app: &mut App) {
    app.init_resource::<Applied>()
        .add_systems(Update, (apply_ledger, derive_state).chain());
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
/// other than the one applied so far (another link's) first stands every tree back up, the
/// transition tick with it, so a tree the new record never names stands and one it names falls
/// as it says; the record is then consumed from its first entry.
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
        for (tree, _, _, mut state) in &mut trees {
            if state.set_if_neq(TreeState::Standing) {
                commands.entity(tree).remove::<Fell>();
            }
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

    use avian3d::prelude::{ColliderAabb, PhysicsPlugins};
    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::ballistics::ComponentHealth;
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

    /// A replica's composition: the real schedules, avian, the trees' sim half and the ledger's
    /// apply, with the fixture grid and `trees` fixture trees on it, one per ordinal.
    fn replica(trees: u16) -> (App, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            crate::trees::sim_plugin,
            install_apply,
        ))
        .init_asset::<Mesh>()
        .init_resource::<crate::ClientReplica>()
        .init_resource::<Incoming>()
        .insert_resource(crate::trees::tests::fixture_grid(|_, _| 0.0))
        .add_systems(PreUpdate, receive);
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
                Transform::from_xyz(0.0, 0.0, 20.0 * f32::from(ordinal)),
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

    /// The tip segment's bounds as avian holds them.
    fn tip_bounds(app: &App, root: Entity) -> ColliderAabb {
        let world = app.world();
        let tip = world
            .get::<Children>(root)
            .expect("segments")
            .iter()
            .filter(|child| world.get::<ColliderAabb>(*child).is_some())
            .last()
            .expect("the tip");
        *world.get::<ColliderAabb>(tip).unwrap()
    }

    /// A RECEIVED FALL REACHES THE BODY IN THE FRAME IT LANDS, FIXED TICK OR NONE (P2c): an
    /// entry arriving in `PreUpdate` of a frame that runs no fixed tick leaves `Update` with the
    /// tree's state, its transition tick, its resting pose and its segments' lying transforms
    /// all set; avian's bounds still stand, and the next fixed tick lays them down. A replica
    /// deriving only in the fixed loop reds the first half.
    #[test]
    fn a_received_fall_derives_in_its_own_frame_and_avian_follows_at_the_next_tick() {
        let (mut app, roots) = replica(1);
        app.world_mut().spawn(DestructionLedger::default());
        // Settle: avian takes its first look at the standing tree.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        app.update();
        app.update();
        let standing = tip_bounds(&app, roots[0]);
        // The receive frame: no time passes, so no fixed tick runs.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
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
            local.translation.x > 1.9 && local.rotation != Quat::IDENTITY,
            "the tip's transform lies, this frame: {local:?}"
        );
        assert_eq!(
            tip_bounds(&app, roots[0]),
            standing,
            "avian has not run: its bounds still stand"
        );
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        app.update();
        let down = tip_bounds(&app, roots[0]);
        assert!(
            down.max.y < standing.max.y - 1.5 && down.max.x > standing.max.x + 1.5,
            "one fixed tick later avian's bounds lie along +x: {down:?} from {standing:?}"
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
