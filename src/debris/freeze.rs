//! Settled loose pieces turned static ("frozen") and back ("thawed"), in the game's environment
//! destruction (game state only), on the authority alone. Paths below are under
//! `vendor/avian3d-0.7.0-child-pose/src/`.
//!
//! WHY. A sleeping island keeps every pair that overlaps in bounds without touching active, and
//! the narrow phase recomputes those every tick ([`super`] module doc). The broad phase pairs no
//! two static colliders, so a settled pile turned static leaves the narrow phase nothing to do.
//!
//! FREEZE ([`freeze`], after the step's splits). An island Avian has put to sleep whose every body
//! is a loose piece, that touches something outside itself, that no mover reaches ([`Reach`]) and
//! that nothing has woken since the step, turns static whole in one tick: rubble interlocks, so the
//! island is the unit, and a whole island leaves no piece resting on a dynamic one. A piece's pose
//! or velocity written since the step wakes its island at the next one (`wake_on_changed`), so the
//! freeze leaves it to that wake rather than zero a push. An island holding any other body (a tank, a launched
//! turret) stays asleep, and so does one touching nothing (a piece asleep in zero gravity). Each
//! piece takes [`RigidBody::Static`], zero velocity and [`Frozen`], and loses `Sleeping`. The
//! island depends on its pieces' colliders and on every collider they touched as it froze
//! ([`FrozenIslands`]): Avian's sleep under gravity is the evidence that those held it.
//!
//! FROZEN ISLANDS NEVER TOUCH (the freeze asserts it). Touching bodies that are not static share an
//! Avian island, and Avian makes a contact only in the narrow phase, from the poses and velocities
//! the thaw has just read: a body that comes to touch a frozen piece is a mover whose reach met it,
//! and thawed it, before the step that made the contact. So what an island touches as it freezes
//! is fixed matter.
//!
//! THAW ([`thaw`], in Avian's step after it places child colliders and before its broad phase: the
//! last system before the narrow phase reads poses and velocities, after everything else a tick
//! changes, the sweep included). A frozen island turns dynamic whole. Avian zeroes a body's rest
//! timer as it rejoins an island (`dynamics/solver/islands/sleeping.rs`,
//! `wake_on_enable_rigid_body`), so the island rests a full `TimeToSleep` again before it can sleep
//! and freeze. Three causes:
//! - reach: a mover (an awake body that is not static, and each piece this thaw frees) with a
//!   collider that could make a contact with a frozen piece in this step. Avian makes contacts
//!   once per step, in the narrow phase, from the poses and linear velocities the step starts
//!   with, between colliders closer than `max(dt |Δv|, contact tolerance)` plus their collision
//!   margins (`collision/narrow_phase/system_param.rs`, `update_contacts`). A frozen piece's
//!   bounds hold its shape grown by the tolerance and its collider's margin
//!   (`collider_tree/update.rs`, `update_moved_collider_aabbs`; no piece carries a body margin,
//!   which those bounds would leave out), so a mover collider's shape bounds at its pose, grown by
//!   `dt |v|` and its margin, meet every frozen collider it could touch. Rotation and acceleration
//!   add nothing: they act in the substeps, after the step's contacts are made, and a body they
//!   carry into a frozen piece meets it at the next step's thaw, which reads the pose it reached;
//! - load: game code pushed a frozen piece (Avian's `Forces` writes a static body's velocity as any
//!   other's) and said so where it pushed ([`Loads`]), as a velocity written onto a sleeping body
//!   wakes its island (`dynamics/solver/islands/sleeping.rs`, `wake_on_changed`);
//! - support: a collider the island depends on goes, leaves its body, is disabled, turns sensor,
//!   or is re-posed, reshaped or re-layered in place (`super::wake_on_static_moved` notes those);
//!   or its body goes, is disabled or stops being static. A collider with no body is never a
//!   support: Avian builds it no contact constraint, and the freeze counts only pairs that build
//!   one. A frozen piece's own collider re-posed holds while the piece's pose keeps the bits it
//!   froze at ([`Frozen`]): Avian rewrites a pose with the value it holds (placing child colliders,
//!   syncing transforms), and anything else that writes one moves the piece.
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
use avian3d::schedule::LastPhysicsTick;
use bevy::ecs::change_detection::Tick;
use bevy::ecs::entity::EntityHashMap;
use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::ecs::system::{SystemChangeTick, SystemParam};
use bevy::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

