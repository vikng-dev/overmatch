//! The map's object scatter: the authored placement of buildings and trees, out of the map's own
//! manifest ([`crate::map`]), spawned as GRAYBOX proxies — a box per building, a trunk cylinder
//! plus a canopy cone per fir — with static colliders on every composition, the dedicated server
//! included.
//!
//! SHARED DATA, NOT REPLICATION — the same stance the terrain takes: both binaries read the same
//! shipped file and derive the same entities, so no scatter entity crosses the wire. The
//! derivation is order-fixed (instances sorted by their own `id` before placement) and pure f32,
//! so client and server land on identical transforms.
//!
//! THE LIBRARY IS THE GAME'S, THE PLACEMENT IS THE MAP'S. [`PROTOTYPES`] is a code table keyed by
//! the prototype id strings the file references; the file's own `prototypes` block — whose `asset`
//! and `included` fields describe the author's shipping state, not ours — is never read. An
//! instance naming an id the table does not carry is a broken ship and panics (ADR-0011).
//!
//! THE FILE'S Y IS STALE (authored against an older sculpt): every instance is re-projected onto
//! the live [`HeightGrid`] at its own XZ. Yaw and scale are taken as authored.
//!
//! Two contact laws, because a belt and a hull meet these differently: a building is a cuboid
//! collider AND a [`crate::world::TerrainMap`] block, so the track field senses it exactly like the
//! authored course's boxes; a fir is a trunk cylinder collider only, never a block — the belts must
//! not climb a trunk, and the hull collision is the honest interaction.

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
use bevy::prelude::*;

use crate::Layer;
use crate::map::{InstanceRecord, MapManifest};
use crate::terrain_grid::HeightGrid;

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

/// The graybox shape one prototype id resolves to. Dimensions are metres at instance scale 1; the
/// instance's own scale multiplies both the visual and the collider.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Proxy {
    /// A building: an axis-aligned box in the prototype's local frame, `min`/`max` bounds around
    /// the instance origin. A `min.y` below zero is a skirt that sinks into the surface.
    Building { min: Vec3, max: Vec3 },
    /// A fir: a trunk cylinder of [`TRUNK_RADIUS_M`] over the full `height_m`, and a canopy cone of
    /// `canopy_diameter_m` reaching from [`CANOPY_BASE_FRACTION`] of that height to the apex.
    Fir {
        height_m: f32,
        canopy_diameter_m: f32,
    },
}

/// The graybox prototype library, keyed by the id the map references. Order is the lookup's only
/// structure — [`Placement`] indexes into this table, so entries may be added but an existing
/// entry's id and shape are what the shipped map resolves against.
const PROTOTYPES: &[(&str, Proxy)] = &[
    (
        "house_proxy",
        Proxy::Building {
            min: Vec3::new(-2.0, -0.5, -3.0),
            max: Vec3::new(2.0, 2.5, 3.0),
        },
    ),
    (
        "church_proxy",
        Proxy::Building {
            min: Vec3::new(-15.0, -18.5, -7.5),
            max: Vec3::new(15.0, 18.5, 7.5),
        },
    ),
    (
        "fir_tree_01_a_LOD2",
        Proxy::Fir {
            height_m: 18.96,
            canopy_diameter_m: 6.5,
        },
    ),
    (
        "fir_tree_01_b_LOD2",
        Proxy::Fir {
            height_m: 14.08,
            canopy_diameter_m: 5.8,
        },
    ),
    (
        "fir_tree_01_c_LOD2",
        Proxy::Fir {
            height_m: 14.55,
            canopy_diameter_m: 6.2,
        },
    ),
];

/// Index into [`PROTOTYPES`] for a prototype id.
fn prototype_index(id: &str) -> Option<usize> {
    PROTOTYPES.iter().position(|(name, _)| *name == id)
}

/// One parsed instance: the registry slot it resolves to and its authored pose MINUS the stale Y.
struct Instance {
    prototype: usize,
    /// Authored world XZ. The file's Y never leaves [`parse`].
    xz: Vec2,
    rotation: Quat,
    scale: Vec3,
    /// The file's own instance id — the sort key that fixes iteration order.
    id: String,
}

/// One placed instance: its registry slot and its GROUND pose — the authored XZ and rotation, the
/// Y re-sampled from the grid, the authored scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    prototype: usize,
    pose: Transform,
}

impl Placement {
    fn proxy(&self) -> Proxy {
        PROTOTYPES[self.prototype].1
    }

