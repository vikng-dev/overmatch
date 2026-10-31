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
//! THE CHUNKS STAY DATA UNTIL A HIT. No chunk is a collider while its element is whole; the
//! first touch swaps the element to its chunks, and the fracture laws ([`fracture`]) wear, crack
//! and drop them ([`breakage`]). Each root carries its [`breakage::Breakage`] from spawn.
//!
//! NOT TERRAIN. No element enters the [`crate::world::TerrainMap`] block list: the belts never
//! climb a wall and the floor inside is the terrain (playtest fork F9). Spawn avoidance reads each
//! building's bounds through [`crate::world::SpawnObstacles`] instead ([`footprint`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
use bevy::math::bounding::Aabb3d;
use bevy::prelude::*;

use crate::Layer;
use crate::ballistics::{BallisticVolume, Envelope, VolumeMaterial};
use crate::scatter::MapInstance;
use crate::substances::SubstanceRegistry;

pub(crate) mod breakage;
pub(crate) mod fracture;
pub mod schema;
pub(crate) mod view;

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
    /// Per entry of the asset's material table: the dynamic friction coefficient its substance
    /// brings to a contact (`materials.ron`).
    friction: Vec<f32>,
    /// The bounds of the elements and of every chunk that tiles them, in the building frame at
    /// scale 1, `(min, max)`: every collider the building ever stands, intact or broken.
    bounds: (Vec3, Vec3),
    /// What the fracture laws read of it.
    pub(crate) layout: breakage::Layout,
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
        let friction = asset
            .materials
            .iter()
            .map(|name| {
                registry
                    .hard(name)
                    .map_err(|err| err.to_string())
                    .and_then(|hard| hard.friction_of(name))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bounds = asset
            .elements
            .iter()
            .flat_map(schema::Element::points)
            .chain(
                asset
                    .chunks
                    .iter()
                    .flat_map(|chunk| chunk.hull.iter().map(|point| Vec3::from_array(*point))),
            )
            .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p), hi.max(p))
            });
        let masonry = asset
            .materials
            .iter()
            .map(|name| fracture::Masonry::of(registry, name))
            .collect::<Result<Vec<_>, _>>()?;
        let layout = breakage::Layout::of(&asset, &masonry)?;
        Ok(Self {
            asset,
            materials,
            friction,
            bounds,
            layout,
        })
    }

    /// Law chunk `c`'s collision friction once loose: its element's substance's, combined by
    /// `Max` so a hull's frictionless proxy (`tank::spawn::spawn_proxy_parts`, rule `Min`) meets
    /// the piece at the piece's own value, and the ground at the larger of the two.
    pub(crate) fn piece_friction(&self, c: usize) -> avian3d::prelude::Friction {
        use avian3d::prelude::{CoefficientCombine, Friction};
        let element = &self.asset.elements[usize::from(self.layout.element_of[c])];
        Friction::new(self.friction[usize::from(element.material)])
            .with_combine_rule(CoefficientCombine::Max)
    }

    /// The bounds of the elements and their chunks in the building frame at scale 1,
    /// `(min, max)`.
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

/// The collision filter of an intact element and of a standing chunk: hulls contact it and
/// rounds walk it.
pub(crate) fn element_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL)
}

/// Element `k`'s collider entity, less its parent: the prism at the instance's `scale`, a
/// ballistic volume of its substance.
fn element_bundle(kind: &BuildingKind, k: usize, scale: f32) -> impl Bundle {
    let element = &kind.asset.elements[k];
    let points: Vec<Vec3> = element.points().into_iter().map(|p| p * scale).collect();
    let material = usize::from(element.material);
    (
        Transform::IDENTITY,
        Collider::convex_hull(points)
            .unwrap_or_else(|| panic!("buildings: element {} has no convex hull", element.name)),
        element_layers(),
        BallisticVolume {
            material: kind.materials[material],
            substance: kind.asset.materials[material].clone(),
        },
        BuildingElement(u16::try_from(k).expect("the schema bounds the element count")),
    )
}