use super::DebrisPiece;
use crate::cost::stages::{Stages, Work};

/// Resolution: whole pieces switched to static per tick, so islands that settle together spread
/// their switches over ticks; one island of any size passes alone, so none waits on its own size.
const FREEZE_PIECE_BUDGET: usize = 8;

/// A frozen piece, in frozen island `island` of [`FrozenIslands`], at the pose bits it froze at.
#[derive(Component, Debug)]
pub(crate) struct Frozen {
    island: u64,
    pose: [u32; 7],
}

#[cfg(test)]
impl Frozen {
    /// A marker in no island, for a reader's test that never thaws it.
    pub(crate) fn fixture() -> Self {
        Self {
            island: u64::MAX,
            pose: [0; 7],
        }
    }
}

fn pose_bits(position: &Position, rotation: &Rotation) -> [u32; 7] {
    let (p, r) = (position.0, rotation.0);
    [p.x, p.y, p.z, r.x, r.y, r.z, r.w].map(f32::to_bits)
}

/// A frozen island: its pieces, in entity order, their colliders, and the colliders they touched
/// as it froze.
struct Island {
    pieces: Vec<Entity>,
    own: Vec<Entity>,
    touched: Vec<Entity>,
}

/// What became of a collider a frozen island depends on, judged at the next thaw.
#[derive(Clone, Copy)]
enum Change {
    /// It went, was disabled or turned sensor, or its body was disabled.
    Lost,
    /// Its attachment, or its body's type, changed from this body.
    Attachment(Entity),
    /// It was re-posed in place, and with `reshaped` reshaped or re-layered too.
    InPlace { reshaped: bool },
}

/// Every frozen island, the colliders each depends on, and what changed for them since the last
/// thaw.
#[derive(Resource, Default)]
pub(crate) struct FrozenIslands {
    next: u64,
    islands: BTreeMap<u64, Island>,
    by_collider: EntityHashMap<Vec<u64>>,
    changed: Vec<(Entity, Change)>,
    /// Islands game code pushed a piece of ([`Loads`]).
    loaded: Vec<u64>,
}

impl FrozenIslands {
    fn note(&mut self, collider: Entity, change: Change) {
        if self.by_collider.contains_key(&collider) {
            self.changed.push((collider, change));
        }
    }

    /// A static body's collider re-posed, and with `reshaped` reshaped or re-layered too, in
    /// place since the last step.
    pub(super) fn moved_in_place(&mut self, collider: Entity, reshaped: bool) {
        self.note(collider, Change::InPlace { reshaped });
    }
}

#[cfg(test)]
impl FrozenIslands {
    /// Each frozen island whose support no longer stands, and why: it touched nothing as it froze,
    /// or a collider it touched has gone, is disabled, is a sensor or is no enabled fixed body's.
    pub(crate) fn unsupported(&self, world: &World) -> Vec<(u64, &'static str)> {
        let disabled = |entity| {
            world.get::<Disabled>(entity).is_some()
                || world.get::<ColliderDisabled>(entity).is_some()
        };
        let mut lost = Vec::new();
        for (&id, island) in &self.islands {
            if island.touched.is_empty() {
                lost.push((id, "it rests on nothing"));
            }
            for &collider in &island.touched {
                if world.get_entity(collider).is_err() {
                    lost.push((id, "its support is gone"));
                } else if disabled(collider) {
                    lost.push((id, "its support is disabled"));
                } else if world.get::<Sensor>(collider).is_some() {
                    lost.push((id, "its support is a sensor"));
                } else if world.get::<ColliderOf>(collider).is_some_and(|of| {
                    disabled(of.body)
                        || world.get::<RigidBodyDisabled>(of.body).is_some()
                        || !world
                            .get::<RigidBody>(of.body)
                            .is_some_and(RigidBody::is_static)
                }) {
                    lost.push((id, "its support is not an enabled fixed body's"));
                }
            }
        }
        lost
    }
}

