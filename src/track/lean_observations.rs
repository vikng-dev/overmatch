//! On-demand observations of the game's driving physics on generic fixture terrain.

use crate::headless_test::{booted_driving_grid, drive_tick, start_fixed_clock};
use crate::terrain_grid::{HeightGrid, TerrainExtent};
use crate::track::sim::TrackContacts;
use avian3d::prelude::{Position, Rotation};
use bevy::prelude::*;

/// Observations, never output gates: the shipped simulation composes support, grip and
/// transmission on a flat, an incline and a crease, without rendering or timing the run.
#[test]
#[ignore = "resolution experiment; run on demand"]
fn terrain_resolution_observations() {
    // Fixture: 64 m square, 1 m cells; slopes and ridge height are terrain inputs.
    for kind in ["flat", "incline", "crease"] {
        let size = 65;
        let heights = (0..size)
            .flat_map(|j| {
                (0..size).map(move |i| {
                    let (x, z) = (i as f32 - 32.0, j as f32 - 32.0);
                    match kind {
                        "flat" => 0.0,
                        "incline" => 0.1 * z,
                        "crease" => 0.15 * (1.0 - (x - 10.0 + (z - 5.0) * 0.25).abs()).max(0.0),
                        _ => unreachable!(),
                    }
                })
            })
            .collect::<Vec<_>>();
        let grid = HeightGrid::new(
            heights.into(),
            size,
            TerrainExtent {
                world_size_m: 64.0,
                height_offset_m: 0.0,
                height_span_m: 1.0,
            },
        );
        let mut app = booted_driving_grid(grid);
        app.init_resource::<crate::cost::stages::Stages>();
        start_fixed_clock(&mut app);
        let tank = app
            .world_mut()
            .query_filtered::<Entity, (With<crate::tank::Tank>, With<crate::tank::Controlled>)>()
            .single(app.world())
            .unwrap();
        for (label, ticks, throttle) in [("settle", 192, 0.0), ("drive", 128, 1.0)] {
            for _ in 0..ticks {
                app.world()
                    .resource::<crate::cost::stages::Stages>()
                    .take_counts();
                drive_tick(&mut app, tank, throttle, 0.0);
            }
            let world = app.world();
            let contacts = world.get::<TrackContacts>(tank).unwrap();
            let load: f32 = contacts.0.iter().flatten().map(|c| c.load).sum();
            let force: Vec3 = contacts
                .0
                .iter()
                .flatten()
                .map(|c| c.normal * c.load + c.tangential_force())
                .sum();
            let count = contacts.0.each_ref().map(|side| side.len());
            let work = serde_json::to_value(
                world
                    .resource::<crate::cost::stages::Stages>()
                    .take_counts(),
            )
            .unwrap();
            println!(
                "MEASURED terrain {kind} {label}: contacts={count:?} load={load:.3} force={force:?} pose={:?} rotation={:?} work={work}",
                world.get::<Position>(tank).unwrap().0,
                world.get::<Rotation>(tank).unwrap().0
            );
        }
    }
}

/// Work observations at each requested resolution, with the shipped plugin and an empty flat
/// fixture world. No output is a requirement; counts expose the cost of each quadrature.
#[test]
#[ignore = "resolution work matrix, run on demand"]
fn resolution_work_matrix() {
    use crate::track::forces::ContactResolution;
    for stations in [97, 48, 24, 16] {
        // Fixture: the requested experiment settings.
        for lateral_points in [3, 2] {
            let grid = HeightGrid::new(
                vec![0.0; 4].into(),
                2,
                TerrainExtent {
                    world_size_m: 64.0,
                    height_offset_m: 0.0,
                    height_span_m: 1.0,
                },
            );
            let mut app = booted_driving_grid(grid);
            crate::track::sim::set_test_resolution(
                &mut app,
                ContactResolution {
                    stations,
                    lateral_points,
                },
            );
            let geom = app.world().resource::<crate::track::rig_geom::RigGeom>();
            let shoe_width = geom.model.width;
            let loop_length = geom.belt_len();
            app.init_resource::<crate::cost::stages::Stages>();
            start_fixed_clock(&mut app);
            let tank = app
                .world_mut()
                .query_filtered::<Entity, (With<crate::tank::Tank>, With<crate::tank::Controlled>)>(
                )
                .single(app.world())
                .unwrap();
            let mut lo = f64::INFINITY;
            let mut hi = 0.0_f64;
            for _ in 0..128 {
                // Fixture: sample the first 128 ticks of a straight launch.
                app.world()
                    .resource::<crate::cost::stages::Stages>()
                    .take_counts();
                drive_tick(&mut app, tank, 1.0, 0.0);
                let work = serde_json::to_value(
                    app.world()
                        .resource::<crate::cost::stages::Stages>()
                        .take_counts(),
                )
                .unwrap();
                let samples = work["belt_terrain_samples"].as_f64().unwrap() / 2.0;
                lo = lo.min(samples);
                hi = hi.max(samples);
            }
            let contacts = app.world().get::<TrackContacts>(tank).unwrap();
            println!(
                "MEASURED resolution {stations}x{lateral_points}: terrain_samples_per_tank={lo}..{hi} shoe_width_m={shoe_width} loop_length_m={loop_length} contacts={} pose={:?}",
                contacts.0.iter().map(Vec::len).sum::<usize>(),
                app.world().get::<Position>(tank).unwrap().0
            );
        }
    }
}
