//! Settled loose pieces turned static ("frozen") and back ("thawed"), in the game's environment
//! destruction (game state only), on the authority alone. Paths below are under
//! `vendor/avian3d-0.7.0-child-pose/src/`.
//!
//! WHY. A sleeping island keeps every pair that overlaps in bounds without touching active, and
//! the narrow phase recomputes those every tick ([`super`] module doc). The broad phase pairs no
//! two static colliders, so a settled pile turned static leaves the narrow phase nothing to do.
//!
//! FREEZE ([`freeze`], after the step's splits). An island Avian has put to sleep whose every body
//! is a loose piece, that touches something outside itself, and that no mover reaches, turns
//! static whole in one tick: rubble interlocks, so the island is the unit, and a whole island
//! leaves no piece resting on a dynamic one. An island holding any other body (a tank, a launched
//! turret) stays asleep, and so does one touching nothing (a piece asleep in zero gravity). Each
//! piece takes [`RigidBody::Static`], zero velocity and [`Frozen`], and loses `Sleeping`. The
//! island depends on its pieces' colliders and on every collider they touched as it froze
//! ([`FrozenIslands`]): Avian's sleep under gravity is the evidence that those held it. Frozen
//! islands never touch each other: a body settling onto a frozen piece reaches it, and thaws it,
//! before the two touch, so what an island touches as it freezes is fixed matter (the freeze
//! asserts it).
//!
//! THAW ([`thaw`], before the sweep and physics). A frozen island turns dynamic whole. Avian
//! zeroes a body's rest timer as it rejoins an island (`dynamics/solver/islands/sleeping.rs`,
//! `wake_on_enable_rigid_body`), so the island rests a full `TimeToSleep` again before it can
//! sleep and freeze. Three causes:
//! - reach: a mover (an awake body that is not static) whose colliders' bounds, grown by
//!   [`REACH_TICKS`] of its travel at its present speed and by Avian's contact tolerance, meet a
//!   piece. Avian sweeps a moving body's bounds over its next step at the end of each step
//!   (`collider_tree/update.rs`, `update_solver_body_aabbs`), at the speed that step ended with; the
//!   travel covers a speed game code wrote since (a push, a launch), which the next step's
//!   speculative contacts use while its broad phase still pairs from the stale bounds, up to
//!   `AABB_MARGIN` apart. So the island is dynamic before any step that solves a contact with the
//!   mover, and the contact meets finite mass. A body spawned since the last step has no bounds
//!   yet and reaches nothing until its first step;
//! - load: a piece whose velocity is not zero, since game code pushed it (Avian's `Forces` writes
//!   a static body's velocity as any other's), as a velocity written onto a sleeping body wakes
//!   its island (`dynamics/solver/islands/sleeping.rs`, `wake_on_changed`);
//! - support: a collider the island depends on goes, leaves its body, is disabled or moves, or its
//!   body goes, moves or stops being static.
//!
//! A frozen piece's press on standing construction stays as its sleeping press did
//! (`contact_transfer`, resting contacts).
//!
//! ORDER. Avian finds a switched body's new pairs, and so builds and solves its contacts, in the
//! order the bodies switched (`collider_tree/update.rs`, `MovedProxies`: insertion order). Freeze
//! takes islands in Avian's island order and both switch their pieces in entity order, which a
//! run's history fixes; a hash set's order follows the entities' bits, which can differ between
//! runs of the same match.
//!
//! ADR-0014 EXCEPTION (owner ruling 2026-11-10: settled rubble may rejoin the fixed structure where
//! that is faster and simpler). A piece's body type and [`Frozen`] change after spawn. Both are
//! server state alone: no client receives a piece's body (`net::debris_stress`, carriers), and
//! nothing here rolls back.

use avian3d::dynamics::solver::islands::{BodyIslandNode, IslandId, PhysicsIslands};
use avian3d::prelude::*;
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

use super::DebrisPiece;
use crate::cost::stages::{Stages, Work};