/// Pushes by game code on bodies that may be frozen: the island of a frozen piece pushed thaws at
/// the next step's thaw. Every game-code velocity write that can meet a frozen piece reports here:
/// the field and blast impulses (`construction::breakage`). The tank's hit impulses and the belt's
/// pushes write tanks and dynamic bodies only.
#[derive(SystemParam)]
pub(crate) struct Loads<'w, 's> {
    frozen: Query<'w, 's, &'static Frozen>,
    islands: ResMut<'w, FrozenIslands>,
}

impl Loads<'_, '_> {
    pub(crate) fn is_frozen(&self, body: Entity) -> bool {
        self.frozen.contains(body)
    }

    /// Game code just wrote `body`'s velocity.
    pub(crate) fn pushed(&mut self, body: Entity) {
        if let Ok(frozen) = self.frozen.get(body) {
            self.islands.loaded.push(frozen.island);
        }
    }
}

pub(crate) fn sim_plugin(app: &mut App) {
    app.init_resource::<FrozenIslands>()
        .add_observer(body_changed)
        .add_observer(body_disabled)
        .add_observer(entity_disabled)
        .add_observer(collider_gone)
        .add_observer(collider_left)
        .add_observer(collider_disabled)
        .add_observer(collider_sensed)
        .add_systems(
            PhysicsSchedule,
            thaw.after(super::wake_on_static_moved)
                .before(PhysicsStepSystems::BroadPhase)
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
    for collider in colliders.get(changed.entity).into_iter().flatten() {
        islands.note(collider, Change::Attachment(changed.entity));
    }
}

/// A body disabled: none of its colliders holds anything up.
fn body_disabled(
    disabled: On<Add, RigidBodyDisabled>,
    colliders: Query<&RigidBodyColliders>,
    mut islands: ResMut<FrozenIslands>,
) {
    for collider in colliders.get(disabled.entity).into_iter().flatten() {
        islands.note(collider, Change::Lost);
    }
}

/// An entity disabled whole: a collider, or a body with its colliders.
fn entity_disabled(
    disabled: On<Add, Disabled>,
    colliders: Query<&RigidBodyColliders, Allow<Disabled>>,
    mut islands: ResMut<FrozenIslands>,
) {
    islands.note(disabled.entity, Change::Lost);
    for collider in colliders.get(disabled.entity).into_iter().flatten() {
        islands.note(collider, Change::Lost);
    }
}

/// A collider going, with a body or without one.
fn collider_gone(gone: On<Discard, ColliderMarker>, mut islands: ResMut<FrozenIslands>) {
    islands.note(gone.entity, Change::Lost);
}

/// A collider leaving its body, despawned or rehomed. Every `Insert<RigidBody>`, a body type switch
/// included, re-inserts `ColliderOf` on the body's colliders (Avian's
/// `on_collider_body_changed::<RigidBody, true>`), so the thaw checks where the collider is then.
fn collider_left(
    left: On<Discard, ColliderOf>,
    of: Query<&ColliderOf>,
    mut islands: ResMut<FrozenIslands>,
) {
    let body = of
        .get(left.entity)
        .expect("a discarded ColliderOf is on its collider in its own Discard observer")
        .body;
    islands.note(left.entity, Change::Attachment(body));
}

fn collider_disabled(disabled: On<Add, ColliderDisabled>, mut islands: ResMut<FrozenIslands>) {
    islands.note(disabled.entity, Change::Lost);
}

/// A sensor holds nothing up.
fn collider_sensed(sensed: On<Add, Sensor>, mut islands: ResMut<FrozenIslands>) {
    islands.note(sensed.entity, Change::Lost);
}

/// The bodies that can make contacts in this step, and what their colliders can touch in it
/// (module doc, reach).
#[derive(SystemParam)]
struct Reach<'w, 's> {
    /// Avian keeps an island node on each enabled body that is not static.
    movers: Query<'w, 's, Entity, (With<BodyIslandNode>, Without<Sleeping>)>,
    bodies: Query<
        'w,
        's,
        (
            &'static LinearVelocity,
            &'static RigidBodyColliders,
            Option<&'static CollisionMargin>,
        ),
    >,
    /// The colliders that make contacts, at their placed poses.
    solids: Query<
        'w,
        's,
        (
            &'static Collider,
            &'static Position,
            &'static Rotation,
            Option<&'static CollisionMargin>,
        ),
        (Without<Sensor>, Without<ColliderDisabled>),
    >,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    spatial: SpatialQuery<'w, 's>,
    /// The coming step's: Avian's own clock in its step, the fixed clock after it.
    time: Res<'w, Time>,
}

impl Reach<'_, '_> {
    /// The awake bodies that are not static, in entity order.
    fn movers(&self) -> Vec<Entity> {
        let mut movers: Vec<_> = self.movers.iter().collect();
        movers.sort_unstable();
        movers
    }

    /// Add to `reached` each body passing `keep` with a solid collider that a solid collider of
    /// `body` could make a contact with in this step (module doc, reach); the spatial queries it
    /// cast.
    fn of(
        &self,
        body: Entity,
        keep: impl Fn(Entity) -> bool,
        reached: &mut BTreeSet<Entity>,
    ) -> u64 {
        let Ok((linear, colliders, body_margin)) = self.bodies.get(body) else {
            return 0; // A body with no collider touches nothing.
        };
        let mut queries = 0;
        let travel = linear.length() * self.time.delta_secs();
        for collider in colliders.iter() {
            let Ok((shape, at, turn, margin)) = self.solids.get(collider) else {
                continue; // A sensor or a disabled collider makes no contact.
            };
            let margin = margin.or(body_margin).map_or(0.0, |margin| margin.0);
            let bounds = shape.aabb(at.0, *turn).grow(Vec3::splat(travel + margin));
            queries += 1;
            self.spatial
                .aabb_intersections_with_aabb_callback(bounds, |hit| {
                    if self.solids.contains(hit)
                        && let Ok(of) = self.collider_of.get(hit)
                        && keep(of.body)
                    {
                        reached.insert(of.body);
                    }
                    true
                });
        }
        queries
    }
}

#[derive(SystemParam)]
struct Freeze<'w, 's> {
    sleeping: Query<
        'w,
        's,
        (
            Entity,
            &'static BodyIslandNode,
            &'static RigidBodyColliders,
            (Ref<'static, Position>, Ref<'static, Rotation>),
            (Ref<'static, LinearVelocity>, Ref<'static, AngularVelocity>),
        ),
        (With<DebrisPiece>, With<Sleeping>, Without<Frozen>),
    >,
    frozen: Query<'w, 's, (), With<Frozen>>,
    physics: Res<'w, PhysicsIslands>,
    graph: Res<'w, ContactGraph>,
    islands: ResMut<'w, FrozenIslands>,
    reach: Reach<'w, 's>,
    last_step: Res<'w, LastPhysicsTick>,
    tick: SystemChangeTick,
    stages: Option<Res<'w, Stages>>,
}

/// Freeze every sleeping island of loose pieces that touches fixed matter, that no mover reaches
/// and that nothing has woken since the step, within the tick's budget (module doc).
fn freeze(p: Freeze, mut commands: Commands) {
    let Freeze {
        sleeping,
        frozen,
        physics,
        graph,
        mut islands,
        reach,
        last_step,
        tick,
        stages,
    } = p;
    let since_step = |at: Tick| at.is_newer_than(last_step.0, tick.this_run());
    let mut members: BTreeMap<u32, Vec<Entity>> = BTreeMap::new();
    let mut woken = BTreeSet::new();
    let mut visits = 0;
    for (piece, node, _, (at, turn), (linear, angular)) in &sleeping {
        visits += 1;
        let id = node.island_id().0;
        members.entry(id).or_default().push(piece);
        let written = [
            at.last_changed(),
            turn.last_changed(),
            linear.last_changed(),
            angular.last_changed(),
        ];
        if written.into_iter().any(since_step) {
            woken.insert(id);
        }
    }
    // A sleeping island with any other body keeps every piece of it.
    members.retain(|&id, pieces| {
        let island = physics
            .get(IslandId(id))
            .expect("a sleeping body's island exists");
        !woken.contains(&id) && island.body_count() as usize == pieces.len()
    });
    let mut queries = 0;
    if !members.is_empty() {
        let candidates: EntityHashMap<u32> = members
            .iter()
            .flat_map(|(&id, pieces)| pieces.iter().map(move |&piece| (piece, id)))
            .collect();
        // An island a mover reaches would thaw at the next step.
        let mut reached = BTreeSet::new();
        for mover in reach.movers() {
            visits += 1;
            queries += reach.of(mover, |body| candidates.contains_key(&body), &mut reached);
        }
        for piece in reached {
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
            let (_, _, colliders, ..) = sleeping.get(piece).expect("a sleeping island's piece");
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
                            "frozen islands never touch (module doc)"
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
            let (.., (at, turn), _) = sleeping.get(piece).expect("a sleeping island's piece");
            // ADR-0014 exception (module doc). Static before `Sleeping` goes, so the wake its
            // removal queues finds the piece outside its island.
            commands
                .entity(piece)
                .insert((
                    RigidBody::Static,
                    LinearVelocity::ZERO,
                    AngularVelocity::ZERO,
                    Frozen {
                        island: id,
                        pose: pose_bits(&at, &turn),
                    },
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
        stages.add(Work::FreezeVisits, visits);
        stages.add(Work::ReachQueries, queries);
    }
}

#[derive(SystemParam)]
struct Thaw<'w, 's> {
    islands: ResMut<'w, FrozenIslands>,
    frozen: Query<'w, 's, (&'static Frozen, &'static Position, &'static Rotation)>,
    attached: Query<'w, 's, &'static ColliderOf>,
    disabled: Query<'w, 's, (), With<ColliderDisabled>>,
    bodies: Query<'w, 's, &'static RigidBody, Without<RigidBodyDisabled>>,
    reach: Reach<'w, 's>,
    stages: Option<Res<'w, Stages>>,
}

/// Thaw every frozen island losing support, pushed or reached (module doc).
fn thaw(p: Thaw, mut commands: Commands) {
    let Thaw {
        mut islands,
        frozen,
        attached,
        disabled,
        bodies,
        reach,
        stages,
    } = p;
    let changed = std::mem::take(&mut islands.changed);
    let loaded = std::mem::take(&mut islands.loaded);
    if islands.islands.is_empty() {
        return;
    }
    let (mut visits, mut queries) = ((changed.len() + loaded.len()) as u64, 0);
    let holds = |collider: Entity, change: Change| match change {
        Change::Lost => false,
        Change::Attachment(body) => {
            attached.get(collider).is_ok_and(|of| of.body == body)
                && bodies.get(body).is_ok_and(RigidBody::is_static)
                && !disabled.contains(collider)
        }
        // A frozen piece's own collider holds while the piece's pose bits do (module doc).
        Change::InPlace { reshaped } => {
            !reshaped
                && attached.get(collider).is_ok_and(|of| {
                    frozen
                        .get(of.body)
                        .is_ok_and(|(piece, at, turn)| piece.pose == pose_bits(at, turn))
                })
        }
    };
    let mut thawing = BTreeSet::new();
    for (collider, change) in changed {
        if !holds(collider, change) {
            thawing.extend(islands.by_collider.get(&collider).into_iter().flatten());
        }
    }
    let by_support = thawing.len();
    let mut by_load = 0;
    for island in loaded {
        // A push on an island an earlier cause in this thaw already frees counts once.
        if thawing.insert(island) {
            by_load += 1;
        }
    }
    let island_of = |piece: Entity| frozen.get(piece).expect("a reached frozen piece").0.island;
    let mut reached = BTreeSet::new();
    for mover in reach.movers() {
        visits += 1;
        queries += reach.of(mover, |body| frozen.contains(body), &mut reached);
    }
    let mut by_reach = 0;
    for piece in reached {
        if thawing.insert(island_of(piece)) {
            by_reach += 1;
        }
    }
    // Each piece freed is a mover in this step too, with whatever velocity a push gave it.
    let mut releasing: Vec<u64> = thawing.iter().copied().collect();
    let mut pieces = Vec::new();
    while let Some(id) = releasing.pop() {
        let island = islands
            .islands
            .remove(&id)
            .expect("a thawing island is frozen");
        for collider in island.own.iter().chain(&island.touched) {
            let ids = islands
                .by_collider
                .get_mut(collider)
                .expect("an island's colliders are indexed");
            ids.retain(|other| *other != id);
            if ids.is_empty() {
                islands.by_collider.remove(collider);
            }
        }
        // A piece gone since it froze has nothing to thaw.
        let freed: Vec<_> = island
            .pieces
            .into_iter()
            .filter(|&piece| frozen.contains(piece))
            .collect();
        let mut further = BTreeSet::new();
        for &piece in &freed {
            visits += 1;
            queries += reach.of(
                piece,
                |body| {
                    frozen
                        .get(body)
                        .is_ok_and(|(other, ..)| !thawing.contains(&other.island))
                },
                &mut further,
            );
        }
        for piece in further {
            let id = island_of(piece);
            if thawing.insert(id) {
                by_reach += 1;
                releasing.push(id);
            }
        }
        pieces.extend(freed);
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
        stages.add(Work::FreezeVisits, visits);
        stages.add(Work::ReachQueries, queries);
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
    /// disabled, turns sensor, is reshaped or re-layered in place, moves with its root, leaves its
    /// root for a dynamic body, or its root turns dynamic or is disabled, and falls; every update,
    /// every frozen island still rests on what it froze on. A mutant that drops any one support
    /// check leaves the stack hanging static where the slab was. The stack froze as one island:
    /// the upper piece, settling, reached the lower one.
    #[test]
    fn a_frozen_piece_always_has_support() {
        type Change = fn(&mut World, Entity, Entity);
        let changes: [(&str, Change); 12] = [
            ("goes", |world, slab, _| {
                world.despawn(slab);
            }),
            ("goes with its root", |world, _, root| {
                world.despawn(root);
            }),
            ("is disabled", |world, slab, _| {
                world.entity_mut(slab).insert(ColliderDisabled);
            }),
            ("is disabled whole", |world, slab, _| {
                world.entity_mut(slab).insert(Disabled);
            }),
            ("turns sensor", |world, slab, _| {
                world.entity_mut(slab).insert(Sensor);
            }),
            ("is reshaped", |world, slab, _| {
                *world.get_mut::<Collider>(slab).unwrap() = Collider::cuboid(20.0, 0.2, 20.0);
            }),
            ("is re-layered", |world, slab, _| {
                world
                    .entity_mut(slab)
                    .insert(CollisionLayers::new([Layer::Terrain], LayerMask::NONE));
            }),
            ("moves with its root", |world, _, root| {
                world.get_mut::<Position>(root).unwrap().0 += Vec3::NEG_Y;
            }),
            ("leaves its root", |world, slab, root| {
                let at = *world.get::<Position>(root).unwrap();
                let body = world
                    .spawn((Transform::from_translation(at.0), at, RigidBody::Dynamic))
                    .id();
                world.entity_mut(slab).insert(ChildOf(body));
            }),
            ("its root turns dynamic", |world, _, root| {
                world.entity_mut(root).insert(RigidBody::Dynamic);
            }),
            ("its root is disabled", |world, _, root| {
                world.entity_mut(root).insert(RigidBodyDisabled);
            }),
            ("its root is disabled whole", |world, _, root| {
                world.entity_mut(root).insert(Disabled);
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

    /// A FROZEN PIECE WHOSE POSE IS WRITTEN THAWS, AND ONE WRITTEN WITH ITS OWN BITS STAYS: a
    /// stack frozen on a slab keeps frozen through a write of the pose its upper piece holds, as
    /// Avian's own writes are, and thaws whole in the update after a write that moves that piece
    /// without velocity; the piece put up falls back. A thaw that ignored a frozen piece's own
    /// collider would leave it hanging where it was put; one that took every write for a move
    /// would thaw it on Avian's.
    #[test]
    fn a_frozen_piece_whose_pose_is_written_thaws() {
        let mut app = settling(GRAVITY);
        slab(&mut app, 0.0);
        let pieces = stack(&mut app, 0.0, 0.0, 2);
        until_frozen(&mut app, &pieces);
        let moved = pieces[1];
        let held = *app.world().get::<Position>(moved).unwrap();
        *app.world_mut().get_mut::<Position>(moved).unwrap() = held;
        app.update();
        app.update();
        for &piece in &pieces {
            assert!(
                is_frozen(&app, piece),
                "a write of its own pose moves nothing"
            );
        }
        // Fixture: 0.3 m up, without velocity.
        app.world_mut().get_mut::<Position>(moved).unwrap().0 += Vec3::Y * 0.3;
        app.update();
        for &piece in &pieces {
            assert!(!is_frozen(&app, piece), "the whole island thawed");
        }
        for _ in 0..32 {
            app.update();
        }
        assert!(
            app.world().get::<Position>(moved).unwrap().0.y < held.0.y + 0.25,
            "the piece put up falls back"
        );
    }

    /// A PIECE WITHIN REACH OF A MOVING BODY IS AWAKE BEFORE CONTACT: a hovering body at rest
    /// beside a piece frozen on the ground, a tank's stand-in (no piece) or a loose piece, pushed
    /// towards it slow or fast, finds it dynamic in every step that solves a contact between them,
    /// moving in the first update the two touch, and pushes it. A contact with a static piece
    /// meets infinite mass. The gap is past Avian's contact tolerance and inside the step's travel
    /// at either speed, so the step after the push makes a speculative contact at the new speed: a
    /// mutant that drops the reach thaw, or the reach's travel, reds this.
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

    /// A SPINNING OR ACCELERATING BODY MEETS NO FROZEN PIECE: a long hovering bar spinning about
    /// its centre, its end sweeping through a piece frozen on the ground, and a hovering body
    /// driven from rest at the piece by a constant acceleration, find it dynamic in every step that
    /// solves a contact with it, and move it. Neither rotation nor acceleration widens Avian's
    /// contact margin (module doc, reach): a body they carry into a frozen piece without a contact
    /// meets it at the next step's thaw, which reads the pose it reached. A reach that passed over
    /// a body with no linear velocity (the bar spins in place; the other starts at rest) meets
    /// this static.
    #[test]
    fn a_spinning_or_accelerating_body_meets_no_frozen_piece() {
        // Fixture: a 4 m bar at 15 rad/s, its end 30 m/s across the piece; a 0.6 m body at rest
        // 0.1 m off under 2000 m/s², which its first step carries into the piece with no contact.
        let cases: [(&str, fn(&mut App) -> Entity); 2] = [
            ("a spinning bar", |app| {
                app.world_mut()
                    .spawn((
                        Position(Vec3::new(0.0, SIDE * 0.75, 0.0)),
                        Rotation::default(),
                        RigidBody::Dynamic,
                        GravityScale(0.0),
                        Collider::cuboid(0.2, 0.2, 4.0),
                        ColliderDensity(1000.0),
                        AngularVelocity(Vec3::Y * 15.0),
                        CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
                    ))
                    .id()
            }),
            ("an accelerating body", |app| {
                let at = Vec3::new(1.9 - SIDE / 2.0 - 0.1 - 0.3, SIDE * 0.75, 0.0);
                mover(app, at, 0.0)
                    .insert((
                        ConstantLinearAcceleration(Vec3::X * 2000.0),
                        CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
                    ))
                    .id()
            }),
        ];
        for (case, spawn) in cases {
            let mut app = settling(GRAVITY);
            slab(&mut app, 0.0);
            let rubble = piece(&mut app, Vec3::new(1.9, SIDE / 2.0, 0.0));
            until_frozen(&mut app, &[rubble]);
            let start = app.world().get::<Position>(rubble).unwrap().0;
            let body = spawn(&mut app);
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
            assert!(first_touch.is_some(), "{case} reached the rubble");
            for _ in 0..16 {
                app.update();
            }
            assert!(
                app.world()
                    .get::<Position>(rubble)
                    .unwrap()
                    .0
                    .distance(start)
                    > 0.05,
                "{case} moved the rubble"
            );
        }
    }

    /// A PUSH ON A PIECE FALLING ASLEEP IS KEPT: a piece pushed in the tick Avian puts it to sleep,
    /// after the step and before the freeze (where a field's or a blast's impulse lands), stays
    /// dynamic and moves off, as a push on any sleeping body does. A freeze that took its island
    /// would zero the push.
    #[test]
    fn a_push_on_a_piece_falling_asleep_is_kept() {
        /// The piece, its push (N s), and whether it was pushed.
        #[derive(Resource)]
        struct Push(Entity, Vec3, bool);
        let mut app = settling(GRAVITY);
        slab(&mut app, 0.0);
        let rubble = piece(&mut app, Vec3::Y * (SIDE / 2.0));
        app.update();
        let start = app.world().get::<Position>(rubble).unwrap().0;
        // Fixture: 3 m/s along +x, past what friction stops within the run.
        let impulse = Vec3::X * 3.0 * app.world().get::<ComputedMass>(rubble).unwrap().value();
        app.insert_resource(Push(rubble, impulse, false))
            .add_systems(
                FixedPostUpdate,
                (|mut push: ResMut<Push>,
                  asleep: Query<(), With<Sleeping>>,
                  mut forces: Query<Forces>| {
                    if !push.2 && asleep.contains(push.0) {
                        push.2 = true;
                        forces.get_mut(push.0).unwrap().apply_linear_impulse(push.1);
                    }
                })
                .after(PhysicsSystems::Writeback)
                .before(freeze),
            );
        (0..640)
            .find(|_| {
                app.update();
                app.world().resource::<Push>().2
            })
            .expect("the piece fell asleep within 10 s");
        assert!(!is_frozen(&app, rubble), "the pushed island stays dynamic");
        for _ in 0..16 {
            app.update();
        }
        assert!(
            app.world().get::<Position>(rubble).unwrap().0.x - start.x > 0.05,
            "the push moved it"
        );
    }

    /// A THAWED STACK RESUMES WITH THE ENGINE'S REST TIMER RESTARTED: a stack frozen on a slab,
    /// given an impulse through Avian's `Forces` worth a tenth of the rest speed, thaws whole in
    /// the next update and freezes again only after resting a full `TimeToSleep`. The tripwire on
    /// Avian zeroing the timer of a body that rejoins an island (module doc): a timer carried from
    /// before the freeze would let the island sleep and freeze in the next step. A field's push
    /// on a frozen piece (`construction::breakage`) relies on the impulse reaching its velocity
    /// and on its report through [`Loads`].
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
            .run_system_once(move |mut forces: Query<Forces>, mut loads: Loads| {
                forces
                    .get_mut(pushed)
                    .unwrap()
                    .apply_linear_impulse(impulse);
                loads.pushed(pushed);
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
