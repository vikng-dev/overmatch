//! Server-simulated pieces at rest, in the game's environment destruction (game state only): the
//! one rule every loose piece follows: a construction's fallen chunks ([`crate::construction::breakage`]),
//! and the debris stress lever's fixture fragments (`net::debris_stress`). The authority
//! alone simulates a piece; a replica draws it at the pose the network carries. A piece is a body
//! with [`DebrisPiece`]; its colliders are the body's own ([`RigidBodyColliders`]). Paths below
//! are under `vendor/avian3d-0.7.0-child-pose/src/`.
//!
//! REST IS AVIAN'S ISLAND SLEEP. A piece is a dynamic body all match; nothing here changes its
//! body type. Avian 0.7 links dynamic bodies through touching contacts into islands and puts an
//! island to sleep once every member has passed its rest test for `TimeToSleep`: either both
//! velocity thresholds pass or kinetic energy per mass is below the linear rest-speed energy
//! (see the vendor's `OVERMATCH_PATCH.md`). A sleeping body
//! leaves the solver and keeps its mass, so what touches it next meets finite mass. Only its
//! touching contact pairs move to the sleeping set the narrow phase skips
//! (`collision/contact_types/contact_graph.rs:797`); a pair that overlaps in bounds without
//! touching stays active and is recomputed every tick (`collision/narrow_phase/system_param.rs:480`).
//! An awake body that starts touching a sleeping one wakes its whole island
//! (`collision/narrow_phase/system_param.rs:247-261`), as does a body or a body's collider of that
//! island going (`collision/narrow_phase/mod.rs:484,528`). An island sleeps whole: a piece touching
//! an awake body that is no piece (a driven tank) shares its island and stays awake while it
//! touches, with every piece that touches it in turn. A tank with any spring contact never sleeps
//! (DERIVED): a non-zero force call resets the sleep timer
//! (`dynamics/rigid_body/forces/query_data.rs:345,687`) and the track code applies its sampled
//! forces every tick (`track::sim::apply_track_forces`). A tank wholly on loose pieces has no
//! sampled force, and with no drive demand it sleeps with its pile; drive demand ends that sleep
//! (`track::sim::wake_driven_tanks`).
//!
//! THE WAKES AVIAN LACKS. Avian wakes no body when a support with no dynamic body of its own goes,
//! is disabled or moves (the TODO in `collision/narrow_phase/mod.rs`, `remove_collider`), so a
//! construction chunk turned to dust under a sleeping piece would leave it hanging. The rule wakes
//! every sleeping body, a piece or any other (a launched turret), that touches a support as it:
//! * goes ([`wake_on_collider_gone`]: `Discard` runs before avian's `Remove` drops the pairs);
//! * goes with its body ([`wake_on_body_gone`]: a static root despawned with child colliders,
//!   whose pairs avian's `Remove<RigidBody>` drops before the children's queued despawn);
//! * is disabled ([`wake_on_collider_disabled`]): avian drops the pairs in an `Add` observer that
//!   Bevy may run before this one, so the rule reads bounds instead: every sleeping body with a
//!   collider whose bounds come within avian's contact tolerance of the support's;
//! * moves ([`wake_on_static_moved`]: a static body's collider re-posed, by its own [`Position`]
//!   or [`Rotation`] or by its body's), read inside the step before
//!   the broad phase, while the last step's pairs still stand. It relies on the vendored avian and
//!   lightyear child-pose writers (`vendor/avian3d-0.7.0-child-pose`,
//!   `vendor/lightyear_avian3d-0.30.1-child-pose`) writing a child collider's pose only when its
//!   bits change: a writer that rewrote it every step would wake every sleeper touching a
//!   construction every step.
//!
//! A piece remains simulated until it crosses the admitted play-area floor. The authority retires
//! it there, logging its speed; construction chunks publish dust and a retirement tick for the
//! shared presentation cursor. A tank crossing the floor is a broken containment invariant.
//!
//! HOW MANY CONSTRUCTION PIECES (DERIVED from `construction::breakage`): a chunk leaves its construction at most once
//! and every split partitions its owned chunks, so live construction bodies never exceed the map's
//! breakable chunks. The construction design owns the retained carriers' bound and the split model.
//! Awake pieces are the ones in the solver. A sleeping piece's own per-tick cost and an awake pile
//! held by a tank's island (above) are both unpriced.

