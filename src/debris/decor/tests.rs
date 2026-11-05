//! The decoration's invariants, in the game's environment destruction model (game state only).

use super::*;
use crate::construction::tests::shipped_house;

/// A player's drawing root: the decoration over the shipped house placed as construction 1
/// (fixture ordinal), its cursor `overstep` ticks past tick 40 (fixture data).
fn player(overstep: f64) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), plugin))
        .init_asset::<Mesh>()
        .insert_resource(Look {
            surface: Handle::default(),
        })
        .insert_resource(PresentationClock::At { tick: 40, overstep });
    app.world_mut().spawn(Breakage::new(shipped_house(), 1));
    app
}

/// A fragment of the house's first two chunks thrown up and spinning (fixture data).
fn shed() -> Shed {
    Shed {
        construction: 1,
        chunks: vec![0, 1],
        at: [12.0, 3.0, -4.0],
        rotation: Quat::from_rotation_y(0.7).to_array(),
        velocity: [1.5, 4.0, -0.5],
        spin: [0.3, -2.0, 1.1],
        seed: 0xC0FF_EE11,
        tick: 40,
    }
}

/// The one drawn piece's decoration and pose after `app` receives `shed`.
fn drawn(app: &mut App, shed: Shed) -> (Entity, Decoration, Transform) {
    app.world_mut().write_message(shed);
    app.update();
    let mut drawn = app.world_mut().query::<(Entity, &Decoration, &Transform)>();
    let (entity, decoration, transform) = drawn
        .single(app.world())
        .expect("the due shed is drawn once");
    (entity, decoration.clone(), *transform)
}

/// THE SAME EVENT, THE SAME PIECE ON EVERY PLAYER: two players drawing one shed at one cursor draw
/// it at the same pose, bit for bit, and at the event's own tick it stands where the event put
/// it: its chunks' centre in the event's construction frame, turned as that frame. A mutant that
/// seeded from a local clock or started from the parent's eased pose breaks it; the one timeline
/// (ADR-0037) relies on it.
#[test]
fn every_player_draws_the_same_event_alike() {
    let (_, first, at_first) = drawn(&mut player(0.5), shed());
    let (_, second, at_second) = drawn(&mut player(0.5), shed());
    assert_eq!(first, second);
    assert_eq!(at_first, at_second);
    let (_, start, pose) = drawn(&mut player(0.0), shed());
    let frame = Transform::from_translation(Vec3::from_array(shed().at))
        .with_rotation(Quat::from_array(shed().rotation));
    let piece = Piece {
        construction: 1,
        chunks: shed().chunks,
        born: 40,
        retired: None,
    };
    let breakage = Breakage::new(shipped_house(), 1);
    let origin = piece
        .admitted(Some((Entity::PLACEHOLDER, &breakage)))
        .unwrap()
        .origin;
    assert!((pose.translation - frame.transform_point(origin)).length() < 1e-5);
    assert!(pose.rotation.angle_between(frame.rotation) < 1e-5);
    assert_eq!(start.velocity, Vec3::from_array(shed().velocity));
}

/// DECORATION MEETS NOTHING: a drawn piece has no body, no collider and no collision layers, so no
/// hull, round or server body can touch it. A mutant that spawned it as a body would put it in
/// every tank's and shell's way.
#[test]
fn a_drawn_piece_carries_no_collider() {
    let mut app = player(0.0);
    let (entity, ..) = drawn(&mut app, shed());
    let piece = app.world().entity(entity);
    assert!(!piece.contains::<avian3d::prelude::Collider>());
    assert!(!piece.contains::<avian3d::prelude::CollisionLayers>());
    assert!(!piece.contains::<avian3d::prelude::RigidBody>());
}

/// A SHED IS DRAWN ONLY FROM ITS TICK: before the cursor reaches it, nothing shows; the chunks
/// still ride their parent there (the construction view's handover).
#[test]
fn a_shed_waits_for_the_cursor() {
    let mut app = player(0.0);
    app.world_mut().write_message(Shed { tick: 41, ..shed() });
    app.update();
    let mut drawn = app.world_mut().query::<&Decoration>();
    assert_eq!(drawn.iter(app.world()).count(), 0);
    *app.world_mut().resource_mut::<PresentationClock>() = PresentationClock::At {
        tick: 41,
        overstep: 0.0,
    };
    app.update();
    assert_eq!(drawn.iter(app.world()).count(), 1);
}

/// A PIECE RESTS WHERE IT MEETS THE GROUND: dropped onto flat ground, it touches within
/// [`TOUCH_M`] and holds that pose for the rest of its life. A mutant that let it fall through
/// or kept integrating after the touch sinks it into the terrain.
#[test]
fn a_piece_rests_where_it_meets_the_ground() {
    let corners = [Vec3::new(-0.2, -0.1, -0.15), Vec3::new(0.2, 0.1, 0.15)];
    let mut decoration = Decoration {
        tick: 0,
        centre: Vec3::Y * 3.0,
        rotation: Quat::IDENTITY,
        velocity: Vec3::new(1.0, 2.0, 0.0),
        spin: Vec3::new(0.0, 0.0, 2.0),
        lands: None,
        life: 8.0,
    };
    let gravity = Vec3::NEG_Y * 9.81;
    let flat = |_: Vec3| 0.0;
    let lands = landing(&decoration, &corners, gravity, flat).expect("it lands within its life");
    decoration.lands = Some(lands);
    let rest = decoration.flight(lands, gravity);
    let low = clearance(&corners, rest, &flat);
    assert!((-TOUCH_M..=0.0).contains(&low), "{low} m");
    assert_eq!(decoration.flight(lands + 2.0, gravity), rest);
    assert!(clearance(&corners, decoration.flight(lands * 0.5, gravity), &flat) > 0.0);
}