/// Resolution: whole pieces switched to static per tick, so islands that settle together spread
/// their switches over ticks; one island of any size passes alone, so none waits on its own size.
const FREEZE_PIECE_BUDGET: usize = 8;

/// Resolution: steps of a mover's travel at its present speed its reach covers: the step about to
/// run, and one more for what that step's forces add.
const REACH_TICKS: f32 = 2.0;

/// A frozen piece, in frozen island `island` of [`FrozenIslands`].
#[derive(Component, Debug)]
pub(crate) struct Frozen {
    island: u64,
}

#[cfg(test)]
impl Frozen {
    /// A marker in no island, for a reader's test that never thaws it.
    pub(crate) fn fixture() -> Self {
        Self { island: u64::MAX }
    }
}

/// A frozen island: its pieces, in entity order, their colliders, and the colliders they touched
/// as it froze.
struct Island {
    pieces: Vec<Entity>,
    own: Vec<Entity>,
    touched: Vec<Entity>,
}

/// Every frozen island, the colliders each depends on, and the changes to those colliders since
/// the last thaw.
#[derive(Resource, Default)]
pub(crate) struct FrozenIslands {
    next: u64,
    islands: BTreeMap<u64, Island>,
    by_collider: EntityHashMap<Vec<u64>>,
    /// A collider a frozen island depends on, with the body it was attached to before a change
    /// to its attachment or to that body; `None` once it goes or is disabled.
    changed: Vec<(Entity, Option<Entity>)>,
}

impl FrozenIslands {
    fn depends_on(&self, collider: Entity) -> bool {
        self.by_collider.contains_key(&collider)
    }
}

#[cfg(test)]
impl FrozenIslands {
    /// Each frozen island whose support no longer stands, and why: it touched nothing as it froze,
    /// or a collider it touched has gone, is disabled or is no fixed body's.
    pub(crate) fn unsupported(&self, world: &World) -> Vec<(u64, &'static str)> {
        let mut lost = Vec::new();
        for (&id, island) in &self.islands {
            if island.touched.is_empty() {
                lost.push((id, "it rests on nothing"));
            }
            for &collider in &island.touched {
                if world.get_entity(collider).is_err() {
                    lost.push((id, "its support is gone"));
                } else if world.get::<ColliderDisabled>(collider).is_some() {
                    lost.push((id, "its support is disabled"));
                } else if world.get::<ColliderOf>(collider).is_some_and(|of| {
                    !world
                        .get::<RigidBody>(of.body)
                        .is_some_and(RigidBody::is_static)
                }) {
                    lost.push((id, "its support is not fixed"));
                }
            }
        }
        lost
    }
}

pub(crate) fn sim_plugin(app: &mut App) {
    app.init_resource::<FrozenIslands>()
        .add_observer(body_changed)
        .add_observer(collider_gone)
        .add_observer(collider_left)
        .add_observer(collider_disabled)
        .add_systems(
            FixedPostUpdate,
            thaw.before(crate::construction::breakage::SweepSet)
                .before(PhysicsSystems::Prepare)
                .run_if(not(resource_exists::<crate::ClientReplica>)),
        )
        .add_systems(
            FixedPostUpdate,
            freeze
                .after(PhysicsSystems::Writeback)
                .after(crate::construction::breakage::SplitSet)
                .before(crate::state::SimPhase::Publish)
                .run_if(not(resource_exists::<crate::ClientReplica>)),
        );
}

/// A body going or changing type: each of its colliders an island depends on is checked at the
/// next thaw, against this body.
fn body_changed(
    changed: On<Discard, RigidBody>,
    colliders: Query<&RigidBodyColliders>,
    mut islands: ResMut<FrozenIslands>,
) {
    let Ok(colliders) = colliders.get(changed.entity) else {
        return; // A body with no collider holds nothing up.
    };
    for collider in colliders.iter() {
        if islands.depends_on(collider) {
            islands.changed.push((collider, Some(changed.entity)));
        }
    }
}