pub(crate) mod crumble;
pub(crate) mod decor;
pub(crate) mod launch;

use avian3d::prelude::*;
use bevy::prelude::*;

/// Shared environment presentation cursor. A replica waits until its interpolation timeline
/// is synchronized; an authority or a tank-less view fixture owns an explicit tick cursor.
#[derive(Resource, Debug, Clone, Copy)]
pub(crate) enum PresentationClock {
    Waiting,
    At { tick: u32, overstep: f64 },
}
impl Default for PresentationClock {
    fn default() -> Self {
        // Offline view fixtures start at the explicit zero cursor; the replica adapter installs
        // Waiting before its first update, and only a synchronized server cursor can replace it.
        Self::At {
            tick: 0,
            overstep: 0.0,
        }
    }
}
impl PresentationClock {
    pub(crate) fn since(self, since: u32) -> Option<f64> {
        match self {
            Self::Waiting => None,
            Self::At { tick, overstep } => {
                Some(f64::from(tick.wrapping_sub(since) as i32) + overstep)
            }
        }
    }
}

/// A piece body under the rule. A piece spawns dynamic with it.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct DebrisPiece;

/// Wake every sleeping body in a touching pair with `collider` in avian's contact graph: removing
/// `Sleeping` wakes its island (avian's `wake_on_remove_sleeping` hook).
fn wake_touching(
    collider: Entity,
    contacts: &ContactGraph,
    sleeping: &Query<(), With<Sleeping>>,
    commands: &mut Commands,
) {
    for edge in contacts.contact_edges_with(collider) {
        if !edge.is_touching() {
            continue;
        }
        for body in [edge.body1, edge.body2].into_iter().flatten() {
            if sleeping.contains(body) {
                commands.entity(body).try_remove::<Sleeping>();
            }
        }
    }
}

/// A collider about to go: every sleeping body it touches wakes (module doc).
fn wake_on_collider_gone(
    gone: On<Discard, ColliderMarker>,
    contacts: Res<ContactGraph>,
    sleeping: Query<(), With<Sleeping>>,
    mut commands: Commands,
) {
    wake_touching(gone.entity, &contacts, &sleeping, &mut commands);
}

/// A body about to go or change type: every sleeping body touching any of its colliders wakes
/// (module doc).
fn wake_on_body_gone(
    gone: On<Discard, RigidBody>,
    bodies: Query<&RigidBodyColliders>,
    contacts: Res<ContactGraph>,
    sleeping: Query<(), With<Sleeping>>,
    mut commands: Commands,
) {
    let Ok(colliders) = bodies.get(gone.entity) else {
        return;
    };
    for collider in colliders {
        wake_touching(collider, &contacts, &sleeping, &mut commands);
    }
}

/// A collider disabled: every sleeping body with a collider whose bounds come within avian's
/// contact tolerance of its bounds wakes (module doc).
fn wake_on_collider_disabled(
    disabled: On<Add, ColliderDisabled>,
    aabbs: Query<&ColliderAabb>,
    spatial: SpatialQuery,
    collider_of: Query<&ColliderOf>,
    config: Res<NarrowPhaseConfig>,
    sleeping: Query<(), With<Sleeping>>,
    mut commands: Commands,
) {
    let aabb = aabbs
        .get(disabled.entity)
        .expect("`ColliderDisabled` marks a collider, which carries its bounds");
    let grown = ColliderAabb::from_min_max(
        aabb.min - Vec3::splat(config.contact_tolerance),
        aabb.max + Vec3::splat(config.contact_tolerance),
    );
    spatial.aabb_intersections_with_aabb_callback(grown, |other| {
        if let Ok(&ColliderOf { body }) = collider_of.get(other)
            && sleeping.contains(body)
        {
            commands.entity(body).try_remove::<Sleeping>();
        }
        true
    });
}

