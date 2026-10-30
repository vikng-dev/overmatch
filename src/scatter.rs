//! The map's object scatter: the authored placement of buildings and trees, out of the map's own
//! manifest ([`crate::map`]), spawned with static colliders on every composition, the dedicated
//! server included — a graybox building or fir as a GRAYBOX proxy (a box per building, a trunk
//! cylinder plus a canopy cone per fir), a tree or a building of the damage model as its own body
//! (below). A graybox building's box is drawn in brick under a timber roof
//! ([`crate::building_surface`]), its shape still the collider's.
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
//! a tree's state on the wire — names it by a number both peers derived from the same bytes.
//!
//! THE FILE'S Y IS STALE (authored against an older sculpt): every instance is re-projected onto
//! the live [`HeightGrid`] at its own XZ. Yaw and scale are taken as authored.
//!
//! Two contact laws, because a belt and a hull meet these differently: a graybox building is a
//! cuboid collider AND a [`crate::world::TerrainMap`] block, so the track field senses it exactly
//! like the authored course's boxes; a fir is a trunk cylinder collider only, never a block — the
//! belts must not climb a trunk, and the hull collision is the honest interaction. A tree of the
//! damage model ([`crate::trees`]) is the fir's successor: a body the walk doses and a hull can
//! fell, spawned there from its own data, never a block either. A building of the destruction
//! model ([`crate::buildings`]) is the graybox building's successor: its walls are hull colliders
//! and matter the walk crosses, never a block (playtest fork F9), and its bounds join the spawn
//! obstacles instead.

use std::collections::BTreeMap;

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
use bevy::prelude::*;

use crate::Layer;
use crate::building_surface::BuildingMaterial;
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

/// Marks a scatter proxy's static collider — every house cuboid and fir trunk, and nothing else
/// (the canopy cone is view-only geometry and carries no collider). Spawned in every composition,
/// server included, so the tag rides with the shape rather than with a window; the client's
/// scatter-hit read ([`crate::vfx`]) filters its casts to entities carrying it.
#[derive(Component)]
pub(crate) struct ScatterProxy;

/// Fir trunk radius (m) at instance scale 1 — the cylinder collider's radius and the trunk mesh's.
const TRUNK_RADIUS_M: f32 = 0.4;

/// Height fraction of a fir at which the canopy cone's base sits; bare trunk below it.
const CANOPY_BASE_FRACTION: f32 = 0.2;

/// Smallest squared quaternion length [`resolve`] will normalize. Far below any rounding an f32
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
    scale: Vec3,
    /// The file's own instance id — the sort key that fixes iteration order.
    id: &'a str,
}

/// One placed instance: its prototype, its identity and its GROUND pose — the authored XZ and
/// rotation, the Y re-sampled from the grid, the authored scale.
#[derive(Debug, Clone, PartialEq)]
struct Placement<'a> {
    prototype: &'a str,
    shape: &'a Prototype,
    identity: MapInstance,
    pose: Transform,
}

impl Placement<'_> {
    /// A building's [`crate::world::TerrainMap`] block form: a unit cube posed and scaled (the
    /// Avian idiom `world::spawn_block` uses), its centre lifted off the ground pose by the
    /// prototype's own bounds. `None` for a fir or a tree — a trunk is not a block.
    fn block(&self) -> Option<Transform> {
        match self.shape {
            Prototype::Building { min, max } => {
                let (min, max) = (Vec3::from_array(*min), Vec3::from_array(*max));
                Some(Transform {
                    translation: self.pose.translation
                        + self.pose.rotation * ((min + max) * 0.5 * self.pose.scale),
                    rotation: self.pose.rotation,
                    scale: (max - min) * self.pose.scale,
                })
            }
            Prototype::Fir { .. } | Prototype::Tree { .. } | Prototype::Structure { .. } => None,
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
            let scale = Vec3::from_array(raw.scale);
            assert!(
                translation.is_finite() && rotation.is_finite() && scale.is_finite(),
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
                scale,
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
            pose: Transform {
                translation: Vec3::new(
                    instance.xz.x,
                    grid.height_at(instance.xz.x, instance.xz.y),
                    instance.xz.y,
                ),
                rotation: instance.rotation,
                scale: instance.scale,
            },
        })
        .collect()
}