/// A collider going, with a body or without one.
fn collider_gone(gone: On<Discard, ColliderMarker>, mut islands: ResMut<FrozenIslands>) {
    if islands.depends_on(gone.entity) {
        islands.changed.push((gone.entity, None));
    }
}

/// A collider leaving its body, despawned or rehomed. Every `Insert<RigidBody>`, a body type switch
/// included, re-inserts `ColliderOf` on the body's colliders (Avian's
/// `on_collider_body_changed::<RigidBody, true>`), so the thaw checks where the collider is then.
fn collider_left(
    left: On<Discard, ColliderOf>,
    of: Query<&ColliderOf>,
    mut islands: ResMut<FrozenIslands>,
) {
    if islands.depends_on(left.entity) {
        let body = of
            .get(left.entity)
            .expect("a discarded ColliderOf is on its collider in its own Discard observer")
            .body;
        islands.changed.push((left.entity, Some(body)));
    }
}

fn collider_disabled(disabled: On<Add, ColliderDisabled>, mut islands: ResMut<FrozenIslands>) {
    if islands.depends_on(disabled.entity) {
        islands.changed.push((disabled.entity, None));
    }
}

/// The awake bodies that can push this step, and what their reach meets.
#[derive(SystemParam)]
struct Reach<'w, 's> {
    movers: Query<
        'w,
        's,
        (
            &'static RigidBody,
            &'static LinearVelocity,
            &'static AngularVelocity,
            &'static RigidBodyColliders,
        ),
        (Without<Sleeping>, Without<Frozen>),
    >,
    aabbs: Query<'w, 's, &'static ColliderAabb>,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    spatial: SpatialQuery<'w, 's>,
    narrow: Res<'w, NarrowPhaseConfig>,
    unit: Res<'w, PhysicsLengthUnit>,
    fixed: Res<'w, Time<Fixed>>,
}

impl Reach<'_, '_> {
    /// Every body passing `keep` with a collider a mover's reach meets.
    fn reached(&self, mut keep: impl FnMut(Entity) -> bool) -> EntityHashSet {
        let dt = self.fixed.timestep().as_secs_f32();
        let tolerance = self.narrow.contact_tolerance * self.unit.0;
        let mut reached = EntityHashSet::default();
        for (body, linear, angular, colliders) in &self.movers {
            if body.is_static() {
                continue;
            }
            let bounds = colliders
                .iter()
                .filter_map(|collider| self.aabbs.get(collider).ok())
                .fold(ColliderAabb::INVALID, |all, aabb| all.merged(*aabb));
            if bounds.min.cmpgt(bounds.max).any() {
                continue; // No collider bounds yet (module doc).
            }
            let radius = (bounds.max - bounds.min).length() * 0.5;
            let travel = (linear.0.length() + angular.0.length() * radius) * dt * REACH_TICKS;
            self.spatial.aabb_intersections_with_aabb_callback(
                bounds.grow(Vec3::splat(travel + tolerance)),
                |collider| {
                    if let Ok(of) = self.collider_of.get(collider)
                        && keep(of.body)
                    {
                        reached.insert(of.body);
                    }
                    true
                },
            );
        }
        reached
    }
}

#[derive(SystemParam)]
struct Freeze<'w, 's> {
    sleeping: Query<
        'w,
        's,
        (Entity, &'static BodyIslandNode, &'static RigidBodyColliders),
        (With<DebrisPiece>, With<Sleeping>, Without<Frozen>),
    >,
    frozen: Query<'w, 's, (), With<Frozen>>,
    physics: Res<'w, PhysicsIslands>,
    graph: Res<'w, ContactGraph>,
    islands: ResMut<'w, FrozenIslands>,
    reach: Reach<'w, 's>,
    stages: Option<Res<'w, Stages>>,
}