    /// A building's [`crate::world::TerrainMap`] block form: a unit cube posed and scaled (the
    /// Avian idiom `world::spawn_block` uses), its centre lifted off the ground pose by the
    /// prototype's own bounds. `None` for a fir — a trunk is not a block.
    fn block(&self) -> Option<Transform> {
        match self.proxy() {
            Proxy::Building { min, max } => Some(Transform {
                translation: self.pose.translation
                    + self.pose.rotation * ((min + max) * 0.5 * self.pose.scale),
                rotation: self.pose.rotation,
                scale: (max - min) * self.pose.scale,
            }),
            Proxy::Fir { .. } => None,
        }
    }
}

/// Resolve the manifest's instance records against the prototype registry and put them in
/// ITERATION ORDER. Everything past the manifest's own shape check is ADR-0011 fail-fast: an
/// unknown prototype id, a pose component that is not finite (a NaN reaches the solver as a NaN
/// collider), or a rotation with no length to normalize is a broken ship and panics naming the
/// instance.
fn resolve(records: &[InstanceRecord]) -> Vec<Instance> {
    let mut instances: Vec<Instance> = records
        .iter()
        .map(|raw| {
            let prototype = prototype_index(&raw.prototype).unwrap_or_else(|| {
                panic!(
                    "scatter: instance {} references prototype {:?}, which the graybox registry \
                     does not carry",
                    raw.id, raw.prototype,
                )
            });
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
                prototype,
                // The Y is DROPPED here, where the stale value cannot leak further.
                xz: translation.xz(),
                rotation: rotation.normalize(),
                scale,
                id: raw.id.clone(),
            }
        })
        .collect();
    // The file's own ids are the fixed order every peer spawns in.
    instances.sort_by(|a, b| a.id.cmp(&b.id));
    instances
}

