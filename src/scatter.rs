//! The map's object scatter: the authored placement of constructions, out of the map's own
//! manifest ([`crate::map`]), spawned with static colliders on every composition, the dedicated
//! server included — a graybox construction as a box proxy, or a construction of the game's
//! destruction model as its own body
//! (below). A graybox construction's box is drawn in brick under a timber roof
//! ([`crate::construction_surface`]), its shape still the collider's.
//!
//! SHARED DATA, NOT REPLICATION — the same stance the terrain takes: both binaries read the same
//! shipped file and derive the same entities, so no scatter entity crosses the wire. The
//! derivation is order-fixed (instances sorted by their own `id` before placement) and pure f32,
//! so client and server land on identical transforms.
//!
//! THE LIBRARY IS THE MAP'S DATA ([`crate::map`] module doc): each instance's prototype id resolves
//! through the manifest's [`Prototype`] library, checked complete at parse, so nothing here can
//! meet an id it does not know.
//!
//! EVERY INSTANCE KEEPS ITS IDENTITY: the file's own id and its ordinal in the id-sorted order ride
//! on the spawned entity as [`MapInstance`], so anything that must name one instance to a peer —
//! a construction's state on the wire — names it by a number both peers derived from the same bytes.
//!
//! THE FILE'S Y IS STALE (authored against an older sculpt): every instance is re-projected onto
//! the live [`HeightGrid`] at its own XZ. Yaw is taken as authored; every prototype keeps its authored size.
//!
//! Graybox blocks and constructions of the destruction model are colliders outside
//! the immutable terrain field; construction bounds also supply spawn obstacles.

use std::collections::BTreeMap;

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
use bevy::prelude::*;

use crate::Layer;
use crate::construction_surface::ConstructionMaterial;
use crate::map::{InstanceRecord, MapManifest, Prototype};
use crate::terrain_grid::HeightGrid;

/// One placed instance's identity, as the map gave it (module doc).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
#[component(immutable)]
pub(crate) struct MapInstance {
    /// The file's own instance id.
    pub(crate) id: String,
    /// The instance's position in the id-sorted placement — the same number on every peer that
    /// loaded the same manifest, which the map's content digest guarantees.
    pub(crate) ordinal: u16,
}

/// Marks a graybox construction's static collider. Spawned in every composition,
/// server included, so the tag rides with the shape rather than with a window; the client's
/// scatter-hit read ([`crate::vfx`]) filters its casts to entities carrying it.
#[derive(Component)]
pub(crate) struct ScatterProxy;

/// Resolution: least squared quaternion length [`resolve`] will normalize. Far below any rounding an f32
/// text export produces (those land within ~1e-7 of unit) and far above the point where the
/// squaring itself underflows, so the only thing it rejects is a quaternion with no direction in it.
const MIN_ROTATION_LENGTH_SQ: f32 = 1.0e-12;

/// One parsed instance: the prototype it resolves to and its authored pose MINUS the stale Y.
struct Instance<'a> {
    /// The prototype id, as the file references it — the view's mesh key.
    prototype: &'a str,
    shape: &'a Prototype,
    /// Authored world XZ. The file's Y never leaves [`resolve`].
    xz: Vec2,
    rotation: Quat,
    /// The file's own instance id — the sort key that fixes iteration order.
    id: &'a str,
}

/// One placed instance: its prototype, its identity and its GROUND pose — the authored XZ and
/// rotation, the Y re-sampled from the grid, at the prototype's authored size.
#[derive(Debug, Clone, PartialEq)]
struct Placement<'a> {
    prototype: &'a str,
    shape: &'a Prototype,
    identity: MapInstance,
    pose: Isometry3d,
}

/// The friction a graybox prototype's collider carries: its `substance` row's, which
/// `map::parse` certified when it read the library.
fn graybox_friction(substance: &str) -> avian3d::prelude::Friction {
    crate::substances::SubstanceRegistry::shipped_static()
        .contact_friction(substance)
        .unwrap_or_else(|err| {
            panic!("scatter: map::parse refuses a graybox substance with no friction: {err}")
        })
}

