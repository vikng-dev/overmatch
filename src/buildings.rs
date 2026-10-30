//! Buildings in the game's destruction model: hollow houses whose walls and roof are matter.
//!
//! A BUILDING IS DATA (ADR-0034). `assets/buildings/<id>/<id>.building.ron` ([`schema`]) is the
//! building as the asset tools' bake wrote it: its elements (piers, sills, lintels, gables, wall
//! plates, roof planes), the fracture chunks that tile them, the joints between chunks and the
//! chunks the ground holds. No game code builds or edits one; the bake is `tools/`, which the game
//! never depends on. Read once at startup into the map's [`BuildingLibrary`], validated there
//! (ADR-0011), and folded into the map's content digest, so two peers spawn the same walls.
//!
//! THE BODY, INTACT. One static root per building, and one child per element: its own convex
//! collider (the element's prism) on `[Terrain, Armor]` with filters `ALL`, and its own
//! [`BallisticVolume`] of the element's substance. Hulls contact an element; the walk slows a
//! round by chord × factor through every element it crosses, and an opening is no element, so
//! rounds and sight pass through it. The instance's uniform scale is applied to the points at
//! spawn and the root's own scale is one (the trees' convention).
//!
//! THE CHUNKS STAY DATA. No chunk is a collider and no element holds an HP pool in this slice:
//! the chunk table and the joint graph wait in the [`BuildingKind`] for the laws that break walls.
//!
//! NOT TERRAIN. No element enters the [`crate::world::TerrainMap`] block list: the belts never
//! climb a wall and the floor inside is the terrain (playtest fork F9). Spawn avoidance reads each
//! building's bounds through [`crate::world::SpawnObstacles`] instead ([`footprint`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
use bevy::prelude::*;

use crate::Layer;
use crate::ballistics::{BallisticVolume, VolumeMaterial};
use crate::scatter::MapInstance;
use crate::substances::SubstanceRegistry;

pub mod schema;

use schema::BuildingAsset;

/// Directory under the asset root holding one folder per building asset.
const BUILDINGS_DIR: &str = "buildings";

/// How far below 1 an instance's up vector's Y may fall before the instance counts as tilted: a
/// pure yaw's rounding, never a lean. A building stands on its foundation plane.
const UPRIGHT_EPSILON: f32 = 1e-6;

/// One building asset, read and validated: what every instance of it spawns from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BuildingKind {
    pub(crate) asset: BuildingAsset,
    /// Per entry of the asset's material table: the substance as the walk reads it.
    materials: Vec<VolumeMaterial>,
    /// The elements' bounds in the building frame at scale 1, `(min, max)`.
    bounds: (Vec3, Vec3),
}

impl BuildingKind {
    /// Resolve asset `id`'s text against the substance registry. Every refusal is a broken ship
    /// (ADR-0011): the message names what, the caller names the file.
    fn resolve(id: &str, text: &str, registry: &SubstanceRegistry) -> Result<Self, String> {
        let asset = BuildingAsset::from_ron(text)?;
        if asset.id != id {
            return Err(format!("names itself {:?}", asset.id));
        }
        let materials = asset
            .materials
            .iter()
            .map(|name| VolumeMaterial::of(registry, name).map_err(|err| err.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let bounds = asset
            .elements
            .iter()
            .flat_map(schema::Element::points)
            .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p), hi.max(p))
            });
        Ok(Self {
            asset,
            materials,
            bounds,
        })
    }

    /// The elements' bounds in the building frame at scale 1, `(min, max)`.
    pub(crate) fn bounds(&self) -> (Vec3, Vec3) {
        self.bounds
    }
}

/// Every building asset the selected map places, keyed by asset id.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct BuildingLibrary {
    kinds: BTreeMap<String, Arc<BuildingKind>>,
    /// The texts the kinds were read from, in id order, for the map's content digest.
    texts: Vec<String>,
}

impl BuildingLibrary {
    /// Read and validate the assets `ids` name under `root`. Every file must exist and hold
    /// (ADR-0011): a map that places a building this build cannot read is present-but-broken.
    pub(crate) fn load<'a>(root: &Path, ids: impl IntoIterator<Item = &'a str>) -> Self {
        let ids: BTreeSet<&str> = ids.into_iter().collect();
        let registry = SubstanceRegistry::shipped_static();
        let mut library = Self::default();
        for id in ids {
            let path = root
                .join(BUILDINGS_DIR)
                .join(id)
                .join(format!("{id}.building.ron"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!("buildings: {} cannot be read: {err}", path.display())
            });
            let kind = BuildingKind::resolve(id, &text, registry)
                .unwrap_or_else(|err| panic!("buildings: {}: {err}", path.display()));
            library.texts.push(text);
            library.kinds.insert(id.to_owned(), Arc::new(kind));
        }
        library
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Arc<BuildingKind>> {
        self.kinds.get(id)
    }

    /// The bytes of every file the library was read from, in id order.
    pub(crate) fn texts(&self) -> impl Iterator<Item = &[u8]> {
        self.texts.iter().map(String::as_bytes)
    }
}