/// Project every instance onto `grid`: the authored XZ and rotation kept, the Y taken from the
/// surface under it. Pure and order-preserving, so two runs over the same file and grid produce
/// the same list.
fn place(instances: &[Instance], grid: &HeightGrid) -> Vec<Placement> {
    instances
        .iter()
        .map(|instance| Placement {
            prototype: instance.prototype,
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

/// The view-side handles, built ONCE per world: one mesh per prototype, cloned per instance, and
/// one flat material per surface. Only built where there is a window to draw into.
struct ScatterView {
    /// The unit cube every building box is a scaled copy of — the same encoding the block list uses.
    cube: Handle<Mesh>,
    /// `(trunk, canopy)` per fir prototype, indexed like [`PROTOTYPES`]; `None` on a building slot.
    fir: Vec<Option<(Handle<Mesh>, Handle<Mesh>)>>,
    wall: Handle<StandardMaterial>,
    bark: Handle<StandardMaterial>,
    needle: Handle<StandardMaterial>,
}

impl ScatterView {
    fn new(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Self {
        Self {
            cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            fir: PROTOTYPES
                .iter()
                .map(|(_, proxy)| match *proxy {
                    Proxy::Fir {
                        height_m,
                        canopy_diameter_m,
                    } => Some((
                        meshes.add(Cylinder::new(TRUNK_RADIUS_M, height_m)),
                        meshes.add(Cone {
                            radius: canopy_diameter_m / 2.0,
                            height: height_m * (1.0 - CANOPY_BASE_FRACTION),
                        }),
                    )),
                    Proxy::Building { .. } => None,
                })
                .collect(),
            wall: materials.add(Color::srgb(0.62, 0.58, 0.53)),
            bark: materials.add(Color::srgb(0.26, 0.19, 0.13)),
            needle: materials.add(Color::srgb(0.11, 0.21, 0.13)),
        }
    }
}

/// Spawn the scatter into the world the grid describes, appending every building to `blocks` (the
/// [`crate::world::TerrainMap`] list the analytic track field is built from). Colliders spawn in
/// every composition; meshes only when `windowed` — the dedicated server draws nothing, exactly as
/// it skips the terrain's render tiles.
pub(crate) fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    manifest: &MapManifest,
    grid: &HeightGrid,
    blocks: &mut Vec<Transform>,
    windowed: bool,
) {
    let placements = place(&resolve(&manifest.instances), grid);
    let view = windowed.then(|| ScatterView::new(meshes, materials));
    let statics = CollisionLayers::new([Layer::Terrain], LayerMask::ALL);
    let (mut buildings, mut firs) = (0usize, 0usize);
    for placement in &placements {
        match placement.proxy() {
            Proxy::Building { .. } => {
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
                ));
                if let Some(view) = &view {
                    entity.insert((Mesh3d(view.cube.clone()), MeshMaterial3d(view.wall.clone())));
                }
                buildings += 1;
            }
            Proxy::Fir {
                height_m,
                canopy_diameter_m: _,
            } => {
                let pose = placement.pose;
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
                ));
                if let Some(view) = &view {
                    let (trunk_mesh, canopy_mesh) = view.fir[placement.prototype]
                        .as_ref()
                        .expect("a fir prototype carries fir meshes");
                    entity.insert((
                        Mesh3d(trunk_mesh.clone()),
                        MeshMaterial3d(view.bark.clone()),
                    ));
                    // The canopy is view-only and spawns FLAT rather than as a child: nothing
                    // reads it, so it needs no hierarchy to propagate.
                    commands.spawn((
                        at(height_m * (1.0 + CANOPY_BASE_FRACTION) / 2.0),
                        Mesh3d(canopy_mesh.clone()),
                        MeshMaterial3d(view.needle.clone()),
                    ));
                }
                firs += 1;
            }
        }
    }
    info!(
        "scatter: {} instances placed on the live surface — {buildings} buildings (also \
         TerrainMap blocks), {firs} firs (trunk colliders only)",
        placements.len(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::map::tests::{shipped_assets, shipped_manifest};

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

    /// One instance record of `prototype`, posed with a deliberately WRONG Y.
    fn record(prototype: &str, xz: Vec2, stale_y: f32) -> InstanceRecord {
        InstanceRecord {
            prototype: prototype.to_owned(),
            translation: [xz.x, stale_y, xz.y],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            id: "a".to_owned(),
        }
    }

    /// THE determinism claim: resolving and placing the SHIPPED manifest twice yields the same
    /// ordered list of transforms. Both peers run this derivation instead of replicating the
    /// result, so a difference here is a desync.
    #[test]
    fn two_parses_place_the_same_ordered_transforms() {
        let grid = sloped_grid();
        let records = shipped_manifest().instances;
        let once = place(&resolve(&records), &grid);
        let twice = place(&resolve(&records), &grid);
        assert_eq!(once.len(), twice.len());
        assert_eq!(once, twice, "the same file must place the same transforms");
        // Ordered by the file's own ids, not by the file's record order.
        let ids: Vec<String> = resolve(&records)
            .into_iter()
            .map(|instance| instance.id)
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "instances must be placed in id order");
    }

    /// THE stale-Y law: the placed Y is the grid's height under the instance's XZ, and the file's
    /// own Y reaches nothing.
    #[test]
    fn placement_reprojects_onto_the_grid_and_ignores_the_authored_y() {
        let grid = sloped_grid();
        let xz = Vec2::new(37.5, -112.25);
        let placed =
            |stale_y: f32| place(&resolve(&[record("house_proxy", xz, stale_y)]), &grid)[0].pose;
        let pose = placed(-500.0);
        assert_eq!(pose.translation.x, xz.x);
        assert_eq!(pose.translation.z, xz.y);
        assert_eq!(pose.translation.y, grid.height_at(xz.x, xz.y));
        assert_eq!(placed(900.0), pose, "the authored Y must change nothing");
    }

    /// A zero quaternion is FINITE, so the pose check waves it through — and then `normalize`
    /// divides by a zero length and puts a NaN rotation on a static collider, which reaches Avian
    /// as geometry nothing can resolve against. It has to be refused where it is still a number in
    /// a file (ADR-0011).
    #[test]
    #[should_panic(expected = "which names no orientation")]
    fn a_rotation_with_no_length_panics() {
        let mut raw = record("house_proxy", Vec2::ZERO, 0.0);
        raw.rotation = [0.0, 0.0, 0.0, 0.0];
        resolve(&[raw]);
    }

    /// …and the exporter's ordinary f32 text, which is never exactly unit, is still normalized
    /// rather than refused. Rejecting off-unit quaternions would refuse the shipped map.
    #[test]
    fn an_off_unit_rotation_is_normalized_not_refused() {
        let mut raw = record("house_proxy", Vec2::ZERO, 0.0);
        // A yaw whose components carry the rounding an f32 decimal round-trip leaves behind.
        raw.rotation = [0.0, 0.747_645, 0.0, 0.664_098_6];
        let resolved = resolve(&[raw]);
        assert!(
            (resolved[0].rotation.length() - 1.0).abs() < 1e-6,
            "the stored rotation must be unit",
        );
        assert!(
            resolved[0].rotation.is_finite(),
            "no NaN may reach a static collider",
        );
    }

    /// ADR-0011: a map naming a prototype the registry does not carry is a broken ship.
    #[test]
    #[should_panic(expected = "the graybox registry does not carry")]
    fn an_unknown_prototype_panics() {
        resolve(&[record("barn_proxy", Vec2::ZERO, 0.0)]);
    }

    /// The registry must cover the SHIPPED map — every prototype the file declares, not merely the
    /// ones it happens to instance today (the church has zero instances and still must resolve).
    #[test]
    fn the_registry_covers_every_prototype_the_shipped_map_declares() {
        let text = std::fs::read_to_string(crate::map::level_path(&shipped_assets()))
            .expect("the shipped tree carries a level file");
        let level: serde_json::Value =
            serde_json::from_str(&text).expect("the shipped level file parses");
        let declared = level["prototypes"]
            .as_object()
            .expect("the level file declares a prototype registry");
        assert!(!declared.is_empty());
        for id in declared.keys() {
            assert!(
                prototype_index(id).is_some(),
                "the shipped map declares prototype {id:?}, which the graybox registry does not \
                 carry",
            );
        }
    }

    /// The two contact laws, on the shipped map: every building is a block whose transform is the
    /// unit-cube encoding of its bounds, and no fir is a block.
    #[test]
    fn buildings_become_blocks_and_firs_do_not() {
        let grid = sloped_grid();
        let placements = place(&resolve(&shipped_manifest().instances), &grid);
        let mut buildings = 0usize;
        for placement in &placements {
            match placement.proxy() {
                Proxy::Building { min, max } => {
                    buildings += 1;
                    let block = placement.block().expect("a building is a block");
                    let scale = placement.pose.scale;
                    assert_eq!(block.scale, (max - min) * scale);
                    assert_eq!(block.rotation, placement.pose.rotation);
                    // The box centre sits over the ground pose by the prototype's own mid-bounds,
                    // so the skirt (`min.y` below zero) stays under the surface.
                    assert_eq!(
                        block.translation,
                        placement.pose.translation
                            + placement.pose.rotation * ((min + max) * 0.5 * scale),
                    );
                }
                Proxy::Fir { .. } => assert_eq!(
                    placement.block(),
                    None,
                    "a fir must never enter the block list — belts would climb the trunk",
                ),
            }
        }
        assert_eq!(buildings, 74, "the shipped map's building count");
        assert_eq!(placements.len(), 709, "the shipped map's instance count");
    }

    /// The spawn itself, in a real world: every building reaches the block list, every instance
    /// gets a static collider, and a windowless composition spawns no mesh.
    #[test]
    fn spawn_adds_a_collider_per_instance_and_a_block_per_building() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>();
        let grid = sloped_grid();
        let manifest = shipped_manifest();
        let mut blocks: Vec<Transform> = Vec::new();
        let world = app.world_mut();
        world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
            world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
                let mut queue = bevy::ecs::world::CommandQueue::default();
                let mut commands = Commands::new(&mut queue, world);
                spawn(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &manifest,
                    &grid,
                    &mut blocks,
                    false,
                );
                queue.apply(world);
            });
        });
        assert_eq!(blocks.len(), 74, "one block per building, no fir");
        let colliders = world
            .query_filtered::<Entity, (With<Collider>, With<RigidBody>)>()
            .iter(world)
            .count();
        assert_eq!(colliders, 709, "one static collider per instance");
        // The view read casts against the marker, not against the terrain layer at large: a
        // collider without it is invisible to the scatter-hit effect, so the tag must be on ALL of
        // them and on nothing else.
        let tagged = world
            .query_filtered::<Entity, With<ScatterProxy>>()
            .iter(world)
            .count();
        assert_eq!(
            tagged, colliders,
            "every scatter collider is a ScatterProxy"
        );
        let untagged = world
            .query_filtered::<Entity, (With<Collider>, Without<ScatterProxy>)>()
            .iter(world)
            .count();
        assert_eq!(untagged, 0, "no scatter collider ships untagged");
        let meshes = world.query::<&Mesh3d>().iter(world).count();
        assert_eq!(meshes, 0, "a windowless composition draws nothing");
    }
}