impl Placement<'_> {
    /// A graybox collider and spawn obstacle: a unit cube posed and scaled, its centre lifted by the
    /// prototype's own bounds. `None` for a construction with its own geometry.
    fn block(&self) -> Option<Transform> {
        match self.shape {
            Prototype::Block { min, max, .. } => {
                let (min, max) = (Vec3::from_array(*min), Vec3::from_array(*max));
                Some(Transform {
                    translation: Vec3::from(self.pose.translation)
                        + self.pose.rotation * ((min + max) * 0.5),
                    rotation: self.pose.rotation,
                    scale: max - min,
                })
            }
            Prototype::Construction { .. } => None,
        }
    }
}

/// Resolve the manifest's instance records against its prototype library and put them in
/// ITERATION ORDER. Everything past the manifest's own checks is ADR-0011 fail-fast: a pose
/// component that is not finite (a NaN reaches the solver as a NaN collider) or a rotation with no
/// length to normalize is a broken ship and panics naming the instance.
fn resolve<'a>(
    records: &'a [InstanceRecord],
    library: &'a BTreeMap<String, Prototype>,
) -> Vec<Instance<'a>> {
    let mut instances: Vec<Instance<'a>> = records
        .iter()
        .map(|raw| {
            // `map::parse` already refused any id outside the library.
            let shape = &library[&raw.prototype];
            let translation = Vec3::from_array(raw.translation);
            let rotation = Quat::from_xyzw(
                raw.rotation[0],
                raw.rotation[1],
                raw.rotation[2],
                raw.rotation[3],
            );
            assert!(
                translation.is_finite() && rotation.is_finite(),
                "scatter: instance {} has a non-finite pose",
                raw.id,
            );
            // A quaternion the exporter wrote as f32 TEXT is never exactly unit, so an off-unit one
            // is normalized below and welcome. One with no length is not off-unit, it is not a
            // rotation: `normalize` divides by zero and hands the solver a NaN collider, which is
            // finite arithmetic's blind spot — the check above passes it. `[0,0,0,0]` is the
            // shape this takes in a file.
            assert!(
                rotation.length_squared() > MIN_ROTATION_LENGTH_SQ,
                "scatter: instance {} has rotation {:?}, which names no orientation",
                raw.id,
                raw.rotation,
            );
            Instance {
                prototype: &raw.prototype,
                shape,
                // The Y is DROPPED here, where the stale value cannot leak further.
                xz: translation.xz(),
                rotation: rotation.normalize(),
                id: &raw.id,
            }
        })
        .collect();
    // The file's own ids are the fixed order every peer spawns in.
    instances.sort_by(|a, b| a.id.cmp(b.id));
    assert!(
        instances.len() <= usize::from(u16::MAX) + 1,
        "scatter: {} instances, more than a MapInstance ordinal can name",
        instances.len(),
    );
    instances
}

/// Project every instance onto `grid`: the authored XZ and rotation kept, the Y taken from the
/// surface under it, the ordinal its position in the sorted list. Pure and order-preserving, so
/// two runs over the same file and grid produce the same list.
fn place<'a>(instances: &[Instance<'a>], grid: &HeightGrid) -> Vec<Placement<'a>> {
    instances
        .iter()
        .enumerate()
        .map(|(ordinal, instance)| Placement {
            prototype: instance.prototype,
            shape: instance.shape,
            identity: MapInstance {
                id: instance.id.to_owned(),
                ordinal: u16::try_from(ordinal).expect("resolve bounded the count"),
            },
            pose: Isometry3d::new(
                Vec3::new(
                    instance.xz.x,
                    grid.height_at(instance.xz.x, instance.xz.y),
                    instance.xz.y,
                ),
                instance.rotation,
            ),
        })
        .collect()
}

/// The asset server and stores for the shared construction material.
pub(crate) struct ViewAssets<'a> {
    pub(crate) server: &'a AssetServer,
    pub(crate) construction_materials: &'a mut Assets<ConstructionMaterial>,
    pub(crate) buffers: &'a mut Assets<bevy::render::storage::ShaderBuffer>,
}

