//! Server-simulated pieces settling, in the game's environment destruction (game state only): the
//! one rule every loose piece follows, a building's fallen chunks ([`crate::buildings::breakage`])
//! and the debris stress lever's fixture fragments (`net::debris_stress`) alike. The authority
//! alone runs it; a replica draws a piece at the pose the network carries. A piece is a body with
//! [`DebrisPiece`]; its colliders are the body's own ([`RigidBodyColliders`]).
//!
//! THE FREEZE RULE ([`freeze_rested`]): a live piece whose speed stays under [`REST_SPEED_M_S`] and
//! spin under [`REST_SPIN_RAD_S`] for [`REST_S`], touching something and no dynamic body but other
//! pieces, becomes `RigidBody::Static`, remembering what it touched ([`RestsOn`]). Avian 0.7 then
//! takes it out of the solver and the islands, moves its proxy to the static tree, and drops every
//! contact pair it held (`remove_body_on::<Insert, RigidBody>` in its narrow phase), and its broad
//! phase never pairs two static bodies: a frozen piece costs the physics a static proxy and nothing
//! per tick. Avian's sleeping was rejected: it sleeps whole islands, so neither the rule nor the cap
//! could freeze one piece, and a piece touching a tank shares the tank's island.
//!
//! THE CAP, BY FREEZING ONLY RESTING PIECES: past [`DebrisSettle::live_cap`] live pieces
//! ([`DEFAULT_LIVE_CAP`]; `None` is none), the oldest live pieces that are at rest now, touch
//! something and touch no dynamic body freeze without waiting out [`REST_S`]. A moving or
//! unsupported piece never freezes, so the cap bounds the live pieces once the moving ones settle,
//! not while they move: the cap is an upper bound on resting live pieces, never a mid-air support.
//!
//! WAKING, before the physics step, so the step that would meet a frozen piece meets a body:
//! * a frozen piece whose support is gone or moves wakes ([`wake_unsupported`]): a collider it
//!   rested on despawned (a chunk turned to dust or fallen, an element swapped to its chunks) or
//!   belongs to a live piece moving faster than [`REST_SPEED_M_S`];
//! * a frozen piece a dynamic body other than a piece is about to reach wakes ([`wake_ahead`]): its
//!   bounds overlap the body's grown by the distance it covers in the step, so a tank's first
//!   contact pushes rubble rather than meeting a fixed block.
//!
//! [`thaw_touched`] after the step wakes one a moving body touched all the same. A piece stays all
//! match: nothing here despawns one.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::ClientReplica;

/// The live-piece cap by default. ASSUMED: one 200-piece breach and the tail of the one before it.
pub(crate) const DEFAULT_LIVE_CAP: u32 = 256;

/// At rest: speed under this, m/s. The rest constants are avian 0.7's sleep defaults
/// (`SleepThreshold`, `TimeToSleep`), ASSUMED to suit a piece.
pub(crate) const REST_SPEED_M_S: f32 = 0.15;
/// At rest: spin under this, rad/s.
pub(crate) const REST_SPIN_RAD_S: f32 = 0.15;
/// How long a piece stays at rest before it freezes, s.
pub(crate) const REST_S: f32 = 0.5;

/// How far past the distance a moving body covers in a step its bounds reach for a frozen piece,
/// m: avian 0.7's broad-phase margin (`AABB_MARGIN`, 0.05 m, `collider_tree/update.rs`) twice
/// over, so a pair the broad phase would make this step finds the piece awake (DERIVED).
const WAKE_MARGIN_M: f32 = 0.1;

/// A piece body under the rule: its release order, numbered from [`DebrisSettle::released`] as it
/// is added. A piece spawns dynamic with it.
#[derive(Component, Clone, Copy, Debug, Default)]
#[require(DebrisRest)]
pub(crate) struct DebrisPiece {
    pub(crate) seq: u64,
}

/// Seconds a live piece has spent at rest, reset by any motion over the rest thresholds.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct DebrisRest(f32);

/// A piece frozen by the freeze rule or the cap: `RigidBody::Static`, its pose final until a wake.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct Frozen;

