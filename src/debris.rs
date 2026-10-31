//! Server-simulated pieces at rest, in the game's environment destruction (game state only): the
//! one rule every loose piece follows, a building's fallen chunks ([`crate::buildings::breakage`])
//! and the debris stress lever's fixture fragments (`net::debris_stress`) alike. The authority
//! alone simulates a piece; a replica draws it at the pose the network carries. A piece is a body
//! with [`DebrisPiece`]; its colliders are the body's own ([`RigidBodyColliders`]).
//!
//! REST IS AVIAN'S ISLAND SLEEP. A piece is a dynamic body all match; nothing here changes its
//! body type. Avian 0.7 links dynamic bodies through touching contacts into islands and puts an
//! island to sleep once every member has stayed under its `SleepThreshold` for `TimeToSleep`
//! (0.15 m/s, 0.15 rad/s and 0.5 s, avian's defaults, ASSUMED to suit a piece). A sleeping body
//! leaves the solver, its contact pairs move to the sleeping set the narrow phase skips, and it
//! keeps its mass, so what touches it next meets finite mass. An awake body that starts touching a
//! sleeping one wakes its whole island (`narrow_phase/system_param.rs`), as does a body or a body's
//! collider of that island going (`narrow_phase/mod.rs`, `remove_body_on`, `remove_collider_on`).
//! An island sleeps whole: a piece touching an awake body that is no piece (a driven tank, which
//! applies forces every tick) shares its island and stays awake while it touches, with every
//! piece that touches it in turn.
//!
//! THE ONE WAKE AVIAN LACKS ([`wake_on_collider_gone`]): avian wakes no body when a collider it
//! touches goes without a dynamic body of its own (`remove_collider`'s TODO in its narrow phase),
//! so a building chunk turned to dust under a sleeping piece would leave the piece hanging. Every
//! sleeping piece touching a collider as it goes wakes. A piece stays all match: nothing here
//! despawns one.

use avian3d::prelude::*;
use bevy::prelude::*;

/// A piece body under the rule: its release order, numbered as it is added. A piece spawns
/// dynamic with it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct DebrisPiece {
    pub(crate) seq: u64,
}

/// Number each piece as it is added, in release order.
fn number(
    added: On<Add, DebrisPiece>,
    mut released: Local<u64>,
    mut pieces: Query<&mut DebrisPiece>,
) {
    if let Ok(mut piece) = pieces.get_mut(added.entity) {
        piece.seq = *released;
        *released += 1;
    }
}

/// A collider about to go: every sleeping piece it touches wakes. `Discard` runs before avian's
/// own `Remove` observer drops the collider's contact pairs, so they are still there to read.
fn wake_on_collider_gone(
    gone: On<Discard, ColliderMarker>,
    contacts: Res<ContactGraph>,
    sleeping: Query<(), (With<DebrisPiece>, With<Sleeping>)>,
    mut commands: Commands,
) {
    for edge in contacts.contact_edges_with(gone.entity) {
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

/// Mount the rule.
pub(crate) fn sim_plugin(app: &mut App) {
    app.add_observer(number).add_observer(wake_on_collider_gone);
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

    /// Run until `piece` sleeps, at most 10 s; the tick it fell asleep.
    fn until_asleep(app: &mut App, piece: Entity) -> u32 {
        (0..640)
            .find(|_| {
                app.update();
                app.world().get::<Sleeping>(piece).is_some()
            })
            .expect("the piece came to rest and slept within 10 s")
    }

    /// A PIECE AT REST SLEEPS, A DYNAMIC BODY STILL: a piece dropped from 3 m onto the ground falls
    /// asleep there, never in the air, and keeps `RigidBody::Dynamic`. The server's per-tick cost
    /// of settled rubble rests on it: a mutant that disables sleeping on pieces leaves every one
    /// in the solver for the match, and one that switches a resting piece's body type brings back
    /// the rebuild of its contacts and island on every rest and every wake.
    #[test]
    fn a_piece_at_rest_sleeps_a_dynamic_body_still() {
        let mut app = settling(Vec3::NEG_Y * 9.81);
        slab(&mut app, 0.0);
        let falling = piece(&mut app, Vec3::Y * 3.0);
        let tick = until_asleep(&mut app, falling);
        let height = app.world().get::<Position>(falling).unwrap().0.y;
        assert!(
            height < 0.25,
            "asleep at tick {tick} at {height} m, on the ground"
        );
        assert_eq!(
            app.world().get::<RigidBody>(falling),
            Some(&RigidBody::Dynamic)
        );
        for _ in 0..64 {
            app.update();
        }
        assert!(
            app.world().get::<Sleeping>(falling).is_some(),
            "it stays asleep"
        );
    }

    /// A SLEEPING PIECE WAKES WHEN WHAT IT RESTS ON GOES: a piece asleep on a static slab is awake
    /// in the tick after the slab despawns (a chunk turned to dust under it), and falls. Avian
    /// wakes nothing for a static collider's removal, so a mutant that drops
    /// [`wake_on_collider_gone`] leaves the piece hanging where the slab was.
    #[test]
    fn a_sleeping_piece_wakes_when_its_support_goes() {
        let mut app = settling(Vec3::NEG_Y * 9.81);
        let support = slab(&mut app, 1.0);
        let resting = piece(&mut app, Vec3::Y * 1.25);
        until_asleep(&mut app, resting);
        app.world_mut().entity_mut(support).despawn();
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

    /// A BODY THAT TRULY DISTURBS A SLEEPING PIECE WAKES IT AND MEETS FINITE MASS: a hovering body
    /// sliding into a piece asleep on the ground, a tank's stand-in (no piece) or a thrown piece,
    /// finds it awake and moving in the first tick the two touch, and pushes it. A rule that rested
    /// pieces by making them static would meet the first contact with infinite mass; one that woke
    /// only for a tank would leave a thrown piece bouncing off a fixed block.
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
                    DebrisPiece::default(),
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

    /// A PIECE RESTING ON AN AWAKE BODY STAYS AWAKE: a piece on a body that never sleeps (a driven
    /// tank's stand-in) shares its island and never sleeps, so it stays mobile with what carries
    /// it. A rule that slept pieces one by one would leave it hanging when its carrier moves off.
    #[test]
    fn a_piece_on_an_awake_body_stays_awake() {
        let mut app = settling(Vec3::NEG_Y * 9.81);
        slab(&mut app, 0.0);
        app.world_mut().spawn((
            Position(Vec3::Y * 0.5),
            Rotation::default(),
            RigidBody::Dynamic,
            SleepingDisabled,
            Collider::cuboid(3.0, 1.0, 3.0),
            ColliderDensity(1000.0),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
        ));
        let riding = piece(&mut app, Vec3::Y * 1.25);
        for _ in 0..320 {
            app.update();
            assert!(app.world().get::<Sleeping>(riding).is_none());
        }
        assert!(
            app.world().get::<Position>(riding).unwrap().0.y > 1.1,
            "it rests on the carrier"
        );
    }
}