/// One element's collider entity: the index of its element in the asset.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[component(immutable)]
pub(crate) struct BuildingElement(pub(crate) u16);

/// A building's root.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct BuildingBody;

/// The collision filter of an intact element: hulls contact it and rounds walk it.
fn element_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL)
}

/// Spawn one building of `kind` at `pose`, the ground pose: the authored XZ, the grid's Y, the
/// authored yaw and uniform scale. A tilted or non-uniformly scaled instance is refused (ADR-0011):
/// a building stands on its foundation plane. Returns the root; the body is whole without a view.
pub(crate) fn spawn(
    commands: &mut Commands,
    kind: &Arc<BuildingKind>,
    identity: MapInstance,
    pose: Transform,
) -> Entity {
    let scale = pose.scale.x;
    assert!(
        pose.scale.y == scale && pose.scale.z == scale && scale > 0.0,
        "buildings: instance {} has scale {:?}; a building scales uniformly",
        identity.id,
        pose.scale,
    );
    assert!(
        (pose.rotation * Vec3::Y).y >= 1.0 - UPRIGHT_EPSILON,
        "buildings: instance {} is tilted ({:?}); a building stands upright",
        identity.id,
        pose.rotation,
    );
    let root = commands
        .spawn((
            Name::new(format!("building {}", identity.id)),
            Transform {
                translation: pose.translation,
                rotation: pose.rotation,
                scale: Vec3::ONE,
            },
            RigidBody::Static,
            identity,
            BuildingBody,
        ))
        .id();
    for (k, element) in kind.asset.elements.iter().enumerate() {
        let points: Vec<Vec3> = element.points().into_iter().map(|p| p * scale).collect();
        let material = usize::from(element.material);
        commands.spawn((
            ChildOf(root),
            Transform::IDENTITY,
            Collider::convex_hull(points).unwrap_or_else(|| {
                panic!("buildings: element {} has no convex hull", element.name)
            }),
            element_layers(),
            BallisticVolume {
                material: kind.materials[material],
                substance: kind.asset.materials[material].clone(),
            },
            BuildingElement(u16::try_from(k).expect("the schema bounds the element count")),
        ));
    }
    root
}