/// What a composition that draws hands the scatter: the server the trees' glb views and the
/// buildings' maps load through, and the store the buildings' one material goes into.
pub(crate) struct ViewAssets<'a> {
    pub(crate) server: &'a AssetServer,
    pub(crate) building_materials: &'a mut Assets<BuildingMaterial>,
}

/// The view-side handles, built ONCE per world: one mesh per prototype, cloned per instance, and
/// one material per surface. Only built where there is a window to draw into.
struct ScatterView {
    /// The unit cube per building prototype, keyed by its id: the cube the block transform scales,
    /// the same encoding the block list uses, its faces framed for the building surface
    /// ([`crate::building_surface::unit_box`]).
    building: BTreeMap<String, Handle<Mesh>>,
    /// `(trunk, canopy)` per fir prototype, keyed by its id.
    fir: BTreeMap<String, (Handle<Mesh>, Handle<Mesh>)>,
    /// One merged mesh per building asset of the destruction model, keyed by the asset id: its
    /// elements' faces in the building frame ([`crate::buildings::view::mesh`]), shared by every
    /// instance.
    structure: BTreeMap<String, Handle<Mesh>>,
    /// The ONE material every building wears, graybox or not: no instance gets its own, so the
    /// buildings batch.
    surface: Handle<BuildingMaterial>,
    bark: Handle<StandardMaterial>,
    needle: Handle<StandardMaterial>,
}

impl ScatterView {
    fn new(
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        assets: &mut ViewAssets<'_>,
        manifest: &MapManifest,
    ) -> Self {
        let library = &manifest.prototypes;
        Self {
            building: library
                .iter()
                .filter_map(|(id, shape)| match *shape {
                    Prototype::Building { min, max } => Some((
                        id.clone(),
                        meshes.add(crate::building_surface::unit_box(
                            crate::building_surface::ridge_axis(min, max),
                        )),
                    )),
                    Prototype::Fir { .. }
                    | Prototype::Tree { .. }
                    | Prototype::Structure { .. } => None,
                })
                .collect(),
            fir: library
                .iter()
                .filter_map(|(id, shape)| match *shape {
                    Prototype::Fir {
                        height_m,
                        canopy_diameter_m,
                    } => Some((
                        id.clone(),
                        (
                            meshes.add(Cylinder::new(TRUNK_RADIUS_M, height_m)),
                            meshes.add(Cone {
                                radius: canopy_diameter_m / 2.0,
                                height: height_m * (1.0 - CANOPY_BASE_FRACTION),
                            }),
                        ),
                    )),
                    Prototype::Building { .. }
                    | Prototype::Tree { .. }
                    | Prototype::Structure { .. } => None,
                })
                .collect(),
            structure: library
                .values()
                .filter_map(|shape| match shape {
                    Prototype::Structure { asset } => Some(asset),
                    Prototype::Building { .. } | Prototype::Fir { .. } | Prototype::Tree { .. } => {
                        None
                    }
                })
                .map(|asset| {
                    let kind = manifest
                        .buildings
                        .get(asset)
                        .expect("map::load loaded every building asset the library names");
                    (
                        asset.clone(),
                        meshes.add(crate::buildings::view::mesh(&kind.asset)),
                    )
                })
                .collect(),
            surface: crate::building_surface::material(
                commands,
                assets.server,
                assets.building_materials,
            ),
            bark: materials.add(Color::srgb(0.26, 0.19, 0.13)),
            needle: materials.add(Color::srgb(0.11, 0.21, 0.13)),
        }
    }
}