/// The view-side handles, built ONCE per world: one mesh per prototype, cloned per instance, and
/// one material per surface. Only built where there is a window to draw into.
struct ScatterView {
    /// The unit cube per construction prototype, keyed by its id: the cube the block transform scales,
    /// its faces framed for the construction surface
    /// ([`crate::construction_surface::unit_box`]).
    construction: BTreeMap<String, Handle<Mesh>>,
    /// One merged mesh per construction asset of the destruction model, keyed by the asset id: its
    /// elements' faces in the construction frame ([`crate::construction::view::mesh`]), shared by every
    /// instance.
    structure: BTreeMap<String, Handle<Mesh>>,
    /// The ONE material every construction wears, graybox or not: no instance gets its own, so the
    /// constructions batch.
    surface: Handle<ConstructionMaterial>,
}

/// Parsed construction data and shared view handles for a fresh scatter. The initial world and
/// every subsequent world use this same recipe; mutable bodies never supply construction data.
#[derive(Resource)]
pub(crate) struct ScatterScene {
    manifest: MapManifest,
    grid: HeightGrid,
    view: Option<ScatterView>,
}

impl ScatterView {
    fn new(
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        assets: &mut ViewAssets<'_>,
        manifest: &MapManifest,
    ) -> Self {
        let library = &manifest.prototypes;
        Self {
            construction: library
                .iter()
                .filter_map(|(id, shape)| match *shape {
                    Prototype::Block { ref surface, .. } => Some((
                        id.clone(),
                        meshes.add(crate::construction_surface::unit_box(
                            surface.top_tangent,
                            &surface.faces,
                        )),
                    )),
                    Prototype::Construction { .. } => None,
                })
                .collect(),
            structure: library
                .values()
                .filter_map(|shape| match shape {
                    Prototype::Construction { asset } => Some(asset),
                    Prototype::Block { .. } => None,
                })
                .map(|asset| {
                    let kind = manifest
                        .constructions
                        .get(asset)
                        .expect("map::load loaded every construction asset the library names");
                    (
                        asset.clone(),
                        meshes.add(crate::construction::view::mesh(kind)),
                    )
                })
                .collect(),
            surface: crate::construction_surface::material(
                commands,
                assets.server,
                assets.construction_materials,
                assets.buffers,
            ),
        }
    }
}

/// Spawn the scatter into the world the grid describes, returning construction bounds as spawn
/// obstacles ([`crate::world::SpawnObstacles`]), never as analytic terrain. Colliders spawn in every
/// composition; meshes only where `view_assets` offers a server to draw
/// with — the dedicated server draws nothing, exactly as it skips the terrain's render tiles.
pub(crate) fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    manifest: &MapManifest,
    grid: &HeightGrid,
    mut view_assets: Option<ViewAssets<'_>>,
) -> Vec<Transform> {
    let view = view_assets
        .as_mut()
        .map(|assets| ScatterView::new(commands, meshes, assets, manifest));
    let scene = ScatterScene {
        manifest: manifest.clone(),
        grid: grid.clone(),
        view,
    };
    let obstacles = scene.construct(commands);
    commands.insert_resource(scene);
    obstacles
}

/// Run the startup construction again. No scatter in the immutable test-course world.
pub(crate) fn reconstruct(mut commands: Commands, scene: Option<Res<ScatterScene>>) {
    if let Some(scene) = scene {
        scene.construct(&mut commands);
    }
}