/// The colliders a frozen piece touched when it froze, none its own: what it rests on.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub(crate) struct RestsOn(pub(crate) Vec<Entity>);

/// The rule's cap and its running counts since the authority started: pieces released, frozen by
/// the rest rule and by the cap, woken.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub(crate) struct DebrisSettle {
    pub(crate) live_cap: Option<u32>,
    pub(crate) released: u64,
    pub(crate) rest_frozen: u64,
    pub(crate) cap_frozen: u64,
    pub(crate) thawed: u64,
}

impl Default for DebrisSettle {
    fn default() -> Self {
        Self {
            live_cap: Some(DEFAULT_LIVE_CAP),
            released: 0,
            rest_frozen: 0,
            cap_frozen: 0,
            thawed: 0,
        }
    }
}

/// Number each piece as it is added, in release order.
fn number(
    added: On<Add, DebrisPiece>,
    mut settle: ResMut<DebrisSettle>,
    mut pieces: Query<&mut DebrisPiece>,
) {
    if let Ok(mut piece) = pieces.get_mut(added.entity) {
        piece.seq = settle.released;
        settle.released += 1;
    }
}

/// The colliders `piece`'s colliders touch, with each one's body.
fn touching<'a>(
    colliders: &'a RigidBodyColliders,
    collisions: &'a Collisions,
) -> impl Iterator<Item = (Entity, Option<Entity>)> + 'a {
    colliders.iter().flat_map(move |own| {
        collisions.collisions_with(own).filter_map(move |pair| {
            let other = if pair.collider1 == own {
                (pair.collider2, pair.body2)
            } else {
                (pair.collider1, pair.body1)
            };
            (!colliders.iter().any(|c| c == other.0)).then_some(other)
        })
    })
}

/// Whether a body touching `piece` is dynamic and no piece: it rides or leans on a vehicle, so
/// freezing it would leave a fixed block in that vehicle's way.
fn touches_vehicle(
    colliders: &RigidBodyColliders,
    collisions: &Collisions,
    vehicles: &Query<&RigidBody, Without<DebrisPiece>>,
) -> bool {
    touching(colliders, collisions).any(|(_, body)| {
        body.and_then(|body| vehicles.get(body).ok())
            .is_some_and(RigidBody::is_dynamic)
    })
}

/// The switch both the freeze rule and the cap make, remembering what the piece rests on.
/// `CollisionEventsEnabled` rides the same insert so the static proxy avian rebuilds for the new
/// body type carries contact events ([`thaw_touched`] reads them); the velocities are zeroed so a
/// wake starts the piece from rest.
fn freeze(commands: &mut Commands, piece: Entity, rests_on: Vec<Entity>) {
    commands.entity(piece).insert((
        Frozen,
        RestsOn(rests_on),
        CollisionEventsEnabled,
        LinearVelocity::ZERO,
        AngularVelocity::ZERO,
        RigidBody::Static,
    ));
}

/// The switch back: dynamic again, from rest, its rest timer cleared.
fn wake(commands: &mut Commands, piece: Entity) {
    commands
        .entity(piece)
        .remove::<(Frozen, RestsOn, CollisionEventsEnabled)>()
        .insert((DebrisRest::default(), RigidBody::Dynamic));
}

/// The live pieces the cap freezes: oldest release first, past `cap`, taking only those for which
/// `resting` holds. `live` is `(seq, piece)`, sorted here.
pub(crate) fn past_cap(
    live: &mut [(u64, Entity)],
    cap: u32,
    resting: impl Fn(Entity) -> bool,
) -> Vec<Entity> {
    let excess = live.len().saturating_sub(cap as usize);
    if excess == 0 {
        return Vec::new();
    }
    live.sort_unstable_by_key(|(seq, _)| *seq);
    live.iter()
        .map(|(_, piece)| *piece)
        .filter(|piece| resting(*piece))
        .take(excess)
        .collect()
}

