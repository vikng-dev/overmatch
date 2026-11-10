//! Static edge solids derived at spawn from the admitted play area on every map composition.
//!
//! PROVISIONAL: invisibility is replaced by map art (#200). Terrain membership routes debris
//! landing response through soil compliance until generic contact-material selection (#200).
//! The design and alternatives live in `.agents/docs/design/map-surroundings.md`.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{Layer, map::MapManifest};

use super::BoundaryWall;

pub(super) fn spawn(commands: &mut Commands, manifest: &MapManifest) {
    let friction = crate::substances::contact_friction(manifest.play_area.friction());
    for (index, wall) in manifest.play_area.wall_boxes().enumerate() {
        commands.spawn((
            Name::new(format!(
                "{} field polygon edge {index}",
                manifest.boundary_path().display()
            )),
            BoundaryWall,
            wall,
            RigidBody::Static,
            Collider::cuboid(1.0, 1.0, 1.0),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
            friction,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Production world wiring, with a coarse flat grid and no scenery to isolate edge contacts.
    /// The component-only Window exercises client construction without GPU or winit.
    fn world_on(manifest: MapManifest, windowed: bool) -> App {
        let mut app = crate::headless_test::headless_shell();
        let grid = crate::terrain_grid::HeightGrid::new(vec![0.0; 81].into(), 9, manifest.extent);
        app.insert_resource(manifest)
            .insert_resource(grid)
            .insert_resource(Gravity(Vec3::ZERO))
            .add_plugins((
                crate::offline_physics().disable::<PhysicsInterpolationPlugin>(),
                super::super::plugin,
            ));
        if windowed {
            app.world_mut().spawn(Window {
                visible: false,
                ..default()
            });
        }
        crate::headless_test::finish_plugins(&mut app);
        // Bevy's first real-time update has zero delta; prime it before the first physics step.
        app.update();
        crate::headless_test::start_fixed_clock(&mut app);
        app.update();
        app
    }

    fn tolerance(app: &App) -> f32 {
        app.world()
            .resource::<NarrowPhaseConfig>()
            .contact_tolerance
            * app.world().resource::<PhysicsLengthUnit>().0
    }

    /// Every composition must spawn only ordinary, friction-routed terrain solids at the edge.
    /// Removing production installation, attaching a mesh, or adding field support breaks it.
    #[test]
    fn clients_and_servers_construct_the_same_invisible_edge_solids() {
        for windowed in [false, true] {
            // Fixture: a small empty map; its boundary is admitted by the production constructor.
            let manifest = crate::map::tests::fixture_manifest(32.0, -1.0, 2.0);
            let expected = crate::substances::contact_friction(manifest.play_area.friction());
            let edges = manifest.play_area.polygon().len();
            let mut app = world_on(manifest, windowed);
            let world = app.world_mut();
            let mut walls = world.query_filtered::<(
                &RigidBody,
                &CollisionLayers,
                &Friction,
                Has<Mesh3d>,
                Has<crate::track::terrain::FieldSurface>,
            ), With<BoundaryWall>>();
            assert_eq!(walls.iter(world).count(), edges);
            for (body, layers, friction, mesh, field) in walls.iter(world) {
                assert_eq!(*body, RigidBody::Static);
                assert_eq!(
                    *layers,
                    CollisionLayers::new([Layer::Terrain], LayerMask::ALL)
                );
                assert_eq!(*friction, expected);
                assert!(!mesh && !field);
            }
            assert!(
                world
                    .resource::<crate::world::TerrainMap>()
                    .blocks
                    .is_empty()
            );
            assert!(
                world
                    .resource::<crate::world::SpawnObstacles>()
                    .0
                    .is_empty()
            );
        }
    }

    /// The camera's terrain-only query must see the wall, with no boundary query special case.
    #[test]
    fn ground_distance_toward_every_edge_returns_its_inner_face() {
        // Fixture: axis-aligned square, ray at mid-height, above the flat ground.
        let manifest = crate::map::tests::fixture_manifest(32.0, -1.0, 2.0);
        let distance = manifest.extent.half_extent();
        let mut app = world_on(manifest, false);
        let allowance = tolerance(&app);
        for direction in [Dir3::X, Dir3::NEG_X, Dir3::Z, Dir3::NEG_Z] {
            let measured = app
                .world_mut()
                .run_system_once(move |spatial: SpatialQuery| {
                    crate::world::ground_distance(
                        &spatial,
                        Ray3d::new(Vec3::Y, direction),
                        distance * 2.0,
                    )
                })
                .unwrap();
            assert!(
                (measured - distance).abs() <= allowance,
                "wall distance {measured}, expected {distance}"
            );
            println!("MEASURED edge {direction:?}: camera distance {measured} m");
        }
    }

    /// Server bodies must stay inside the side bound, including a rotating flat debris piece.
    /// Fixture velocity and shape exercise ordinary debris layers; gravity is zero to isolate sides.
    #[test]
    fn outward_flat_and_spinning_pieces_remain_inside() {
        for spin in [Vec3::ZERO, Vec3::new(0.0, 6.0, 0.0)] {
            for direction in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
                let manifest = crate::map::tests::fixture_manifest(32.0, -1.0, 2.0);
                let half = manifest.extent.half_extent();
                let mut app = world_on(manifest, false);
                let allowance = tolerance(&app);
                let piece = app
                    .world_mut()
                    .spawn((
                        crate::debris::DebrisPiece,
                        Transform::from_translation(direction * (half - 4.0) + Vec3::Y),
                        RigidBody::Dynamic,
                        Collider::cuboid(2.0, 0.25, 1.0),
                        ColliderDensity(1800.0),
                        crate::construction::breakage::piece_layers(),
                        LinearVelocity(direction * 12.0),
                        AngularVelocity(spin),
                    ))
                    .id();
                let mut greatest_overrun = f32::NEG_INFINITY;
                for _ in 0..crate::headless_test::FIXED_TICKS_PER_SECOND {
                    app.update();
                    let world = app.world();
                    let bounds = world.get::<Collider>(piece).unwrap().aabb(
                        world.get::<Position>(piece).unwrap().0,
                        *world.get::<Rotation>(piece).unwrap(),
                    );
                    let overrun = bounds
                        .min
                        .xz()
                        .abs()
                        .max_element()
                        .max(bounds.max.xz().abs().max_element())
                        - half;
                    greatest_overrun = greatest_overrun.max(overrun);
                    assert!(
                        overrun <= allowance,
                        "piece crossed by {overrun} m with spin {spin:?}, side {direction:?}"
                    );
                }
                println!(
                    "MEASURED piece spin {spin:?}, side {direction:?}: greatest side overrun {greatest_overrun} m; tolerance {allowance} m"
                );
            }
        }
    }

    /// The owner's zero-grip choice must hold on every pair path, static and dynamic.
    /// Read the spawned wall and shipped rows so a material rule change cannot bypass the witness.
    #[test]
    fn the_shipped_wall_has_no_grip_on_any_friction_path() {
        let mut manifest = crate::map::tests::shipped_manifest();
        manifest.instances.clear();
        let mut app = world_on(manifest, false);
        let wall = *app
            .world_mut()
            .query_filtered::<&Friction, With<BoundaryWall>>()
            .iter(app.world())
            .next()
            .unwrap();
        let default = app.world().resource::<DefaultFriction>().0;
        let registry = crate::substances::SubstanceRegistry::shipped_static();
        let mut tested = 0;
        let no_grip = |other: Friction| {
            for pair in [wall.combine(other), other.combine(wall)] {
                assert_eq!(pair.static_coefficient, 0.0);
                assert_eq!(pair.dynamic_coefficient, 0.0);
            }
        };
        no_grip(default);
        for name in registry.keys() {
            if let Some(hard) = registry.get(name).unwrap().hard()
                && hard.friction.is_some()
            {
                no_grip(hard.contact_friction(name).unwrap());
                tested += 1;
            }
        }
        assert!(
            tested > 0,
            "the shipped registry supplies contact substances"
        );
        let path = crate::assets::asset_root().join(crate::tank::TANK_SPEC_PATH);
        let spec: crate::spec::TankSpec = crate::provenance::from_shipped_str(
            path.to_str().unwrap(),
            &std::fs::read_to_string(&path).unwrap(),
        )
        .unwrap();
        // Fixture nominal pressure and centre-of-mass height are unused by pair mixing; the
        // track friction comes from data.
        let grip = crate::track::forces::GripParams::of(&spec, 1.0, 1.0);
        no_grip(grip.track);
        assert_eq!(grip.pair(wall), 0.0);
        println!(
            "MEASURED no-grip pairs: {tested} shipped substances, track GripParams::pair and DefaultFriction, static and dynamic"
        );
    }
}