/// Every collider of a static body re-posed since the last step: every sleeping body touching it
/// wakes (module doc).
fn wake_on_static_moved(
    moved: Query<(Entity, &ColliderOf), Or<(Changed<Position>, Changed<Rotation>)>>,
    bodies: Query<&RigidBody>,
    contacts: Res<ContactGraph>,
    sleeping: Query<(), With<Sleeping>>,
    mut commands: Commands,
) {
    for (collider, of) in &moved {
        if bodies.get(of.body).is_ok_and(RigidBody::is_static) {
            wake_touching(collider, &contacts, &sleeping, &mut commands);
        }
    }
}

struct FloorRetirement {
    entity: Entity,
    height: f32,
    speed: f32,
    /// Stress-fixture pieces have no construction ledger state.
    entries: Option<Vec<crate::construction::breakage::Entry>>,
}

/// The game's destruction model: no escaped piece falls forever. Independent bodies prepare
/// their departures in parallel; ledger entries, carrier retirement and body removal commit
/// together in entity order before collider snapshots can expose them to the next march.
fn retire_below_floor(
    map: Res<crate::map::MapManifest>,
    pieces: Query<
        (
            Entity,
            &Position,
            &LinearVelocity,
            Option<&crate::construction::breakage::Piece>,
        ),
        With<DebrisPiece>,
    >,
    clock: Option<Res<crate::ShotClock>>,
    mut commands: Commands,
) {
    use crate::construction::breakage::{Entry, Event, Record, RetirePiece};
    let floor = map.play_area.floor_m();
    // Clockless analytical fixtures publish at their explicit zero cursor.
    let tick = clock.as_deref().map_or(0, |clock| clock.0.raw());
    let mut departures = bevy::utils::Parallel::<Vec<FloorRetirement>>::default();
    pieces
        .par_iter()
        .for_each(|(entity, position, velocity, piece)| {
            if position.y < floor {
                let entries = piece.map(|piece| {
                    piece
                        .chunks
                        .iter()
                        .map(|&chunk| Entry {
                            construction: piece.construction,
                            event: Event::Dust(chunk),
                            tick,
                        })
                        .collect()
                });
                departures.scope(|departures| {
                    departures.push(FloorRetirement {
                        entity,
                        height: position.y,
                        speed: velocity.length(),
                        entries,
                    })
                });
            }
        });
    let mut departures: Vec<_> = departures.drain().collect();
    departures.sort_unstable_by_key(|departure| departure.entity);
    if departures.is_empty() {
        return;
    }
    for departure in &departures {
        warn!(piece = %departure.entity, speed_mps = departure.speed, height_m = departure.height,
            floor_m = floor, "debris: retiring piece below the play-area floor");
    }
    commands.queue(move |world: &mut World| {
        for departure in departures {
            if let Some(entries) = departure.entries {
                world.resource_mut::<Record>().entries.extend(entries);
                world.trigger(RetirePiece {
                    entity: departure.entity,
                    tick,
                });
            }
            assert!(
                world.despawn(departure.entity),
                "the floor gate owns each selected live piece until retirement"
            );
        }
    });
}

fn assert_tanks_above_floor(
    map: Res<crate::map::MapManifest>,
    tanks: Query<(Entity, &Position), With<crate::tank::Tank>>,
) {
    let floor = map.play_area.floor_m();
    tanks.par_iter().for_each(|(tank, position)| {
        assert!(position.y >= floor,
            "tank {tank} below play-area floor: y={} m, floor={floor} m; hull containment is broken", position.y);
    });
}