/// Spawn one building of `kind` at `pose`, the ground pose: the authored XZ, the grid's Y, the
/// authored yaw and uniform scale. A tilted or non-uniformly scaled instance is refused (ADR-0011):
/// a building stands on its foundation plane. Returns the root; the body is whole without a view.
/// Its [`Envelope`] is the kind's bounds at the instance's scale: a static body, and every element
/// and chunk collider it stands stays inside them.
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
            Envelope(Aabb3d::from_min_max(
                kind.bounds.0 * scale,
                kind.bounds.1 * scale,
            )),
            breakage::Breakage::new(kind.clone(), scale),
        ))
        .id();
    for k in 0..kind.asset.elements.len() {
        commands.spawn((ChildOf(root), element_bundle(kind, k, scale)));
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

    /// EVERY COLLIDER A BUILDING EVER STANDS LIES INSIDE ITS ENVELOPE (the game's damage model:
    /// a round's crossing of the building runs until it has left the envelope, so a step never
    /// ends beside a wall, `ballistics::Envelope`): each intact element's, and each chunk's that
    /// a swap stands in its place, at the instance's scale (1.5, fixture data). A spawn without
    /// the envelope, or one built from the elements alone where a chunk reaches past them, or at
    /// scale 1, fails here.
    #[test]
    fn every_collider_a_building_stands_lies_inside_its_envelope() {
        let kind = shipped_house();
        let scale = 1.5;
        let pose = Transform::from_xyz(5.0, 2.0, -3.0)
            .with_rotation(Quat::from_rotation_y(0.7))
            .with_scale(Vec3::splat(scale));
        let (world, root) = spawned(&kind, pose);
        let elements = crate::ballistics::assert_envelope_holds(&world, root);
        assert_eq!(elements, kind.asset.elements.len());
        let envelope = world.get::<Envelope>(root).unwrap().0;
        for c in 0..kind.layout.chunks.len() {
            let collider = breakage::chunk_collider(&kind, c, scale, Vec3::ZERO, 0.0)
                .expect("a shipped chunk has volume");
            let aabb = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
            assert!(
                envelope.min.cmple(aabb.min.into()).all()
                    && envelope.max.cmpge(aabb.max.into()).all(),
                "chunk {c} spans {:?}..{:?}, outside the envelope {envelope:?}",
                aabb.min,
                aabb.max,
            );
        }
    }

    /// A LOOSE PIECE GRIPS A HULL AND KEEPS ITS MATERIAL AGAINST THE GROUND: a tank collision
    /// proxy is frictionless under `Min` (`tank::spawn::spawn_proxy_parts`, so the track model owns
    /// the hull's grip on terrain), which beats every other rule but `Max`. Each chunk of the
    /// shipped house, loose, combines with that proxy at its own substance's row, and with a default-friction terrain at no less than the terrain's; the
    /// proxy against that terrain stays zero. A piece on any other rule, or without its row's
    /// value, fails here.
    #[test]
    fn a_loose_piece_resolves_to_its_own_friction_against_a_hull_proxy() {
        use avian3d::prelude::{CoefficientCombine, Friction};
        let kind = shipped_house();
        let proxy = Friction::ZERO.with_combine_rule(CoefficientCombine::Min);
        let terrain = Friction::new(0.5);
        assert_eq!(proxy.combine(terrain).dynamic_coefficient, 0.0);
        assert!(!kind.layout.chunks.is_empty());
        for c in 0..kind.layout.chunks.len() {
            let piece = kind.piece_friction(c);
            let element = &kind.asset.elements[usize::from(kind.layout.element_of[c])];
            let row = SubstanceRegistry::shipped_static()
                .hard(&kind.asset.materials[usize::from(element.material)])
                .unwrap()
                .friction
                .unwrap();
            assert_eq!(proxy.combine(piece).dynamic_coefficient, row);
            assert_eq!(terrain.combine(piece).dynamic_coefficient, row.max(0.5));
        }
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