/// Spawn the scatter into the world the grid describes, appending every graybox building to
/// `blocks` (the [`crate::world::TerrainMap`] list the analytic track field is built from), and
/// return every building of the destruction model's bounds in that list's encoding: the spawn
/// obstacles ([`crate::world::SpawnObstacles`]), never the track field's. Colliders spawn in every
/// composition; meshes and the trees' glb views only where `view_assets` offers a server to draw
/// with — the dedicated server draws nothing, exactly as it skips the terrain's render tiles.
pub(crate) fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    manifest: &MapManifest,
    grid: &HeightGrid,
    blocks: &mut Vec<Transform>,
    mut view_assets: Option<ViewAssets<'_>>,
) -> Vec<Transform> {
    let placements = place(&resolve(&manifest.instances, &manifest.prototypes), grid);
    let view = view_assets
        .as_mut()
        .map(|assets| ScatterView::new(commands, meshes, materials, assets, manifest));
    if let Some(view) = &view {
        // The breaks' view draws a building's new meshes in the same one material.
        commands.insert_resource(crate::buildings::view::BuildingLook {
            surface: view.surface.clone(),
        });
    }
    let assets = view_assets.map(|assets| assets.server);
    let statics = CollisionLayers::new([Layer::Terrain], LayerMask::ALL);
    let (mut buildings, mut firs, mut trees, mut structures) = (0usize, 0usize, 0usize, 0usize);
    let mut obstacles = Vec::new();
    for placement in &placements {
        match placement.shape {
            Prototype::Structure { asset } => {
                let kind = manifest
                    .buildings
                    .get(asset)
                    .expect("map::load loaded every building asset the library names");
                let root = crate::buildings::spawn(
                    commands,
                    kind,
                    placement.identity.clone(),
                    placement.pose,
                );
                obstacles.push(crate::buildings::footprint(kind, placement.pose));
                if let Some(view) = &view {
                    // The root stands at unit scale; the view, a child, carries the instance's.
                    commands.spawn((
                        ChildOf(root),
                        Transform::from_scale(placement.pose.scale),
                        Mesh3d(view.structure[asset].clone()),
                        MeshMaterial3d(view.surface.clone()),
                        crate::render_cost::CasterClass::House,
                        crate::buildings::view::BuildingView,
                    ));
                }
                structures += 1;
            }
            Prototype::Tree { asset } => {
                let kind = manifest
                    .trees
                    .get(asset)
                    .expect("map::load loaded every tree asset the library names");
                let root =
                    crate::trees::spawn(commands, kind, placement.identity.clone(), placement.pose);
                if let Some(assets) = assets {
                    crate::trees::view::attach(
                        commands,
                        root,
                        kind,
                        placement.pose.scale.x,
                        assets,
                    );
                }
                trees += 1;
            }
            Prototype::Building { .. } => {
                let block = placement
                    .block()
                    .expect("a building placement carries a block transform");
                blocks.push(block);
                let mut entity = commands.spawn((
                    block,
                    RigidBody::Static,
                    Collider::cuboid(1.0, 1.0, 1.0),
                    statics,
                    ScatterProxy,
                    placement.identity.clone(),
                ));
                if let Some(view) = &view {
                    entity.insert((
                        Mesh3d(view.building[placement.prototype].clone()),
                        MeshMaterial3d(view.surface.clone()),
                        crate::render_cost::CasterClass::House,
                    ));
                }
                buildings += 1;
            }
            Prototype::Fir {
                height_m,
                canopy_diameter_m: _,
            } => {
                let (pose, height_m) = (placement.pose, *height_m);
                // Both primitives are centred on their own origin, so each rides at its own centre
                // height above the ground pose, scaled with the instance.
                let at = |centre_m: f32| Transform {
                    translation: pose.translation + Vec3::Y * (centre_m * pose.scale.y),
                    rotation: pose.rotation,
                    scale: pose.scale,
                };
                let trunk = at(height_m / 2.0);
                let mut entity = commands.spawn((
                    trunk,
                    RigidBody::Static,
                    Collider::cylinder(TRUNK_RADIUS_M, height_m),
                    statics,
                    ScatterProxy,
                    placement.identity.clone(),
                ));
                if let Some(view) = &view {
                    let (trunk_mesh, canopy_mesh) = &view.fir[placement.prototype];
                    entity.insert((
                        Mesh3d(trunk_mesh.clone()),
                        crate::render_cost::CasterClass::Tree,
                        MeshMaterial3d(view.bark.clone()),
                    ));
                    // The canopy is view-only and spawns FLAT rather than as a child: nothing
                    // reads it, so it needs no hierarchy to propagate.
                    commands.spawn((
                        at(height_m * (1.0 + CANOPY_BASE_FRACTION) / 2.0),
                        Mesh3d(canopy_mesh.clone()),
                        crate::render_cost::CasterClass::Tree,
                        MeshMaterial3d(view.needle.clone()),
                    ));
                }
                firs += 1;
            }
        }
    }
    info!(
        "scatter: {} instances placed on the live surface — {buildings} graybox buildings (also \
         TerrainMap blocks), {structures} buildings (a collider per element), {firs} graybox firs \
         (trunk colliders only), {trees} trees",
        placements.len(),
    );
    obstacles
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::map::tests::mixed_fixture_manifest;

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

    /// A one-building library (fixture data).
    fn library() -> BTreeMap<String, Prototype> {
        BTreeMap::from([("house_proxy".to_owned(), house())])
    }

    fn house() -> Prototype {
        Prototype::Building {
            min: [-2.0, -0.5, -3.0],
            max: [2.0, 2.5, 3.0],
        }
    }

    /// One instance record of `prototype`, posed with a deliberately WRONG Y.
    fn record(id: &str, xz: Vec2, stale_y: f32) -> InstanceRecord {
        InstanceRecord {
            prototype: "house_proxy".to_owned(),
            translation: [xz.x, stale_y, xz.y],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
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

    /// `manifest` spawned on the sloped grid into a fresh app with an asset server, the block list
    /// it filled and the obstacles it returned; `draw` hands the scatter that server, as a windowed
    /// composition does.
    fn spawned(manifest: &MapManifest, draw: bool) -> (App, Vec<Transform>, Vec<Transform>) {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .init_asset::<BuildingMaterial>()
        .init_asset::<bevy::world_serialization::WorldAsset>();
        let grid = sloped_grid();
        let mut blocks: Vec<Transform> = Vec::new();
        let mut obstacles: Vec<Transform> = Vec::new();
        let server = draw.then(|| app.world().resource::<AssetServer>().clone());
        let world = app.world_mut();
        world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
            world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
                world.resource_scope(|world, mut buildings: Mut<Assets<BuildingMaterial>>| {
                    let mut queue = bevy::ecs::world::CommandQueue::default();
                    let mut commands = Commands::new(&mut queue, world);
                    obstacles = spawn(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        manifest,
                        &grid,
                        &mut blocks,
                        server.as_ref().map(|server| ViewAssets {
                            server,
                            building_materials: &mut buildings,
                        }),
                    );
                    queue.apply(world);
                });
            });
        });
        (app, blocks, obstacles)
    }

    /// EVERY BUILDING WEARS THE ONE MATERIAL, the draw-count invariant the brick-and-timber surface
    /// rides on: a second house — another id, another scale (fixture data) — draws with the first
    /// one's mesh and material handles, and the world holds one building material in all. A view
    /// that minted either per instance would split the buildings into a draw each.
    #[test]
    fn every_building_draws_with_one_mesh_and_one_material() {
        let mut manifest = mixed_fixture_manifest();
        let mut second = manifest
            .instances
            .iter()
            .find(|instance| {
                matches!(
                    manifest.prototypes[&instance.prototype],
                    Prototype::Building { .. }
                )
            })
            .expect("the fixture places a house")
            .clone();
        second.id = "house_0002".to_owned();
        second.translation[0] += 30.0;
        second.scale = [0.6; 3];
        manifest.instances.push(second);
        let (mut app, blocks, _) = spawned(&manifest, true);
        assert_eq!(blocks.len(), 2, "both houses are blocks");
        let world = app.world_mut();
        let drawn: Vec<(AssetId<Mesh>, AssetId<BuildingMaterial>)> = world
            .query_filtered::<(&Mesh3d, &MeshMaterial3d<BuildingMaterial>), Without<ChildOf>>()
            .iter(world)
            .map(|(mesh, material)| (mesh.id(), material.id()))
            .collect();
        assert_eq!(drawn.len(), 2, "both graybox houses draw");
        assert_eq!(drawn[0], drawn[1], "one mesh and one material for both");
        assert_eq!(
            world.resource::<Assets<BuildingMaterial>>().len(),
            1,
            "one building material in the world",
        );
    }

    /// The spawn itself, in a real world: every graybox building reaches the block list and every
    /// building of the destruction model the obstacles alone, every instance is one static body
    /// carrying its identity — a proxy collider for a graybox building or fir, a root with a
    /// collider per segment for a tree and one per element for a building — and a windowless
    /// composition spawns no mesh and no view. A structure that entered the block list would hand
    /// the belts a wall to climb (playtest fork F9).
    #[test]
    fn spawn_adds_a_body_per_instance_and_a_block_per_building() {
        let manifest = mixed_fixture_manifest();
        let (mut app, blocks, obstacles) = spawned(&manifest, false);
        let world = app.world_mut();
        let (mut buildings, mut proxies, mut trees, mut segments) = (0, 0, 0, 0);
        let (mut structures, mut elements) = (0, 0);
        for instance in &manifest.instances {
            match &manifest.prototypes[&instance.prototype] {
                Prototype::Building { .. } => {
                    buildings += 1;
                    proxies += 1;
                }
                Prototype::Fir { .. } => proxies += 1,
                Prototype::Tree { asset } => {
                    trees += 1;
                    segments += manifest.trees.get(asset).unwrap().segments.len();
                }
                Prototype::Structure { asset } => {
                    structures += 1;
                    elements += manifest.buildings.get(asset).unwrap().asset.elements.len();
                }
            }
        }
        assert_eq!(structures, 1, "the fixture places a building");
        assert_eq!(obstacles.len(), structures, "one obstacle per building");
        assert_eq!(
            (buildings, proxies, trees),
            (1, 2, 1),
            "the fixture places a house, a graybox fir and a tree"
        );
        assert_eq!(blocks.len(), buildings, "one block per building, no trunk");
        let bodies = world
            .query_filtered::<Entity, With<RigidBody>>()
            .iter(world)
            .count();
        assert_eq!(
            bodies,
            manifest.instances.len(),
            "one static body per instance"
        );
        let mut identities: Vec<(String, u16)> = world
            .query::<&MapInstance>()
            .iter(world)
            .map(|instance| (instance.id.clone(), instance.ordinal))
            .collect();
        identities.sort();
        assert_eq!(identities.len(), bodies, "every body carries its identity");
        assert!(
            identities
                .iter()
                .enumerate()
                .all(|(k, (_, ordinal))| usize::from(*ordinal) == k),
            "ordinals are the id order, dense from zero",
        );
        // The view read casts against the marker, not against the terrain layer at large: a
        // proxy collider without it is invisible to the scatter-hit effect, so the tag is on every
        // proxy and on no tree (the walk marks a tree's hits itself).
        let tagged = world
            .query_filtered::<Entity, (With<ScatterProxy>, With<Collider>)>()
            .iter(world)
            .count();
        assert_eq!(tagged, proxies, "every proxy collider is a ScatterProxy");
        let tree_roots = world
            .query_filtered::<Entity, (With<crate::trees::TreeBody>, Without<ScatterProxy>)>()
            .iter(world)
            .count();
        assert_eq!(tree_roots, trees, "one untagged root per tree");
        let tree_colliders = world
            .query_filtered::<Entity, (With<crate::trees::TreeSegment>, With<Collider>)>()
            .iter(world)
            .count();
        assert_eq!(tree_colliders, segments, "one collider per tree segment");
        let element_colliders = world
            .query_filtered::<Entity, (With<crate::buildings::BuildingElement>, With<Collider>)>()
            .iter(world)
            .count();
        assert_eq!(
            element_colliders, elements,
            "one collider per building element"
        );
        assert_eq!(
            world.query::<&Collider>().iter(world).count(),
            proxies + segments + elements,
            "no other collider ships",
        );
        let views = world
            .query::<&crate::trees::view::TreeView>()
            .iter(world)
            .count();
        assert_eq!(views, 0, "a windowless composition attaches no view");
        let meshes = world.query::<&Mesh3d>().iter(world).count();
        assert_eq!(meshes, 0, "a windowless composition draws nothing");
    }

    /// EVERY BUILDING OF ONE ASSET DRAWS ONE SHARED MESH IN THE ONE MATERIAL: a second instance of
    /// the fixture's brick house at another scale (1.3, fixture data) draws, as a child of its
    /// root at its own scale, with the first one's mesh, and both wear the material the graybox
    /// houses wear. A view that built a mesh per instance (74 on Kalinovo) or a material per house
    /// splits the buildings into a draw each; one at the root's unit scale draws a scaled house at
    /// asset size, its walls off its colliders.
    #[test]
    fn every_building_of_one_asset_draws_one_mesh_at_its_instance_scale() {
        let mut manifest = mixed_fixture_manifest();
        let mut second = manifest
            .instances
            .iter()
            .find(|instance| {
                matches!(
                    manifest.prototypes[&instance.prototype],
                    Prototype::Structure { .. }
                )
            })
            .expect("the fixture places a building")
            .clone();
        second.id = "brick_0002".to_owned();
        second.translation[0] += 30.0;
        second.scale = [1.3; 3];
        manifest.instances.push(second);
        let (mut app, _, _) = spawned(&manifest, true);
        let world = app.world_mut();
        let views: Vec<(Entity, Vec3, AssetId<Mesh>, AssetId<BuildingMaterial>)> = world
            .query::<(
                &ChildOf,
                &Transform,
                &Mesh3d,
                &MeshMaterial3d<BuildingMaterial>,
            )>()
            .iter(world)
            .map(|(parent, transform, mesh, material)| {
                (parent.parent(), transform.scale, mesh.id(), material.id())
            })
            .collect();
        assert_eq!(views.len(), 2, "one view per building");
        assert_eq!(views[0].2, views[1].2, "one mesh for the asset");
        assert_eq!(views[0].3, views[1].3, "one material");
        let mut scales: Vec<f32> = views
            .iter()
            .map(|&(root, scale, ..)| {
                assert!(world.get::<crate::buildings::BuildingBody>(root).is_some());
                assert_eq!(scale, Vec3::splat(scale.x));
                scale.x
            })
            .collect();
        scales.sort_by(f32::total_cmp);
        assert_eq!(scales, [1.0, 1.3], "each at its instance's scale");
        assert_eq!(world.resource::<Assets<BuildingMaterial>>().len(), 1);
    }

    /// A COMPOSITION THAT DRAWS ATTACHES EACH TREE'S VIEW: the sim spawns a tree's body only, so
    /// the scatter, handed an asset server, puts one glb view under each tree's root at that
    /// instance's scale (the fixture's tree rescaled to 1.5, fixture data, so a view at unit
    /// scale is told apart). A scatter that leaves the view to the sim, which spawns none, draws
    /// no tree at all.
    #[test]
    fn a_drawing_composition_attaches_one_view_per_tree_at_its_scale() {
        let mut manifest = mixed_fixture_manifest();
        let mut trees = 0;
        for instance in &mut manifest.instances {
            if let Prototype::Tree { .. } = manifest.prototypes[&instance.prototype] {
                instance.scale = [1.5; 3];
                trees += 1;
            }
        }
        let (mut app, _, _) = spawned(&manifest, true);
        let world = app.world_mut();
        let views: Vec<(Entity, Vec3)> = world
            .query_filtered::<(&ChildOf, &Transform), With<crate::trees::view::TreeView>>()
            .iter(world)
            .map(|(parent, transform)| (parent.parent(), transform.scale))
            .collect();
        assert_eq!(views.len(), trees, "one view per tree");
        for (root, scale) in views {
            let body = world
                .get::<crate::trees::TreeBody>(root)
                .expect("a view hangs under its tree's root");
            assert_eq!(
                scale,
                Vec3::splat(body.scale),
                "the view is at the instance's scale"
            );
            assert_eq!(body.scale, 1.5);
        }
    }
}