/// Freeze every sleeping island of loose pieces that touches fixed matter and that no mover
/// reaches, within the tick's budget (module doc).
fn freeze(p: Freeze, mut commands: Commands) {
    let Freeze {
        sleeping,
        frozen,
        physics,
        graph,
        mut islands,
        reach,
        stages,
    } = p;
    let mut members: BTreeMap<u32, Vec<Entity>> = BTreeMap::new();
    for (piece, node, _) in &sleeping {
        members.entry(node.island_id().0).or_default().push(piece);
    }
    // A sleeping island with any other body keeps every piece of it.
    members.retain(|&id, pieces| {
        let island = physics
            .get(IslandId(id))
            .expect("a sleeping body's island exists");
        island.body_count() as usize == pieces.len()
    });
    if !members.is_empty() {
        let candidates: EntityHashMap<u32> = members
            .iter()
            .flat_map(|(&id, pieces)| pieces.iter().map(move |&piece| (piece, id)))
            .collect();
        // An island a mover reaches would thaw before the next step.
        for piece in reach.reached(|body| candidates.contains_key(&body)) {
            members.remove(&candidates[&piece]);
        }
    }
    let mut froze = 0;
    for mut pieces in members.into_values() {
        if froze > 0 && froze + pieces.len() > FREEZE_PIECE_BUDGET {
            break;
        }
        pieces.sort_unstable();
        let (mut own, mut touched) = (Vec::new(), Vec::new());
        for &piece in &pieces {
            let (_, _, colliders) = sleeping.get(piece).expect("a sleeping island's piece");
            for collider in colliders.iter() {
                own.push(collider);
                for pair in graph.contact_pairs_with(collider) {
                    if !pair.is_touching() || !pair.generates_constraints() {
                        continue;
                    }
                    let (other, body) = if pair.collider1 == collider {
                        (pair.collider2, pair.body2)
                    } else {
                        (pair.collider1, pair.body1)
                    };
                    if body.is_none_or(|body| pieces.binary_search(&body).is_err()) {
                        assert!(
                            body.is_none_or(|body| !frozen.contains(body)),
                            "frozen islands never touch: a body settling onto a frozen piece \
                             reaches it, and thaws it, first"
                        );
                        touched.push(other);
                    }
                }
            }
        }
        if touched.is_empty() {
            continue; // Nothing held it: no evidence of support.
        }
        touched.sort_unstable();
        touched.dedup();
        froze += pieces.len();
        let id = islands.next;
        islands.next += 1;
        for &collider in own.iter().chain(&touched) {
            islands.by_collider.entry(collider).or_default().push(id);
        }
        for &piece in &pieces {
            // ADR-0014 exception (module doc). Static before `Sleeping` goes, so the wake its
            // removal queues finds the piece outside its island.
            commands
                .entity(piece)
                .insert((
                    RigidBody::Static,
                    LinearVelocity::ZERO,
                    AngularVelocity::ZERO,
                    Frozen { island: id },
                ))
                .remove::<Sleeping>();
        }
        islands.islands.insert(
            id,
            Island {
                pieces,
                own,
                touched,
            },
        );
    }
    if let Some(stages) = stages.as_deref() {
        stages.add(Work::FrozePieces, froze as u64);
    }
}

