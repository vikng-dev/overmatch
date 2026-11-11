//! Static edge solids derived at spawn from the admitted play area on every map composition.
//!
//! Avian holds bodies [`CONTACT_MARGIN_M`] off each face; ray queries and the belt meet the face.
//!
//! PROVISIONAL: invisibility is replaced by map art (#200). Terrain membership routes debris
//! landing response through soil compliance until generic contact-material selection (#200).
//! The design and alternatives live in `.agents/docs/design/map-surroundings.md`.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{Layer, map::MapManifest};

use super::BoundaryWall;

/// Resolution: Parry 0.27's f32 GJK misreads a contact within a fraction of a metre of a face
/// this large: at touch its normal runs along the face with depths toward the far end, a little
/// further off it drops the contact. MEASURED at 0.5 m on faces to 1,250 m half-diagonal (the
/// shipped edge's is 930 m): of 4,000 resting hulls per face, none dropped and no normal off the
/// face; map-surroundings §3.6.
const CONTACT_MARGIN_M: f32 = 0.5;

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
            CollisionMargin(CONTACT_MARGIN_M),
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

    /// Fixture: a 12-vertex convex hull, 2.07 x 0.78 x 5.92 m, as f32 bits.
    const HULL: [[u32; 3]; 12] = [
        [0x3f84939f, 0xbe89d7b8, 0xc03f4d9c],
        [0x3f84939f, 0x3df8d310, 0xc03f4d9c],
        [0x3f8493a0, 0xbf0d942e, 0xc0153cc2],
        [0x3f8493a4, 0xbf0d9412, 0x4036f21e],
        [0x3f8493a4, 0x3d8b4f00, 0x403b6f09],
        [0x3f8493a2, 0x3e680ba0, 0xc00b43da],
        [0xbf846277, 0xbe89d7b8, 0xc03f4d9c],
        [0xbf846277, 0x3df8d310, 0xc03f4d9c],
        [0xbf846278, 0xbf0d942e, 0xc0153cc2],
        [0xbf84627c, 0xbf0d9412, 0x4036f21e],
        [0xbf84627c, 0x3d8b4f00, 0x403b6f09],
        [0xbf84627a, 0x3e680ba0, 0xc00b43da],
    ];

    /// The solver pushes a body out along its manifold's normal by its depth, so containment
    /// relies on a hull resting on an edge solid, or pressed into its margin, keeping a manifold
    /// with the face's normal and a depth short of the solid itself. Mutant: without the margin,
    /// hulls within f32 resolution of a shipped-size face get normals along it, depths toward
    /// its far end, or no manifold.
    #[test]
    fn hulls_on_a_shipped_edge_solid_get_its_face() {
        let mut manifest = crate::map::tests::shipped_manifest();
        manifest.instances.clear();
        let mut app = world_on(manifest, false);
        let world = app.world_mut();
        let walls: Vec<_> = world
            .query_filtered::<(
                Entity,
                &Collider,
                &Position,
                &Rotation,
                Option<&CollisionMargin>,
            ), With<BoundaryWall>>()
            .iter(world)
            .map(|(wall, collider, position, rotation, margin)| {
                let half = collider.shape_scaled().as_cuboid().unwrap().half_extents;
                (
                    wall,
                    half,
                    position.0,
                    rotation.0,
                    margin.map_or(0.0, |m| m.0),
                )
            })
            .collect();
        let points: Vec<Vec3> = HULL
            .iter()
            .map(|bits| Vec3::from_array(bits.map(f32::from_bits)))
            .collect();
        let hull = Collider::convex_hull(points.clone()).unwrap();
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut unit = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        let mut hulls = Vec::new();
        for (wall, half, position, rotation, margin) in walls {
            for index in 0..500 {
                // Fixture: within a degree of square to the face, away from its ends, 3 to 7 m
                // up; alternately within 20 um of the margin and pressed up to 5 cm into it (the
                // wall drives press at most 1.4 cm, MEASURED).
                let mut tilt = || (unit() - 0.5) * 2.0_f32.to_radians();
                let turn = Quat::from_euler(EulerRot::YXZ, tilt(), tilt(), tilt());
                let overlap = if index % 2 == 0 {
                    (unit() - 0.5) * 40e-6
                } else {
                    unit() * 0.05_f32.min(margin)
                };
                // Wall-local X runs along the edge and Z inward; the inner face is z = half.z.
                let reach = points
                    .iter()
                    .map(|point| -(turn * *point).z)
                    .fold(f32::NEG_INFINITY, f32::max);
                let local = Vec3::new(
                    (unit() - 0.5) * 2.0 * (half.x - 5.0),
                    3.0 + 4.0 * unit() - position.y,
                    half.z + margin - overlap + reach,
                );
                let body = world
                    .spawn((
                        RigidBody::Dynamic,
                        hull.clone(),
                        Transform::from_translation(position + rotation * local)
                            .with_rotation(rotation * turn),
                        CollisionLayers::new([Layer::Vehicle], [Layer::Terrain]),
                    ))
                    .id();
                hulls.push((body, wall, rotation * Vec3::Z, overlap.max(0.0), margin));
            }
        }
        app.update();
        let contacts = app.world().resource::<ContactGraph>();
        let (mut least, mut excess, mut past_mm) = (1.0_f32, f32::NEG_INFINITY, 0);
        let count = hulls.len();
        for (body, wall, inward, overlap, margin) in hulls {
            let (_, pair) = contacts
                .get(body, wall)
                .expect("a hull within the margin pairs with the face");
            assert!(
                !pair.manifolds.is_empty(),
                "the face dropped a resting hull"
            );
            // The normal points from the first collider to the second.
            let face = if pair.collider1 == body {
                -inward
            } else {
                inward
            };
            // Avian measures depth from the margin's surface, so the solid's face is at `margin`.
            let mut deepest = f32::NEG_INFINITY;
            for manifold in &pair.manifolds {
                least = least.min(manifold.normal.dot(face));
                for point in &manifold.points {
                    deepest = deepest.max(point.penetration);
                }
            }
            assert!(deepest < margin, "a manifold reaches into the solid");
            excess = excess.max(deepest - overlap);
            past_mm += usize::from(deepest - overlap > 1e-3);
        }
        println!(
            "MEASURED least normal cosine {least}; {past_mm} of {count} hulls with a point over 1 mm deeper than their overlap, most {excess} m"
        );
        // Resolution: 2.6 degrees separates the face's own normal (f32 pose rounding 750 m out is
        // 6e-5 m over a 0.5 m standoff) from the faults, which miss by tens of degrees.
        assert!(least > 0.999, "a manifold off the face normal");
    }
}