/// Mount the rule.
pub(crate) fn sim_plugin(app: &mut App) {
    app.init_resource::<crate::map::MapManifest>()
        .add_observer(wake_on_collider_gone)
        .add_observer(wake_on_body_gone)
        .add_observer(wake_on_collider_disabled)
        .add_systems(
            PhysicsSchedule,
            wake_on_static_moved
                .after(PhysicsStepSystems::First)
                .before(PhysicsStepSystems::BroadPhase),
        )
        .add_systems(
            FixedPostUpdate,
            (retire_below_floor, assert_tanks_above_floor)
                .after(PhysicsSystems::Writeback)
                .before(crate::state::SimPhase::ColliderPoses)
                .run_if(not(resource_exists::<crate::ClientReplica>)),
        );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::Layer;

    /// The rule on avian, one fixed tick per update at 64 Hz, gravity `gravity`.
    fn settling(gravity: Vec3) -> App {
        let mut app = App::new();
        // Fixture: bounds enclose the analytical bodies independently of the selected map.
        app.insert_resource(crate::map::tests::fixture_manifest(4096.0, -1024.0, 2048.0));
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            sim_plugin,
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

    /// The game's destruction model: an escaped piece leaves within one tick, publishes the
    /// chunks as dust, and retires its carrier at the authority tick. Replicas rely on both facts.
    #[test]
    fn a_piece_below_floor_retires_with_its_ledger_and_carrier_tick() {
        use crate::construction::breakage::{Entry, Event, Piece, Record, RetirePiece};
        #[derive(Resource, Default)]
        struct Retired(Vec<(Entity, u32)>);
        let mut app = settling(Vec3::ZERO);
        // Fixture: a checked map with construction ordinals, and an explicit authority tick.
        let map = crate::map::tests::construction_fixture_manifest();
        let floor = map.play_area.floor_m();
        app.insert_resource(map)
            .insert_resource(crate::ShotClock(crate::sim_tick::SimTick::new(7)))
            .init_resource::<Record>()
            .init_resource::<Retired>()
            .add_observer(|retired: On<RetirePiece>, mut log: ResMut<Retired>| {
                log.0.push((retired.entity, retired.tick));
            });
        app.update();
        let below = piece(&mut app, Vec3::new(0.0, floor - 1.0, 0.0));
        // Fixture: ordinal zero is a breakable construction; chunk zero belongs to this group.
        app.world_mut().entity_mut(below).insert((
            Piece {
                construction: 0,
                chunks: vec![0],
                born: 0,
                retired: None,
            },
            LinearVelocity(Vec3::new(3.0, 4.0, 0.0)),
        ));
        let at_floor = piece(&mut app, Vec3::new(3.0, floor, 0.0));
        let above = piece(&mut app, Vec3::new(6.0, floor + 1.0, 0.0));
        app.update();
        assert!(app.world().get_entity(below).is_err());
        assert!(app.world().get_entity(at_floor).is_ok());
        assert!(app.world().get_entity(above).is_ok());
        assert_eq!(
            app.world().resource::<Record>().entries,
            [Entry {
                construction: 0,
                event: Event::Dust(0),
                tick: 7
            }]
        );
        assert_eq!(app.world().resource::<Retired>().0, [(below, 7)]);
    }

    /// The shared timeline, not a replica's local position, owns piece retirement.
    #[test]
    fn a_replica_waits_for_authority_below_the_floor() {
        let mut app = settling(Vec3::ZERO);
        app.init_resource::<crate::ClientReplica>();
        app.update();
        let floor = app
            .world()
            .resource::<crate::map::MapManifest>()
            .play_area
            .floor_m();
        let below = piece(&mut app, Vec3::new(0.0, floor - 1.0, 0.0));
        app.update();
        assert!(app.world().get_entity(below).is_ok());
    }

    /// The server relies on hull containment; a named tank escaping is a bug in every build.
    #[test]
    #[should_panic(expected = "below play-area floor")]
    fn a_tank_below_the_floor_is_a_containment_bug() {
        let mut app = settling(Vec3::ZERO);
        app.update();
        let floor = app
            .world()
            .resource::<crate::map::MapManifest>()
            .play_area
            .floor_m();
        app.world_mut()
            .spawn((crate::tank::Tank, Position(Vec3::Y * (floor - 1.0))));
        app.update();
    }

    /// A 0.4 m cube piece at `at`, dynamic (fixture data).
    fn piece(app: &mut App, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                DebrisPiece,
                Position(at),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(0.4, 0.4, 0.4),
                ColliderDensity(1800.0),
                CollisionLayers::new(
                    [Layer::Debris],
                    [
                        Layer::Terrain,
                        Layer::Vehicle,
                        Layer::Debris,
                        Layer::TrackSide,
                    ],
                ),
            ))
            .id()
    }

    /// A static 20 × 1 × 20 m slab whose top is at `top`.
    fn slab(app: &mut App, top: f32) -> Entity {
        app.world_mut()
            .spawn((
                RigidBody::Static,
                Position(Vec3::new(0.0, top - 0.5, 0.0)),
                Collider::cuboid(20.0, 1.0, 20.0),
                CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
            ))
            .id()
    }

    /// Run until `piece` sleeps, at most 10 s; the tick it fell asleep.
    fn until_asleep(app: &mut App, piece: Entity) -> u32 {
        (0..640)
            .find(|_| {
                app.update();
                app.world().get::<Sleeping>(piece).is_some()
            })
            .expect("the piece came to rest and slept within 10 s")
    }

    /// The same cube with no [`DebrisPiece`]: a launched turret's stand-in (fixture data).
    fn other_body(app: &mut App, at: Vec3) -> Entity {
        let body = piece(app, at);
        app.world_mut().entity_mut(body).remove::<DebrisPiece>();
        body
    }

    /// `resting`, asleep on a support at 1 m that `go` has just taken away: awake in the tick
    /// after, and fallen below the support's top within 16 more.
    fn wakes_and_falls(app: &mut App, resting: Entity) {
        app.update();
        assert!(app.world().get::<Sleeping>(resting).is_none(), "it woke");
        for _ in 0..16 {
            app.update();
        }
        assert!(
            app.world().get::<Position>(resting).unwrap().0.y < 1.1,
            "it falls"
        );
    }

    /// A SLEEPING BODY WAKES WHEN WHAT IT RESTS ON GOES, A PIECE OR ANY OTHER: a piece, and a body
    /// that is no piece (a launched turret), asleep on a static slab wake in the tick after the
    /// slab despawns (a chunk turned to dust under them) and fall. Avian wakes nothing for a
    /// static collider's removal, so a mutant that drops [`wake_on_collider_gone`], or wakes only
    /// pieces, leaves a body hanging where the slab was.
    #[test]
    fn a_sleeping_body_wakes_when_its_support_goes() {
        for spawn in [piece as fn(&mut App, Vec3) -> Entity, other_body] {
            let mut app = settling(Vec3::NEG_Y * 9.81);
            let support = slab(&mut app, 1.0);
            let resting = spawn(&mut app, Vec3::Y * 1.25);
            until_asleep(&mut app, resting);
            app.world_mut().entity_mut(support).despawn();
            wakes_and_falls(&mut app, resting);
        }
    }

    /// A SLEEPING BODY WAKES WHEN ITS SUPPORT'S STATIC ROOT GOES WITH IT: a body asleep on a slab
    /// that is the child collider of a static root wakes when the root is despawned, and falls.
    /// The children's despawn is queued after avian's `Remove<RigidBody>` has dropped their pairs,
    /// so a mutant that drops [`wake_on_body_gone`] leaves the body hanging.
    #[test]
    fn a_sleeping_body_wakes_when_its_supports_root_goes() {
        let mut app = settling(Vec3::NEG_Y * 9.81);
        let at = Vec3::new(0.0, 0.5, 0.0);
        let root = app
            .world_mut()
            .spawn((
                Transform::from_translation(at),
                Position(at),
                RigidBody::Static,
            ))
            .id();
        app.world_mut().spawn((
            ChildOf(root),
            Transform::IDENTITY,
            Collider::cuboid(20.0, 1.0, 20.0),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
        ));
        let resting = piece(&mut app, Vec3::Y * 1.25);
        until_asleep(&mut app, resting);
        app.world_mut().entity_mut(root).despawn();
        wakes_and_falls(&mut app, resting);
    }

    /// A SLEEPING BODY WAKES WHEN ITS SUPPORT MOVES: a body asleep on a static slab wakes when the
    /// slab is re-posed a metre down, and falls after it. Avian wakes no body for a static
    /// collider's move, so a mutant that drops [`wake_on_static_moved`] leaves the body hanging
    /// where the slab was; a falling tree's trunk under a piece relies on it.
    #[test]
    fn a_sleeping_body_wakes_when_its_support_moves() {
        let mut app = settling(Vec3::NEG_Y * 9.81);
        let support = slab(&mut app, 1.0);
        let resting = piece(&mut app, Vec3::Y * 1.25);
        until_asleep(&mut app, resting);
        app.world_mut().get_mut::<Position>(support).unwrap().0 += Vec3::NEG_Y;
        wakes_and_falls(&mut app, resting);
    }

    /// A SLEEPING BODY WAKES WHEN ITS SUPPORT IS DISABLED: a body asleep on a static slab wakes
    /// when the slab takes `ColliderDisabled`, and falls through it. Avian drops the slab's pairs
    /// and wakes no other body, so a mutant that drops [`wake_on_collider_disabled`] leaves the
    /// body hanging over a slab it no longer meets.
    #[test]
    fn a_sleeping_body_wakes_when_its_support_is_disabled() {
        let mut app = settling(Vec3::NEG_Y * 9.81);
        let support = slab(&mut app, 1.0);
        let resting = piece(&mut app, Vec3::Y * 1.25);
        until_asleep(&mut app, resting);
        app.world_mut().entity_mut(support).insert(ColliderDisabled);
        wakes_and_falls(&mut app, resting);
    }

    /// A BODY THAT TRULY DISTURBS A SLEEPING PIECE WAKES IT AND MEETS FINITE MASS: a hovering body
    /// sliding into a piece asleep on the ground, a tank's stand-in (no piece) or a thrown piece,
    /// finds it awake and moving in the first tick the two touch, and pushes it. A rule that rested
    /// pieces by making them static would meet the first contact with infinite mass; one that woke
    /// only for a tank would leave a thrown piece bouncing off a fixed block. No game code runs
    /// here: this is the tripwire on avian's wake-on-touch (`collision/narrow_phase/system_param.rs:
    /// 247-261`, `dynamics/solver/islands/mod.rs:964-966`) the module's rest rule rests on, and
    /// the one test of it.
    #[test]
    fn a_body_that_touches_a_sleeping_piece_wakes_and_pushes_it() {
        for mover_is_a_piece in [false, true] {
            let mut app = settling(Vec3::NEG_Y * 9.81);
            slab(&mut app, 0.0);
            let rubble = piece(&mut app, Vec3::Y * 0.2);
            until_asleep(&mut app, rubble);
            let start = app.world().get::<Position>(rubble).unwrap().0;
            let mut mover = app.world_mut().spawn((
                Position(Vec3::new(-1.5, 0.45, 0.0)),
                Rotation::default(),
                RigidBody::Dynamic,
                GravityScale(0.0),
                Collider::cuboid(0.6, 0.4, 0.6),
                ColliderDensity(1000.0),
                LinearVelocity(Vec3::X * 4.0),
            ));
            if mover_is_a_piece {
                mover.insert((
                    DebrisPiece,
                    CollisionLayers::new([Layer::Debris], LayerMask::ALL),
                ));
            } else {
                mover.insert(CollisionLayers::new([Layer::Vehicle], LayerMask::ALL));
            }
            let mover = mover.id();
            let first_touch = (0..64).find(|_| {
                app.update();
                app.world()
                    .resource::<ContactGraph>()
                    .get(mover, rubble)
                    .is_some_and(|(edge, _)| edge.is_touching())
            });
            assert!(first_touch.is_some(), "the mover reached the rubble");
            assert!(
                app.world().get::<Sleeping>(rubble).is_none(),
                "awake at the first touch (mover a piece: {mover_is_a_piece})"
            );
            assert!(
                app.world().get::<LinearVelocity>(rubble).unwrap().0.x > 0.0,
                "the first touch moved it (mover a piece: {mover_is_a_piece})"
            );
            for _ in 0..32 {
                app.update();
            }
            assert!(
                app.world().get::<Position>(rubble).unwrap().0.x - start.x > 0.05,
                "the rubble was pushed (mover a piece: {mover_is_a_piece})"
            );
        }
    }
}
