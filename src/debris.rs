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
//! THE ONE WAKE AVIAN LACKS ([`wake_on_collider_gone`]): avian wakes no body when a collider it
//! touches goes without a dynamic body of its own (the TODO at `collision/narrow_phase/mod.rs:414`),
//! so a building chunk turned to dust under a sleeping piece would leave the piece hanging. Every
//! sleeping piece touching a collider as it goes wakes. A piece stays all match: nothing here
//! despawns one.
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
    app.add_observer(wake_on_collider_gone);
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