/// `FixedPostUpdate`, after the step: THE FREEZE RULE, then THE CAP (module doc).
pub(crate) fn freeze_rested(
    time: Res<Time>,
    mut settle: ResMut<DebrisSettle>,
    collisions: Collisions,
    vehicles: Query<&RigidBody, Without<DebrisPiece>>,
    mut live: Query<
        (
            Entity,
            &DebrisPiece,
            &mut DebrisRest,
            &LinearVelocity,
            &AngularVelocity,
            &RigidBodyColliders,
        ),
        Without<Frozen>,
    >,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let mut remaining = Vec::new();
    let mut resting = Vec::new();
    for (piece, body, mut rest, velocity, spin, colliders) in &mut live {
        let at_rest = velocity.0.length() < REST_SPEED_M_S && spin.0.length() < REST_SPIN_RAD_S;
        rest.0 = if at_rest { rest.0 + dt } else { 0.0 };
        let rests_on: Vec<Entity> = touching(colliders, &collisions).map(|(c, _)| c).collect();
        let settled =
            at_rest && !rests_on.is_empty() && !touches_vehicle(colliders, &collisions, &vehicles);
        if settled && rest.0 >= REST_S {
            freeze(&mut commands, piece, rests_on);
            settle.rest_frozen += 1;
            continue;
        }
        remaining.push((body.seq, piece));
        if settled {
            resting.push((piece, rests_on));
        }
    }
    let Some(cap) = settle.live_cap else {
        return;
    };
    for piece in past_cap(&mut remaining, cap, |piece| {
        resting.iter().any(|(p, _)| *p == piece)
    }) {
        if let Some((_, rests_on)) = resting.iter_mut().find(|(p, _)| *p == piece) {
            freeze(&mut commands, piece, std::mem::take(rests_on));
            settle.cap_frozen += 1;
        }
    }
}

/// `FixedPostUpdate`, before the step: wake every frozen piece whose support is gone or moves
/// (module doc).
pub(crate) fn wake_unsupported(
    frozen: Query<(Entity, &RestsOn), With<Frozen>>,
    colliders: Query<&ColliderOf>,
    pieces: Query<&LinearVelocity, (With<DebrisPiece>, Without<Frozen>)>,
    mut settle: ResMut<DebrisSettle>,
    mut commands: Commands,
) {
    for (piece, rests_on) in &frozen {
        // A live piece at rest still holds what froze on it: two pieces resting against each
        // other freeze one after the other, and either moving wakes the other.
        let gone = rests_on
            .0
            .iter()
            .any(|&support| match colliders.get(support) {
                Err(_) => true,
                Ok(of) => pieces
                    .get(of.body)
                    .is_ok_and(|velocity| velocity.0.length() >= REST_SPEED_M_S),
            });
        if gone {
            wake(&mut commands, piece);
            settle.thawed += 1;
        }
    }
}

/// `FixedPostUpdate`, before the step: wake every frozen piece a moving body that is no piece
/// reaches this step (module doc).
pub(crate) fn wake_ahead(
    time: Res<Time>,
    frozen: Query<(Entity, &RigidBodyColliders), With<Frozen>>,
    movers: Query<(&RigidBody, &LinearVelocity, &RigidBodyColliders), Without<DebrisPiece>>,
    bounds: Query<&ColliderAabb>,
    mut settle: ResMut<DebrisSettle>,
    mut commands: Commands,
) {
    if frozen.is_empty() {
        return;
    }
    let union = |colliders: &RigidBodyColliders| -> Option<(Vec3, Vec3)> {
        colliders
            .iter()
            .filter_map(|c| bounds.get(c).ok())
            .map(|aabb| (aabb.min, aabb.max))
            .reduce(|(lo, hi), (a, b)| (lo.min(a), hi.max(b)))
    };
    let reach: Vec<(Vec3, Vec3)> = movers
        .iter()
        .filter(|(body, ..)| body.is_dynamic())
        .filter_map(|(_, velocity, colliders)| {
            let grow = Vec3::splat(velocity.0.length() * time.delta_secs() + WAKE_MARGIN_M);
            union(colliders).map(|(lo, hi)| (lo - grow, hi + grow))
        })
        .collect();
    for (piece, colliders) in &frozen {
        let Some((lo, hi)) = union(colliders) else {
            continue;
        };
        if reach
            .iter()
            .any(|(a, b)| lo.cmple(*b).all() && hi.cmpge(*a).all())
        {
            wake(&mut commands, piece);
            settle.thawed += 1;
        }
    }
}