/// A building's bounds at `pose`, in the [`crate::world::TerrainMap`] block encoding (a unit cube
/// posed and scaled): what spawn avoidance refuses a spawn inside of.
pub(crate) fn footprint(kind: &BuildingKind, pose: Transform) -> Transform {
    let (min, max) = kind.bounds();
    Transform {
        translation: pose.translation + pose.rotation * ((min + max) * 0.5 * pose.scale),
        rotation: pose.rotation,
        scale: (max - min) * pose.scale,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use avian3d::prelude::SimpleCollider;

    use super::*;
    use crate::map::tests::shipped_assets;

    /// The shipped house, read through the real loader.
    pub(crate) fn shipped_house() -> Arc<BuildingKind> {
        BuildingLibrary::load(&shipped_assets(), ["house_1"])
            .get("house_1")
            .expect("the shipped house")
            .clone()
    }

    /// `kind` spawned at `pose` into a fresh world; its root.
    fn spawned(kind: &Arc<BuildingKind>, pose: Transform) -> (World, Entity) {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        let identity = MapInstance {
            id: "house_0001".to_owned(),
            ordinal: 0,
        };
        let root = spawn(&mut commands, kind, identity, pose);
        queue.apply(&mut world);
        (world, root)
    }

    /// THE INTACT BODY IS ITS ELEMENTS (the game's environment destruction model): one static root
    /// and, under it, one convex collider per element on the layers rounds and hulls read, each a
    /// ballistic volume of its element's own substance from the shipped registry, and no chunk is
    /// a collider. A spawn that built a collider per chunk (1163 per house, MEASURED for
    /// `house_1`), dropped the armour layer (rounds would fly through walls) or wore one
    /// substance for the whole house (a brick roof) fails here.
    #[test]
    fn a_house_spawns_one_ballistic_collider_per_element() {
        let kind = shipped_house();
        let (mut world, root) = spawned(&kind, Transform::from_xyz(5.0, 2.0, -3.0));
        let elements = kind.asset.elements.len();
        let mut seen = 0;
        let mut children = world.query::<(
            &ChildOf,
            &BuildingElement,
            &Collider,
            &CollisionLayers,
            &BallisticVolume,
        )>();
        for (parent, element, _, layers, volume) in children.iter(&world) {
            assert_eq!(parent.parent(), root);
            let authored = &kind.asset.elements[usize::from(element.0)];
            let substance = &kind.asset.materials[usize::from(authored.material)];
            assert_eq!(&volume.substance, substance, "{}", authored.name);
            assert_eq!(
                volume.material,
                VolumeMaterial::of(SubstanceRegistry::shipped_static(), substance).unwrap()
            );
            assert_eq!(*layers, element_layers());
            seen += 1;
        }
        assert_eq!(seen, elements, "one collider per element");
        assert_eq!(world.query::<&Collider>().iter(&world).count(), elements);
        let substances: BTreeSet<String> = world
            .query::<&BallisticVolume>()
            .iter(&world)
            .map(|volume| volume.substance.clone())
            .collect();
        assert_eq!(
            substances,
            BTreeSet::from(["Brick".to_owned(), "Wood".to_owned()])
        );
        assert!(world.get::<RigidBody>(root).is_some());
    }

    /// THE INSTANCE'S SCALE SCALES THE WALLS, NOT THE ROOT (the trees' convention): at scale 1.5
    /// (fixture data) every element's collider spans 1.5 times its authored extent and the root
    /// stays at unit scale; the footprint spawn avoidance reads is the scaled bounds, posed.
    #[test]
    fn the_instance_scale_scales_every_element_and_the_footprint() {
        let kind = shipped_house();
        let pose = Transform::from_xyz(5.0, 2.0, -3.0).with_scale(Vec3::splat(1.5));
        let (mut world, root) = spawned(&kind, pose);
        assert_eq!(world.get::<Transform>(root).unwrap().scale, Vec3::ONE);
        let mut colliders = world.query::<(&BuildingElement, &Collider)>();
        for (element, collider) in colliders.iter(&world) {
            let authored = &kind.asset.elements[usize::from(element.0)];
            let aabb = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
            let (lo, hi) = authored
                .points()
                .iter()
                .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &p| {
                    (lo.min(p), hi.max(p))
                });
            let size = aabb.max - aabb.min;
            assert!(
                (size - (hi - lo) * 1.5).abs().max_element() < 1e-3,
                "{}: {size} against {}",
                authored.name,
                (hi - lo) * 1.5
            );
        }
        let block = footprint(&kind, pose);
        let (min, max) = kind.bounds;
        assert_eq!(block.scale, (max - min) * 1.5);
        assert_eq!(block.translation, pose.translation + (min + max) * 0.75);
    }

    /// A tilted building is refused where it is still a pose (ADR-0011): its foundation plane
    /// would not be level and its anchors would hold nothing.
    #[test]
    #[should_panic(expected = "a building stands upright")]
    fn a_tilted_building_is_refused() {
        spawned(
            &shipped_house(),
            Transform::from_rotation(Quat::from_rotation_x(0.3)),
        );
    }

    /// A FILE THAT BREAKS THE CONTRACT DOES NOT LOAD: the shipped house parses and holds, and the
    /// same text naming another id, with an anchor lifted off the foundation plane, a joint out
    /// of order, an element whose run skips a chunk, or a material the registry does not know, is
    /// refused naming why.
    #[test]
    fn a_building_file_that_breaks_the_contract_is_refused() {
        let path = shipped_assets().join("buildings/house_1/house_1.building.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        let registry = SubstanceRegistry::shipped_static();
        BuildingKind::resolve("house_1", &text, registry).expect("the shipped house holds");
        assert!(BuildingKind::resolve("house_2", &text, registry).is_err());
        let mut asset = BuildingAsset::from_ron(&text).unwrap();
        asset.anchors[0].centroid[1] += 0.1;
        assert!(
            asset
                .check()
                .unwrap_err()
                .contains("off the foundation plane")
        );
        let mut asset = BuildingAsset::from_ron(&text).unwrap();
        asset.joints.swap(0, 1);
        assert!(asset.check().unwrap_err().contains("out of order"));
        let mut asset = BuildingAsset::from_ron(&text).unwrap();
        asset.elements[1].chunks[0] += 1;
        assert!(
            asset
                .check()
                .unwrap_err()
                .contains("do not continue the run")
        );
        let unknown = text.replacen("\"Brick\"", "\"Adobe\"", 1);
        let err = BuildingKind::resolve("house_1", &unknown, registry).unwrap_err();
        assert!(err.contains("Adobe"), "{err}");
    }
}