#[derive(SystemParam)]
struct Thaw<'w, 's> {
    islands: ResMut<'w, FrozenIslands>,
    frozen: Query<'w, 's, &'static Frozen>,
    pushed: Query<
        'w,
        's,
        (
            &'static Frozen,
            &'static LinearVelocity,
            &'static AngularVelocity,
        ),
        Or<(Changed<LinearVelocity>, Changed<AngularVelocity>)>,
    >,
    /// Colliders and bodies re-posed since the last thaw: a child collider's own pose follows its
    /// body's only inside the step.
    moved: Query<'w, 's, (Entity, &'static ColliderOf), Or<(Changed<Position>, Changed<Rotation>)>>,
    moved_bodies: Query<
        'w,
        's,
        (&'static RigidBody, &'static RigidBodyColliders),
        (Without<Frozen>, Or<(Changed<Position>, Changed<Rotation>)>),
    >,
    attached: Query<'w, 's, &'static ColliderOf>,
    disabled: Query<'w, 's, (), With<ColliderDisabled>>,
    bodies: Query<'w, 's, (&'static RigidBody, Has<Frozen>)>,
    reach: Reach<'w, 's>,
    stages: Option<Res<'w, Stages>>,
}

/// Thaw every frozen island reached, pushed or losing support (module doc).
fn thaw(p: Thaw, mut commands: Commands) {
    let Thaw {
        mut islands,
        frozen,
        pushed,
        moved,
        moved_bodies,
        attached,
        disabled,
        bodies,
        reach,
        stages,
    } = p;
    let changed = std::mem::take(&mut islands.changed);
    if islands.islands.is_empty() {
        return;
    }
    let mut lost: Vec<Entity> = changed
        .into_iter()
        .filter(|&(collider, was_on)| {
            let holds = was_on.is_some_and(|body| {
                attached.get(collider).is_ok_and(|of| of.body == body)
                    && bodies.get(body).is_ok_and(|(kind, _)| kind.is_static())
            }) && !disabled.contains(collider);
            !holds
        })
        .map(|(collider, _)| collider)
        .collect();
    let fixed_unfrozen = |body| {
        bodies
            .get(body)
            .is_ok_and(|(kind, frozen)| kind.is_static() && !frozen)
    };
    lost.extend(
        moved
            .iter()
            .filter(|&(collider, of)| islands.depends_on(collider) && fixed_unfrozen(of.body))
            .map(|(collider, _)| collider),
    );
    lost.extend(
        moved_bodies
            .iter()
            .filter(|(kind, _)| kind.is_static())
            .flat_map(|(_, colliders)| colliders.iter())
            .filter(|&collider| islands.depends_on(collider)),
    );
    let mut thawing = BTreeSet::new();
    for collider in &lost {
        thawing.extend(islands.by_collider.get(collider).into_iter().flatten());
    }
    let by_support = thawing.len();
    let mut by_load = 0;
    for (piece, linear, angular) in &pushed {
        if (linear.0 != Vec3::ZERO || angular.0 != Vec3::ZERO) && thawing.insert(piece.island) {
            by_load += 1;
        }
    }
    let mut by_reach = 0;
    for piece in reach.reached(|body| frozen.contains(body)) {
        if thawing.insert(frozen.get(piece).expect("a reached frozen piece").island) {
            by_reach += 1;
        }
    }
    let mut pieces = Vec::new();
    for id in &thawing {
        let island = islands
            .islands
            .remove(id)
            .expect("a thawing island is frozen");
        for collider in island.own.iter().chain(&island.touched) {
            let ids = islands
                .by_collider
                .get_mut(collider)
                .expect("an island's colliders are indexed");
            ids.retain(|other| other != id);
            if ids.is_empty() {
                islands.by_collider.remove(collider);
            }
        }
        // A piece gone since it froze has nothing to thaw.
        pieces.extend(
            island
                .pieces
                .into_iter()
                .filter(|&piece| frozen.contains(piece)),
        );
    }
    pieces.sort_unstable();
    for &piece in &pieces {
        commands
            .entity(piece)
            .remove::<Frozen>()
            .insert(RigidBody::Dynamic);
    }
    if let Some(stages) = stages.as_deref() {
        stages.add(Work::ThawedPieces, pieces.len() as u64);
        stages.add(Work::ThawSupport, by_support as u64);
        stages.add(Work::ThawLoad, by_load as u64);
        stages.add(Work::ThawReach, by_reach as u64);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::Layer;
    use crate::debris::tests::{piece, settling, slab};

    /// Fixture gravity, m/s².
    const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);
    /// The side of [`piece`]'s cube, m (fixture data).
    const SIDE: f32 = 0.4;

    fn is_frozen(app: &App, piece: Entity) -> bool {
        app.world().get::<Frozen>(piece).is_some()
    }

    /// Run until every one of `pieces` is frozen, at most 10 s; the updates it took.
    fn until_frozen(app: &mut App, pieces: &[Entity]) -> u32 {
        1 + (0..640)
            .find(|_| {
                app.update();
                pieces.iter().all(|&piece| is_frozen(app, piece))
            })
            .expect("the pieces came to rest and froze within 10 s")
    }

    /// `n` pieces stacked on a top at `top`, at `x` (fixture data).
    fn stack(app: &mut App, x: f32, top: f32, n: usize) -> Vec<Entity> {
        (0..n)
            .map(|level| piece(app, Vec3::new(x, top + SIDE * (level as f32 + 0.5), 0.0)))
            .collect()
    }

    /// A hovering 0.6 × 0.4 × 0.6 m body at `at`, sliding along +x at `speed` (fixture data).
    fn mover(app: &mut App, at: Vec3, speed: f32) -> EntityWorldMut<'_> {
        app.world_mut().spawn((
            Position(at),
            Rotation::default(),
            RigidBody::Dynamic,
            GravityScale(0.0),
            Collider::cuboid(0.6, 0.4, 0.6),
            ColliderDensity(1000.0),
            LinearVelocity(Vec3::X * speed),
        ))
    }

    /// Every frozen island touched something as it froze, and all of it stands: it exists, is
    /// enabled and is fixed.
    fn assert_supported(app: &App) {
        let world = app.world();
        let lost = world.resource::<FrozenIslands>().unsupported(world);
        assert!(
            lost.is_empty(),
            "a frozen island lost its support: {lost:?}"
        );
    }

    /// A FROZEN PIECE ALWAYS HAS SUPPORT: a stack of two frozen on a slab (the child collider of a
    /// static root) thaws whole in the update after the slab goes, goes with its root, is
    /// disabled, moves with its root, leaves its root for a dynamic body, or its root turns
    /// dynamic, and falls; every update, every frozen island still rests on what it froze on. A
    /// mutant that drops any one support check leaves the stack hanging static where the slab
    /// was. The stack froze as one island: the upper piece, settling, reached the lower one.
    #[test]
    fn a_frozen_piece_always_has_support() {
        type Change = fn(&mut World, Entity, Entity);
        let changes: [(&str, Change); 6] = [
            ("goes", |world, slab, _| {
                world.despawn(slab);
            }),
            ("goes with its root", |world, _, root| {
                world.despawn(root);
            }),
            ("is disabled", |world, slab, _| {
                world.entity_mut(slab).insert(ColliderDisabled);
            }),
            ("moves", |world, _, root| {
                world.get_mut::<Position>(root).unwrap().0 += Vec3::NEG_Y;
            }),
            ("leaves its root", |world, slab, root| {
                let at = *world.get::<Position>(root).unwrap();
                let body = world
                    .spawn((Transform::from_translation(at.0), at, RigidBody::Dynamic))
                    .id();
                world.entity_mut(slab).insert(ChildOf(body));
            }),
            ("turns dynamic", |world, _, root| {
                world.entity_mut(root).insert(RigidBody::Dynamic);
            }),
        ];
        for (change, apply) in changes {
            let mut app = settling(GRAVITY);
            let at = Vec3::Y * 0.5;
            let root = app
                .world_mut()
                .spawn((
                    Transform::from_translation(at),
                    Position(at),
                    RigidBody::Static,
                ))
                .id();
            let support = app
                .world_mut()
                .spawn((
                    ChildOf(root),
                    Transform::IDENTITY,
                    Collider::cuboid(20.0, 1.0, 20.0),
                    CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
                ))
                .id();
            let pieces = stack(&mut app, 0.0, 1.0, 2);
            until_frozen(&mut app, &pieces);
            assert_supported(&app);
            let island = |app: &App, piece| app.world().get::<Frozen>(piece).unwrap().island;
            assert_eq!(island(&app, pieces[0]), island(&app, pieces[1]));
            let top = app.world().get::<Position>(pieces[1]).unwrap().0.y;
            apply(app.world_mut(), support, root);
            app.update();
            for &piece in &pieces {
                assert!(!is_frozen(&app, piece), "thawed as its support {change}");
                assert!(app.world().get::<RigidBody>(piece).unwrap().is_dynamic());
            }
            for _ in 0..16 {
                app.update();
                assert_supported(&app);
            }
            assert!(
                app.world().get::<Position>(pieces[1]).unwrap().0.y < top - 0.05,
                "the stack falls as its support {change}"
            );
        }
    }

    /// A PIECE WITHIN REACH OF A MOVING BODY IS AWAKE BEFORE CONTACT: a hovering body at rest
    /// beside a piece frozen on the ground, a tank's stand-in (no piece) or a loose piece, pushed
    /// towards it slow or fast, finds it dynamic in every step that solves a contact between them,
    /// moving in the first update the two touch, and pushes it. A contact with a static piece
    /// meets infinite mass. The gap is inside the margin Avian's broad phase pairs bounds by, so
    /// the step after the push makes speculative contacts at the new speed: a mutant that drops
    /// the reach thaw, or the reach's travel, reds this.
    #[test]
    fn a_piece_within_reach_of_a_moving_body_is_awake_before_contact() {
        // Fixture: a 3 cm gap, between Avian's 5 mm contact tolerance and its 5 cm tree margin;
        // a slow push, and one whose step travels past the piece's whole side.
        let gap = 0.03;
        for speed in [4.0, 40.0] {
            for mover_is_a_piece in [false, true] {
                let case = format!("{speed} m/s, mover a piece: {mover_is_a_piece}");
                let mut app = settling(GRAVITY);
                slab(&mut app, 0.0);
                let rubble = piece(&mut app, Vec3::Y * (SIDE / 2.0));
                until_frozen(&mut app, &[rubble]);
                let start = app.world().get::<Position>(rubble).unwrap().0;
                let at = Vec3::new(-(SIDE / 2.0 + gap + 0.3), 0.45, 0.0);
                let mut body = mover(&mut app, at, 0.0);
                if mover_is_a_piece {
                    body.insert((
                        DebrisPiece,
                        CollisionLayers::new([Layer::Debris], LayerMask::ALL),
                    ));
                } else {
                    body.insert(CollisionLayers::new([Layer::Vehicle], LayerMask::ALL));
                }
                let body = body.id();
                app.update();
                app.update();
                assert!(
                    is_frozen(&app, rubble),
                    "a body at rest beside it reaches nothing"
                );
                app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = Vec3::X * speed;
                let first_touch = (0..64).find(|update| {
                    app.update();
                    let world = app.world();
                    let pair = world.resource::<ContactGraph>().get(body, rubble);
                    if pair.is_some_and(|(_, pair)| !pair.manifolds.is_empty()) {
                        assert!(
                            world.get::<RigidBody>(rubble).unwrap().is_dynamic(),
                            "update {update}: the step met the rubble static ({case})"
                        );
                    }
                    pair.is_some_and(|(edge, _)| edge.is_touching())
                });
                assert!(
                    first_touch.is_some(),
                    "the mover reached the rubble ({case})"
                );
                assert!(
                    app.world().get::<LinearVelocity>(rubble).unwrap().0.x > 0.0,
                    "the first touch moved it ({case})"
                );
                for _ in 0..32 {
                    app.update();
                }
                assert!(
                    app.world().get::<Position>(rubble).unwrap().0.x - start.x > 0.05,
                    "the rubble was pushed ({case})"
                );
            }
        }
    }

    /// A THAWED STACK RESUMES WITH THE ENGINE'S REST TIMER RESTARTED: a stack frozen on a slab,
    /// given an impulse through Avian's `Forces` worth a tenth of the rest speed, thaws whole in
    /// the next update and freezes again only after resting a full `TimeToSleep`. The tripwire on
    /// Avian zeroing the timer of a body that rejoins an island (module doc): a timer carried from
    /// before the freeze would let the island sleep and freeze in the next step. A field's push
    /// on a frozen piece (`construction::breakage`) relies on the impulse reaching its velocity.
    #[test]
    fn a_thawed_stack_rests_a_full_window_before_it_freezes_again() {
        let mut app = settling(GRAVITY);
        slab(&mut app, 0.0);
        let pieces = stack(&mut app, 0.0, 0.0, 2);
        until_frozen(&mut app, &pieces);
        let pushed = pieces[1];
        let rest = app.world().get::<SleepThreshold>(pushed).unwrap().linear
            * app.world().resource::<PhysicsLengthUnit>().0;
        let mass = app.world().get::<ComputedMass>(pushed).unwrap().value();
        let impulse = Vec3::X * 0.1 * rest * mass;
        app.world_mut()
            .run_system_once(move |mut forces: Query<Forces>| {
                forces
                    .get_mut(pushed)
                    .unwrap()
                    .apply_linear_impulse(impulse);
            })
            .unwrap();
        app.update();
        for &piece in &pieces {
            assert!(!is_frozen(&app, piece), "the whole island thawed");
        }
        let dt = app
            .world()
            .resource::<Time<Fixed>>()
            .timestep()
            .as_secs_f32();
        let window = app.world().resource::<TimeToSleep>().0;
        // The thaw's own update ran the first step of rest.
        let rested = (1 + until_frozen(&mut app, &pieces)) as f32 * dt;
        assert!(
            rested >= window,
            "froze again after {rested} s, inside Avian's {window} s rest window"
        );
    }

    /// Three stacks of three side by side, frozen on a slab as one island, then a mover sliding
    /// into them: every piece's pose bits and whether it is frozen, every update.
    fn freeze_thaw_trace(app: &mut App) -> Vec<(Vec<[u32; 7]>, Vec<bool>)> {
        slab(app, 0.0);
        let pieces: Vec<_> = [-SIDE, 0.0, SIDE]
            .into_iter()
            .flat_map(|x| stack(app, x, 0.0, 3))
            .collect();
        until_frozen(app, &pieces);
        mover(app, Vec3::new(-3.0, SIDE * 0.75, 0.0), 3.0)
            .insert(CollisionLayers::new([Layer::Vehicle], LayerMask::ALL));
        (0..320)
            .map(|_| {
                app.update();
                let world = app.world();
                let poses = pieces
                    .iter()
                    .map(|&piece| {
                        let p = world.get::<Position>(piece).unwrap().0;
                        let r = world.get::<Rotation>(piece).unwrap().0;
                        [p.x, p.y, p.z, r.x, r.y, r.z, r.w].map(f32::to_bits)
                    })
                    .collect();
                let frozen = pieces.iter().map(|&piece| is_frozen(app, piece)).collect();
                (poses, frozen)
            })
            .collect()
    }

    /// FREEZE AND THAW ARE DETERMINISTIC IN ORDER: the same scene, built once as is and again
    /// after other entities shift every entity's bits while keeping their order, repeats every
    /// pose bit and every freeze and thaw. Avian's work after a switch follows the order the
    /// bodies switched (module doc); a thaw that switched pieces in a hash set's order would
    /// follow the bits.
    #[test]
    fn freeze_and_thaw_are_deterministic_in_order() {
        let trace = |shift: usize| {
            let mut app = settling(GRAVITY);
            app.world_mut()
                .spawn_batch((0..shift).map(|_| Name::new("shift")));
            freeze_thaw_trace(&mut app)
        };
        let plain = trace(0);
        assert!(
            plain.iter().any(|(_, frozen)| frozen.contains(&false)),
            "the mover thawed the fixture"
        );
        // Fixture shifts.
        for shift in [333, 1000] {
            for (update, (a, b)) in plain.iter().zip(&trace(shift)).enumerate() {
                assert_eq!(
                    a, b,
                    "shift {shift}, update {update} after the mover's spawn"
                );
            }
        }
    }
}