/// `FixedPostUpdate`, after the step: a frozen piece a moving body that is no piece started
/// touching wakes. A frozen piece carries `CollisionEventsEnabled`, so its new pairs report their
/// start here.
pub(crate) fn thaw_touched(
    mut started: MessageReader<CollisionStart>,
    frozen: Query<(), With<Frozen>>,
    vehicles: Query<&RigidBody, Without<DebrisPiece>>,
    mut settle: ResMut<DebrisSettle>,
    mut thawed: Local<Vec<Entity>>,
    mut commands: Commands,
) {
    thawed.clear();
    for event in started.read() {
        for (piece, other) in [(event.body1, event.body2), (event.body2, event.body1)] {
            let (Some(piece), Some(other)) = (piece, other) else {
                continue;
            };
            let pushed = vehicles.get(other).is_ok_and(RigidBody::is_dynamic);
            if frozen.contains(piece) && pushed && !thawed.contains(&piece) {
                thawed.push(piece);
                wake(&mut commands, piece);
            }
        }
    }
    settle.thawed += thawed.len() as u64;
}

/// The rule's place in the fixed step: the wakes before the physics step, the thaw and the freeze
/// after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DebrisSettleSet {
    Wake,
    Settle,
}

/// Mount the rule on the authority.
pub(crate) fn sim_plugin(app: &mut App) {
    let authority = not(resource_exists::<ClientReplica>);
    app.init_resource::<DebrisSettle>()
        .add_observer(number)
        .configure_sets(
            FixedPostUpdate,
            (
                DebrisSettleSet::Wake.before(PhysicsSystems::StepSimulation),
                DebrisSettleSet::Settle.after(PhysicsSystems::StepSimulation),
            ),
        )
        .add_systems(
            FixedPostUpdate,
            (
                (wake_unsupported, wake_ahead)
                    .chain()
                    .in_set(DebrisSettleSet::Wake)
                    .run_if(authority.clone()),
                (thaw_touched, freeze_rested)
                    .chain()
                    .in_set(DebrisSettleSet::Settle)
                    .run_if(authority),
            ),
        );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::Layer;

    /// The rule on avian, one fixed tick per update at 64 Hz, the cap at `cap`, gravity `gravity`.
    fn settling(cap: Option<u32>, gravity: Vec3) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
            sim_plugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity(gravity))
        .insert_resource(DebrisSettle {
            live_cap: cap,
            ..default()
        })
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

    /// A 0.4 m cube piece at `at`, dynamic (fixture data).
    fn piece(app: &mut App, at: Vec3) -> Entity {
        app.world_mut()
            .spawn((
                DebrisPiece::default(),
                Position(at),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(0.4, 0.4, 0.4),
                ColliderDensity(1800.0),
                CollisionLayers::new(
                    [Layer::Debris],
                    [Layer::Terrain, Layer::Vehicle, Layer::Debris],
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

    /// THE CAP NEVER MAKES A MID-AIR SUPPORT: with the cap at none live, a piece dropped from 3 m
    /// is never frozen while it falls, and freezes once it rests on the ground. A cap that froze
    /// the oldest live piece wherever it was would hang it in the air on the first tick.
    #[test]
    fn the_cap_freezes_no_falling_piece() {
        let mut app = settling(Some(0), Vec3::NEG_Y * 9.81);
        slab(&mut app, 0.0);
        let falling = piece(&mut app, Vec3::Y * 3.0);
        let mut froze = None;
        for tick in 0..640 {
            app.update();
            if app.world().get::<Frozen>(falling).is_some() {
                froze = Some(tick);
                break;
            }
        }
        let tick = froze.expect("the piece came to rest and froze within 10 s");
        let height = app.world().get::<Position>(falling).unwrap().0.y;
        assert!(
            height < 0.25,
            "frozen at tick {tick} at {height} m, on the ground"
        );
        assert_eq!(app.world().resource::<DebrisSettle>().cap_frozen, 1);
    }

    /// A FROZEN PIECE WAKES WHEN WHAT IT RESTS ON GOES: a piece frozen on a slab is dynamic again
    /// in the tick after the slab despawns (a chunk turned to dust under it), and falls. A rule
    /// that woke only on a vehicle's touch would leave it hanging where the slab was.
    #[test]
    fn a_frozen_piece_wakes_when_its_support_goes() {
        let mut app = settling(None, Vec3::NEG_Y * 9.81);
        let support = slab(&mut app, 1.0);
        let resting = piece(&mut app, Vec3::Y * 1.25);
        for _ in 0..640 {
            app.update();
            if app.world().get::<Frozen>(resting).is_some() {
                break;
            }
        }
        assert!(app.world().get::<Frozen>(resting).is_some(), "it froze");
        app.world_mut().entity_mut(support).despawn();
        app.update();
        assert!(app.world().get::<Frozen>(resting).is_none());
        assert_eq!(
            app.world().get::<RigidBody>(resting),
            Some(&RigidBody::Dynamic)
        );
        for _ in 0..16 {
            app.update();
        }
        assert!(
            app.world().get::<Position>(resting).unwrap().0.y < 1.2,
            "it falls"
        );
    }

    /// A MOVING BODY'S FIRST CONTACT PUSHES RUBBLE: a dynamic body that is no piece, closing on a
    /// frozen piece, finds it dynamic in every tick the two touch, and moves it. A wake that ran
    /// after the step would let the first contact meet a fixed block of infinite mass.
    #[test]
    fn a_moving_body_first_meets_a_woken_piece() {
        let mut app = settling(None, Vec3::ZERO);
        let ground = slab(&mut app, -5.0);
        let rubble = piece(&mut app, Vec3::ZERO);
        app.world_mut().entity_mut(rubble).insert((
            Frozen,
            RestsOn(vec![ground]),
            RigidBody::Static,
        ));
        let mover = app
            .world_mut()
            .spawn((
                Position(Vec3::new(-1.5, 0.0, 0.0)),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                ColliderDensity(1000.0),
                LinearVelocity(Vec3::X * 4.0),
                CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            ))
            .id();
        let mut touched = false;
        for _ in 0..64 {
            app.update();
            let world = app.world_mut();
            let touching = world
                .resource::<ContactGraph>()
                .get(mover, rubble)
                .is_some_and(|(_, pair)| pair.is_touching());
            if touching {
                touched = true;
                assert_eq!(
                    world.get::<RigidBody>(rubble),
                    Some(&RigidBody::Dynamic),
                    "the rubble is a body when the contact is solved"
                );
            }
        }
        assert!(touched, "the mover reached the rubble");
        assert!(
            app.world().get::<Position>(rubble).unwrap().0.x > 0.05,
            "the rubble was pushed"
        );
    }

    /// THE CAP TAKES THE OLDEST RESTING PIECES FIRST AND NEVER ONE THAT MOVES, HANGS OR RIDES: a
    /// mutant that takes the newest leaves the stalest bodies costing the solver, and one that
    /// freezes a piece that is not resting manufactures a fixed block in mid-air or in a tank's
    /// path. The game's pieces and the stress lever's share it.
    #[test]
    fn the_cap_freezes_the_oldest_resting_pieces() {
        let piece = |n: u32| Entity::from_raw_u32(n).expect("a valid index");
        let mut live: Vec<(u64, Entity)> = [3, 1, 4, 0, 2]
            .into_iter()
            .map(|seq| (seq, piece(seq as u32 + 1)))
            .collect();
        let moving = piece(1);
        let frozen = past_cap(&mut live, 3, |p| p != moving);
        assert_eq!(frozen, vec![piece(2), piece(3)]);
        assert!(past_cap(&mut live, 5, |_| true).is_empty());
        assert!(past_cap(&mut live, 0, |_| false).is_empty());
    }
}
