//! Server-simulated pieces at rest, in the game's environment destruction (game state only): the
//! one rule every loose piece follows, a building's fallen chunks ([`crate::buildings::breakage`])
//! and the debris stress lever's fixture fragments (`net::debris_stress`) alike. The authority
//! alone simulates a piece; a replica draws it at the pose the network carries. A piece is a body
//! with [`DebrisPiece`]; its colliders are the body's own ([`RigidBodyColliders`]). Paths below
//! are under `vendor/avian3d-0.7.0-child-pose/src/`.
//!
//! REST IS AVIAN'S ISLAND SLEEP. A piece is a dynamic body all match; nothing here changes its
//! body type. Avian 0.7 links dynamic bodies through touching contacts into islands and puts an
//! island to sleep once every member has stayed under its `SleepThreshold` for `TimeToSleep`
//! (0.15 m/s, 0.15 rad/s and 0.5 s, avian's defaults, ASSUMED to suit a piece). A sleeping body
//! leaves the solver and keeps its mass, so what touches it next meets finite mass. Only its
//! touching contact pairs move to the sleeping set the narrow phase skips
//! (`collision/contact_types/contact_graph.rs:797`); a pair that overlaps in bounds without
//! touching stays active and is recomputed every tick (`collision/narrow_phase/system_param.rs:480`).
//! An awake body that starts touching a sleeping one wakes its whole island
//! (`collision/narrow_phase/system_param.rs:247-261`), as does a body or a body's collider of that
//! island going (`collision/narrow_phase/mod.rs:484,528`). An island sleeps whole: a piece touching
//! an awake body that is no piece (a driven tank) shares its island and stays awake while it
//! touches, with every piece that touches it in turn. A tank never sleeps (DERIVED): a non-zero
//! force call resets the sleep timer (`dynamics/rigid_body/forces/query_data.rs:345,687`) and the
//! track code applies forces every tick (`track::sim::apply_track_forces`).
//!
//! THE WAKES AVIAN LACKS. Avian wakes no body when a support with no dynamic body of its own goes,
//! is disabled or moves (the TODO in `collision/narrow_phase/mod.rs`, `remove_collider`), so a
//! building chunk turned to dust under a sleeping piece would leave it hanging. The rule wakes
//! every sleeping body, a piece or any other (a launched turret), that touches a support as it:
//! * goes ([`wake_on_collider_gone`]: `Discard` runs before avian's `Remove` drops the pairs);
//! * goes with its body ([`wake_on_body_gone`]: a static root despawned with child colliders,
//!   whose pairs avian's `Remove<RigidBody>` drops before the children's queued despawn);
//! * is disabled ([`wake_on_collider_disabled`]): avian drops the pairs in an `Add` observer that
//!   Bevy may run before this one, so the rule reads bounds instead: every sleeping body with a
//!   collider whose bounds come within avian's contact tolerance of the support's;
//! * moves ([`wake_on_static_moved`]: a static body's collider re-posed, by its own [`Position`]
//!   or [`Rotation`] or by its body's, as a falling tree's trunk is), read inside the step before
//!   the broad phase, while the last step's pairs still stand.
//!
//! A piece stays all match: nothing here despawns one.
//!
//! HOW MANY PIECES (DERIVED from `buildings::breakage`): a chunk leaves its building at most once
//! and a loose piece never splits, so the pieces of a map never exceed its breakable chunks, and
//! nothing else limits them. The shipped map's bound is 74,370 (MEASURED: 1,005 brick chunks in
//! `assets/buildings/house_1/house_1.building.ron`, the elements whose material states a seam, times
//! the 74 `house_proxy` placements in `assets/maps/kalinovo/level.json`; wood chunks never break).
//! Awake pieces are the ones in the solver. A sleeping piece's own per-tick cost and an awake pile
//! held by a tank's island (above) are both unpriced.

use avian3d::prelude::*;
use bevy::prelude::*;

/// A piece body under the rule. A piece spawns dynamic with it.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct DebrisPiece;

/// Wake every sleeping body in a touching pair with `collider` in avian's contact graph: removing
/// `Sleeping` wakes its island (avian's `wake_on_remove_sleeping` hook).
pub(crate) fn wake_touching(
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

/// Mount the rule.
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

pub(crate) fn sim_plugin(app: &mut App) {
    app.add_observer(wake_on_collider_gone)
        .add_observer(wake_on_body_gone)
        .add_observer(wake_on_collider_disabled)
        .add_systems(
            PhysicsSchedule,
            wake_on_static_moved
                .after(PhysicsStepSystems::First)
                .before(PhysicsStepSystems::BroadPhase),
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

    /// A SLEEPING BODY WAKES WHEN ITS SUPPORT IS DISABLED: a body asleep on a static slab wakes
    /// when the slab takes `ColliderDisabled`, and falls through it. Avian drops the slab's pairs
    /// and wakes no other body, so a mutant that drops [`wake_on_collider_disabled`] leaves the
    /// body hanging over a slab it no longer meets.
    #[test]
    fn a_sleeping_body_wakes_when_its_support_moves() {
        let mut app = settling(Vec3::NEG_Y * 9.81);
        let support = slab(&mut app, 1.0);
        let resting = piece(&mut app, Vec3::Y * 1.25);
        until_asleep(&mut app, resting);
        app.world_mut().get_mut::<Position>(support).unwrap().0 += Vec3::NEG_Y;
        wakes_and_falls(&mut app, resting);
    }

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