impl ScatterScene {
    fn construct(&self, commands: &mut Commands) -> Vec<Transform> {
        let manifest = &self.manifest;
        let placements = place(
            &resolve(&manifest.instances, &manifest.prototypes),
            &self.grid,
        );
        let view = &self.view;
        if let Some(view) = view {
            // The breaks' view draws a construction's new meshes in the same one material.
            commands.insert_resource(crate::construction::view::Look {
                surface: view.surface.clone(),
            });
        }
        let statics = CollisionLayers::new([Layer::Terrain], LayerMask::ALL);
        let (mut constructions, mut structures) = (0usize, 0usize);
        let mut obstacles = Vec::new();
        for placement in &placements {
            match placement.shape {
                Prototype::Construction { asset } => {
                    let kind = manifest
                        .constructions
                        .get(asset)
                        .expect("map::load loaded every construction asset the library names");
                    let root = crate::construction::spawn(
                        commands,
                        kind,
                        placement.identity.clone(),
                        placement.pose,
                    );
                    obstacles.push(crate::construction::footprint(kind, placement.pose));
                    if let Some(view) = view {
                        commands.spawn((
                            ChildOf(root),
                            Transform::IDENTITY,
                            Mesh3d(view.structure[asset].clone()),
                            MeshMaterial3d(view.surface.clone()),
                            crate::render_cost::CasterClass::Construction,
                            crate::construction::view::View,
                        ));
                    }
                    structures += 1;
                }
                Prototype::Block { substance, .. } => {
                    let block = placement
                        .block()
                        .expect("a construction placement carries a block transform");
                    obstacles.push(block);
                    let mut entity = commands.spawn((
                        block,
                        RigidBody::Static,
                        Collider::cuboid(1.0, 1.0, 1.0),
                        statics,
                        graybox_friction(substance),
                        crate::substances::SubstanceRegistry::shipped_static()
                            .hard(substance)
                            .expect("map::parse certifies every graybox substance")
                            .feedback,
                        ScatterProxy,
                        placement.identity.clone(),
                    ));
                    if let Some(view) = view {
                        entity.insert((
                            Mesh3d(view.construction[placement.prototype].clone()),
                            MeshMaterial3d(view.surface.clone()),
                            crate::render_cost::CasterClass::Construction,
                        ));
                    }
                    constructions += 1;
                }
            }
        }
        info!(
            "scatter: {} instances placed on the live surface — {constructions} graybox constructions, {structures} constructions (a collider per element)",
            placements.len(),
        );
        obstacles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::map::tests::construction_fixture_manifest;

    /// A tilted fixture surface: height rises with x and z, so a re-projection lands on a different
    /// Y at every XZ (a flat fixture would pass whatever the placement did with the file's Y).
    fn sloped_grid() -> HeightGrid {
        let size = 9u32;
        let n = size as usize;
        let samples: Vec<f32> = (0..n * n)
            .map(|k| (k % n) as f32 * 0.5 + (k / n) as f32 * 0.25)
            .collect();
        HeightGrid::new(samples.into(), size, crate::terrain_grid::FIXTURE_EXTENT)
    }

    /// A one-construction library (fixture data).
    fn library() -> BTreeMap<String, Prototype> {
        BTreeMap::from([("house_proxy".to_owned(), house())])
    }

    fn house() -> Prototype {
        Prototype::Block {
            min: [-2.0, -0.5, -3.0],
            max: [2.0, 2.5, 3.0],
            substance: "Brick".to_owned(),
            surface: crate::map::tests::fixture_box_surface(),
        }
    }

    /// One instance record of `prototype`, posed with a deliberately WRONG Y.
    fn record(id: &str, xz: Vec2, stale_y: f32) -> InstanceRecord {
        InstanceRecord {
            prototype: "house_proxy".to_owned(),
            translation: [xz.x, stale_y, xz.y],
            rotation: [0.0, 0.0, 0.0, 1.0],
            id: id.to_owned(),
        }
    }

    /// THE stale-Y law: the placed Y is the grid's height under the instance's XZ, and the file's
    /// own Y reaches nothing.
    #[test]
    fn placement_reprojects_onto_the_grid_and_ignores_the_authored_y() {
        let (grid, library) = (sloped_grid(), library());
        let xz = Vec2::new(37.5, -112.25);
        let placed =
            |stale_y: f32| place(&resolve(&[record("a", xz, stale_y)], &library), &grid)[0].pose;
        let pose = placed(-500.0);
        assert_eq!(pose.translation.x, xz.x);
        assert_eq!(pose.translation.z, xz.y);
        assert_eq!(pose.translation.y, grid.height_at(xz.x, xz.y));
        assert_eq!(placed(900.0), pose, "the authored Y must change nothing");
    }

    /// IDENTITY IS THE SORTED ORDER, NOT THE FILE'S: the ordinal an instance carries is its position
    /// among the ids, whatever order the author exported them in, so two peers reading the same
    /// bytes name the same instance by the same number. A mutant that numbered in file order would
    /// hand `b` ordinal 0 here.
    #[test]
    fn ordinals_follow_the_id_order_whatever_the_file_order() {
        let records = [
            record("b", Vec2::ZERO, 0.0),
            record("c", Vec2::X, 0.0),
            record("a", Vec2::Y, 0.0),
        ];
        let library = library();
        let placed = place(&resolve(&records, &library), &sloped_grid());
        let identities: Vec<(&str, u16)> = placed
            .iter()
            .map(|placement| (placement.identity.id.as_str(), placement.identity.ordinal))
            .collect();
        assert_eq!(identities, [("a", 0), ("b", 1), ("c", 2)]);
    }

    /// A zero quaternion is FINITE, so the pose check waves it through — and then `normalize`
    /// divides by a zero length and puts a NaN rotation on a static collider, which reaches Avian
    /// as geometry nothing can resolve against. It has to be refused where it is still a number in
    /// a file (ADR-0011).
    #[test]
    #[should_panic(expected = "which names no orientation")]
    fn a_rotation_with_no_length_panics() {
        let mut raw = record("a", Vec2::ZERO, 0.0);
        raw.rotation = [0.0, 0.0, 0.0, 0.0];
        resolve(&[raw], &library());
    }

    /// Spawn on the sloped fixture grid. Physics registers collider requirements before spawning;
    /// drawing supplies the asset server and shared material stores.
    fn spawned(manifest: &MapManifest, draw: bool, physics: bool) -> (App, Vec<Transform>) {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .init_asset::<ConstructionMaterial>()
        .init_asset::<bevy::render::storage::ShaderBuffer>()
        .init_asset::<bevy::world_serialization::WorldAsset>();
        if physics {
            app.add_plugins((
                bevy::time::TimePlugin,
                bevy::transform::TransformPlugin,
                avian3d::prelude::PhysicsPlugins::default(),
            ))
            .insert_resource(bevy::time::TimeUpdateStrategy::FixedTimesteps(1));
        }
        let grid = sloped_grid();
        let mut obstacles: Vec<Transform> = Vec::new();
        let server = draw.then(|| app.world().resource::<AssetServer>().clone());
        let world = app.world_mut();
        world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
            world.resource_scope(
                |world, mut constructions: Mut<Assets<ConstructionMaterial>>| {
                    world.resource_scope(
                        |world, mut buffers: Mut<Assets<bevy::render::storage::ShaderBuffer>>| {
                            let mut queue = bevy::ecs::world::CommandQueue::default();
                            let mut commands = Commands::new(&mut queue, world);
                            obstacles = spawn(
                                &mut commands,
                                &mut meshes,
                                manifest,
                                &grid,
                                server.as_ref().map(|server| ViewAssets {
                                    server,
                                    construction_materials: &mut constructions,
                                    buffers: &mut buffers,
                                }),
                            );
                            queue.apply(world);
                        },
                    );
                },
            );
        });
        (app, obstacles)
    }

    /// EVERY CONSTRUCTION WEARS THE ONE MATERIAL, the draw-count invariant the brick-and-timber surface
    /// rides on: a second house — another id and position (fixture data) — draws with the first
    /// one's mesh and material handles, and the world holds one construction material in all. A view
    /// that minted either per instance would split the constructions into a draw each.
    #[test]
    fn every_construction_draws_with_one_mesh_and_one_material() {
        let mut manifest = construction_fixture_manifest();
        let mut second = manifest
            .instances
            .iter()
            .find(|instance| {
                matches!(
                    manifest.prototypes[&instance.prototype],
                    Prototype::Block { .. }
                )
            })
            .expect("the fixture places a house")
            .clone();
        second.id = "house_0002".to_owned();
        second.translation[0] += 30.0;
        manifest.instances.push(second);
        let (mut app, _) = spawned(&manifest, true, false);
        let world = app.world_mut();
        let drawn: Vec<(AssetId<Mesh>, AssetId<ConstructionMaterial>)> = world
            .query_filtered::<(&Mesh3d, &MeshMaterial3d<ConstructionMaterial>), Without<ChildOf>>()
            .iter(world)
            .map(|(mesh, material)| (mesh.id(), material.id()))
            .collect();
        assert_eq!(drawn.len(), 2, "both graybox houses draw");
        assert_eq!(drawn[0], drawn[1], "one mesh and one material for both");
        assert_eq!(
            world.resource::<Assets<ConstructionMaterial>>().len(),
            1,
            "one construction material in the world",
        );
    }

    /// Belt contacts read a graybox's authored material pair and rigid rolling law, independently
    /// of the soil below it. Testing only Avian's coefficient would miss FieldSurface exclusion.
    #[test]
    fn a_graybox_belt_contact_reads_its_substance_instead_of_underlying_soil() {
        use crate::track::forces::{
            ForceParams, GripElements, GripParams, SideInput, SideState, TravelField, contact_side,
            rigid_rolling_resistance, rolling_reference_kpa,
        };
        use crate::track::rubble::{PieceData, RubbleBodies, RubbleQuery};
        use bevy::ecs::system::RunSystemOnce;
        use bevy::math::Affine3A;
        let manifest = construction_fixture_manifest();
        let (mut app, _) = spawned(&manifest, false, true);
        app.finish();
        app.cleanup();
        app.update();
        app.update();
        let (entity, block) = app
            .world_mut()
            .query_filtered::<(Entity, &Transform), With<ScatterProxy>>()
            .single(app.world())
            .map(|(e, t)| (e, *t))
            .unwrap();
        let expected = manifest
            .prototypes
            .values()
            .find_map(|prototype| match prototype {
                Prototype::Block { substance, .. } => Some(
                    crate::substances::SubstanceRegistry::shipped_static()
                        .contact_friction(substance)
                        .unwrap(),
                ),
                _ => None,
            })
            .expect("the fixture declares the graybox's substance");
        let top = block.translation + block.rotation * Vec3::Y * (block.scale.y / 2.0);
        let grid = sloped_grid();
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery, bodies: Query<PieceData>| {
                let near = rubble.near(top, 2.0); // Fixture: encloses the pad and its box top.
                assert!(
                    near.contains(&entity),
                    "graybox colliders must enter belt support queries"
                );
                let pieces = RubbleBodies::gather(&rubble, &near, &bodies);
                // Fixture pad: 0.4 m long, 0.2 m wide, resting 5 mm inside the support envelope.
                let loop_pts = [
                    Vec2::new(-0.2, 0.02),
                    Vec2::new(0.2, 0.02),
                    Vec2::new(0.2, 1.02),
                    Vec2::new(-0.2, 1.02),
                    Vec2::new(-0.2, 0.02),
                ];
                let input = SideInput {
                    loop_pts: &loop_pts,
                    count: 24,
                    plane_x: 0.0,
                    columns: [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)],
                    travel: TravelField { knots: &[] },
                };
                let track = crate::substances::contact_friction(0.7); // Fixture material coefficient.
                let params = ForceParams {
                    face_offset: 0.025,
                    free_travel: 0.0,
                    support_stiffness_per_m: 2.0e6,
                    support_damping_per_m: 1.0e5,
                    engage_depth: 0.002,
                    probe_reach: 0.5,
                    grip: Some(GripParams {
                        track,
                        nominal_pressure_kpa: rolling_reference_kpa(),
                        side_weight: 100_000.0,
                    }),
                };
                let pair = track.combine(expected).dynamic_coefficient;
                for soil in [0.01, 0.99] {
                    // Fixture: two soils that differ from the solid pair.
                    let field = crate::track::oracle::BlockField::default()
                        .with_height(Some(grid.clone()))
                        .with_ground(Some(crate::ground_field::GroundField::uniform(
                            crate::ground_field::ContactGround {
                                grip: soil,
                                rolling_resistance: soil,
                            },
                        )));
                    let report = contact_side(
                        &input,
                        SideState::default(),
                        Affine3A::from_rotation_translation(block.rotation, top),
                        1.0 / 64.0,
                        &params,
                        &field,
                        &pieces,
                        |_| block.rotation * Vec3::Z,
                        &mut GripElements::for_links(24),
                    );
                    assert!(
                        !report.contacts.is_empty(),
                        "the fixture pad bears on the graybox"
                    );
                    for contact in report.contacts {
                        assert_eq!(contact.collider, Some(entity));
                        assert_eq!(contact.mu, pair);
                        let rolling = contact.rolling_force.length() / contact.load;
                        assert!((rolling - rigid_rolling_resistance()).abs() < 1e-6);
                    }
                }
            })
            .unwrap();
    }

    /// The game's environment destruction model must build complete simulation state without a
    /// renderer. Map spawning and the contact laws rely on one common body per construction,
    /// one collider per source element, and no construction in the immutable terrain field.
    #[test]
    fn spawn_builds_complete_constructions_without_view_assets() {
        let manifest = construction_fixture_manifest();
        let (mut app, obstacles) = spawned(&manifest, false, false);
        let world = app.world_mut();
        let mut expected_elements = 0;
        let mut expected_bodies = 0;
        for instance in &manifest.instances {
            if let Prototype::Construction { asset } = &manifest.prototypes[&instance.prototype] {
                let kind = manifest.constructions.get(asset).unwrap();
                expected_elements += kind.asset.elements.len();
                expected_bodies += 1;
            }
        }
        let proxies = manifest.instances.len() - expected_bodies;
        assert_eq!(obstacles.len(), manifest.instances.len());
        assert_eq!(
            world.query::<&RigidBody>().iter(world).count(),
            manifest.instances.len()
        );
        let mut identities: Vec<_> = world
            .query::<&MapInstance>()
            .iter(world)
            .map(|instance| (instance.id.clone(), instance.ordinal))
            .collect();
        identities.sort();
        assert_eq!(identities.len(), manifest.instances.len());
        assert!(
            identities
                .iter()
                .enumerate()
                .all(|(k, (_, ordinal))| usize::from(*ordinal) == k)
        );
        let roots: Vec<_> = world
            .query::<(&MapInstance, &crate::construction::breakage::Breakage)>()
            .iter(world)
            .collect();
        assert_eq!(roots.len(), expected_bodies);
        for (identity, body) in roots {
            assert_eq!(body.construction, identity.ordinal);
            assert_eq!(
                body.live.state.pool,
                body.live
                    .structure
                    .chunks
                    .iter()
                    .map(crate::construction::fracture::full_pool)
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(
            world
                .query_filtered::<Entity, (With<crate::construction::Element>, With<Collider>)>()
                .iter(world)
                .count(),
            expected_elements
        );
        assert_eq!(
            world.query::<&Collider>().iter(world).count(),
            expected_elements + proxies
        );
        assert_eq!(
            world
                .query_filtered::<Entity, (With<ScatterProxy>, With<Collider>)>()
                .iter(world)
                .count(),
            proxies,
            "the surface-evidence cast sees every graybox proxy",
        );
        assert_eq!(world.query::<&Mesh3d>().iter(world).count(), 0);
    }

    /// The game's environment destruction view shares a skin-and-card mesh per admitted kind and
    /// one surface material. Each view and physical root keeps the authored geometry.
    #[test]
    fn drawing_shares_each_constructions_authored_mesh() {
        let mut manifest = construction_fixture_manifest();
        let originals = manifest.instances.clone();
        for mut second in originals {
            if matches!(
                &manifest.prototypes[&second.prototype],
                Prototype::Construction { .. }
            ) {
                second.id.push_str("_copy");
                second.translation[0] += 30.0; // Fixture separation.
                manifest.instances.push(second);
            }
        }
        let (mut app, _) = spawned(&manifest, true, false);
        let world = app.world_mut();
        let views: Vec<_> = world
            .query_filtered::<(
                &ChildOf,
                &Transform,
                &Mesh3d,
                &MeshMaterial3d<ConstructionMaterial>,
            ), With<crate::construction::view::View>>()
            .iter(world)
            .map(|(parent, transform, mesh, material)| {
                (parent.parent(), transform.scale, mesh.id(), material.id())
            })
            .collect();
        let mut by_kind: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for (root, scale, mesh, material) in views {
            let body = world
                .get::<crate::construction::breakage::Breakage>(root)
                .unwrap();
            assert_eq!(scale, Vec3::ONE);
            assert_eq!(world.get::<Transform>(root).unwrap().scale, Vec3::ONE);
            by_kind
                .entry(body.kind.asset.id.clone())
                .or_default()
                .push((scale.x, mesh, material));
        }
        let expected_kinds: std::collections::BTreeSet<_> = manifest
            .prototypes
            .values()
            .filter_map(|prototype| match prototype {
                Prototype::Construction { asset } => Some(asset),
                _ => None,
            })
            .collect();
        assert_eq!(by_kind.len(), expected_kinds.len());
        for views in by_kind.values_mut() {
            views.sort_by(|a, b| a.0.total_cmp(&b.0));
            assert_eq!(views.len(), 2);
            assert!(views.iter().all(|view| view.0 == 1.0));
            assert_eq!(views[0].1, views[1].1);
            assert_eq!(views[0].2, views[1].2);
        }
        assert_eq!(world.resource::<Assets<ConstructionMaterial>>().len(), 1);
    }
}
