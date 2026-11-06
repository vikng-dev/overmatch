//! The game's damage model: the authority ends outward flights at the admitted map floor.

use super::*;
use crate::ballistics::{FireShell, Projectile};

/// The game's damage model: every march ends; the server's per-round work relies on retirement
/// at this map's floor even when a flight passes over the edge without meeting terrain.
#[test]
fn an_outward_round_retires_on_its_first_tick_below_the_map_floor() {
    // Fixture: empty, flat map; a drag-free round isolates the floor from contact and drag laws.
    let manifest = crate::map::tests::fixture_manifest(128.0, 0.0, 50.0);
    let half = manifest.extent.half_extent();
    let floor = manifest.play_area.floor_m();
    let grid = crate::terrain_grid::HeightGrid::new(vec![0.0; 81].into(), 9, manifest.extent);
    let mut app = booted_sim_on_map(Some(grid), Some(manifest));
    start_fixed_clock(&mut app);
    app.update();
    let round = crate::rounds::tests::of_type("Blunt", "1.0", "0.02", "");
    let birth = Vec3::new(half - 1.0, 2.0, 0.0);
    let mut fire = FireShell::of_round(
        &round,
        birth,
        birth,
        Dir3::X,
        crate::spec::FireMechanism::Single,
        false,
        None,
        None,
    );
    let dt = app.world().resource::<Time<Fixed>>().delta_secs();
    fire.speed = 1.0 / dt; // Fixture: one horizontal metre per fixed tick.
    fire.drag_k = 0.0;
    app.world_mut().trigger(fire);
    app.world_mut().flush();
    let shell = app
        .world_mut()
        .query_filtered::<Entity, With<Projectile>>()
        .single(app.world())
        .unwrap();
    let gravity = crate::physical_laws::gravity_mps2();
    // DERIVED: semi-implicit gravity crosses no later than continuous free fall; two ticks cover
    // the strict inequality at the floor and integer rounding of the observation interval.
    let ticks = ((2.0 * (birth.y - floor) / gravity).sqrt() / dt).ceil() as usize + 2;
    let mut crossed = false;
    let (mut last_y, mut last_vy) = (birth.y, 0.0);
    for tick in 1..=ticks {
        app.update();
        if app.world().get::<Projectile>(shell).is_some() {
            let at = app.world().get::<Transform>(shell).unwrap().translation;
            assert!(
                at.y >= floor,
                "a live round persisted below its map floor: {at:?}, floor={floor}"
            );
            crossed |= at.x > half;
            (last_y, last_vy) = (at.y, (at.y - last_y) / dt);
        } else {
            assert!(crossed, "the flight must pass outward through the map edge");
            assert!(
                last_y + (last_vy - gravity * dt) * dt < floor,
                "retirement must occur on the first floor-crossing tick"
            );
            assert!(
                app.world().get_entity(shell).is_err(),
                "retirement removes the complete flight"
            );
            println!(
                "MEASURED outward round retired at tick {tick}, floor={floor} m, last live y={last_y} m"
            );
            return;
        }
    }
    panic!("the outward flight did not retire within its derived free-fall bound");
}
