//! Constructions in the game's environment destruction model, game state only.
//!
//! A CONSTRUCTION IS DATA (ADR-0034). `assets/constructions/<id>/<id>.construction.ron` ([`schema`]) is the
//! construction as the asset tools' bake wrote it: its convex elements, their fracture chunks,
//! the joints between chunks and the chunks the ground holds. No game code builds or edits one; the bake is `tools/`, which the game
//! never depends on. Read once at startup into the map's [`ConstructionLibrary`], validated there
//! (ADR-0011), and folded into the map's content digest, so two peers spawn the same construction state.
//!
//! THE BODY, INTACT. One static root per construction, and one child per element: its own convex
//! collider (the element's hull) on `[Terrain, Armor]` with filters `ALL`, and its own
//! [`BallisticVolume`] of the element's substance. Hulls contact an element; the walk slows a
//! round by chord × factor through every element it crosses, and an opening is no element, so
//! rounds and sight pass through it. The instance's uniform scale is applied to the points at
//! spawn and the root's own scale is one.
//!
//! MUTABLE STATE EXISTS AT SPAWN. No chunk is a collider while its element is whole; the
//! first touch swaps the element to its chunks, and the fracture laws ([`fracture`]) wear, crack
//! and drop them ([`breakage`]). Each root carries complete [`breakage::Breakage`] from spawn;
//! neither simulation nor stable chunk addresses depend on view assets.
//!
//! NOT TERRAIN. No element enters the [`crate::world::TerrainMap`] block list, so the terrain
//! field stays immutable and the floor inside is the terrain (playtest fork F9). The belts meet a
//! standing element or chunk only as a collider of the rubble overlay (`track::rubble`'s
//! `RubbleQuery`), as they meet loose pieces. Spawn avoidance reads each
//! construction's bounds through [`crate::world::SpawnObstacles`] instead ([`footprint`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
use bevy::prelude::*;

use crate::Layer;
use crate::ballistics::{BallisticVolume, VolumeMaterial};
use crate::scatter::MapInstance;
use crate::substances::SubstanceRegistry;

pub mod attachments;
pub(crate) mod breakage;
pub(crate) mod fracture;
pub mod schema;
pub(crate) mod view;

use attachments::Attachments;
use schema::ConstructionAsset;

/// Directory under the asset root holding one folder per construction asset.
const CONSTRUCTIONS_DIR: &str = "constructions";

/// Resolution: upright-vector tolerance for pure-yaw rounding, never a lean; anchored
/// constructions stand on their foundation plane.
const UPRIGHT_EPSILON: f32 = 1e-6;

/// One construction asset, read and validated: what every instance of it spawns from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConstructionKind {
    pub(crate) asset: ConstructionAsset,
    /// Per entry of the asset's material table: the substance as the walk reads it.
    materials: Vec<VolumeMaterial>,
    /// Per entry of the asset's material table: the friction its substance's colliders carry
    /// (`substances::HardSubstance::contact_friction`).
    friction: Vec<avian3d::prelude::Friction>,
    /// The bounds of the elements and of every chunk that tiles them, in the construction frame at
    /// scale 1, `(min, max)`: every collider the construction ever stands, intact or broken.
    bounds: (Vec3, Vec3),
    /// What the fracture laws read of it.
    pub(crate) layout: breakage::Layout,
    /// What its view adds to the law geometry, view only: `<id>.dressing.ron`, read with the
    /// library ([`ConstructionLibrary::load`]); bare for a kind resolved from its text alone.
    pub(crate) dressing: view::Dressing,
    /// Checked at admission, keyed by the authored uniform scale's bits. Gameplay never
    /// converts an attachment again; instances at the same scale share immutable construction.
    structures: BTreeMap<u32, Arc<fracture::Structure>>,
    /// Initial gravity solved once from admitted data, shared until an instance changes.
    equilibria: BTreeMap<u32, Arc<fracture::bondsolve::Assessment>>,
}

impl ConstructionKind {
    /// Resolve asset `id`'s text against the substance registry and the attachment kinds. Every
    /// refusal is a broken ship (ADR-0011): the message names what, the caller names the file.
    fn resolve(
        id: &str,
        text: &str,
        registry: &SubstanceRegistry,
        attachments: &Attachments,
    ) -> Result<Self, String> {
        let asset = ConstructionAsset::from_ron(text)?;
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
            .map(|name| registry.contact_friction(name))
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
        let law_materials = asset
            .materials
            .iter()
            .map(|name| fracture::Material::of(registry, name))
            .collect::<Result<Vec<_>, _>>()?;
        let layout = breakage::Layout::of(&asset, &law_materials, attachments)?;
        let structure = Arc::new(layout.checked_structure(&asset, 1.0)?);
        let equilibrium = Arc::new(fracture::bondsolve::Assessment::at_rest(&structure)?);
        Ok(Self {
            dressing: view::Dressing::bare(&layout),
            asset,
            materials,
            friction,
            bounds,
            layout,
            structures: BTreeMap::from([(1.0_f32.to_bits(), structure)]),
            equilibria: BTreeMap::from([(1.0_f32.to_bits(), equilibrium)]),
        })
    }

    fn admit_scale(&mut self, scale: f32) -> Result<(), String> {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            self.structures.entry(scale.to_bits())
        {
            let structure = Arc::new(self.layout.checked_structure(&self.asset, scale)?);
            let unit = &self.equilibria[&1.0_f32.to_bits()];
            let equilibrium = Arc::new(fracture::bondsolve::Assessment::at_rest_scaled(
                &structure,
                unit,
                f64::from(scale),
            )?);
            entry.insert(structure);
            self.equilibria.insert(scale.to_bits(), equilibrium);
        }
        Ok(())
    }

    fn structure(&self, scale: f32) -> Arc<fracture::Structure> {
        self.structures
            .get(&scale.to_bits())
            .expect(
                "map admission retains the checked structure for every placed construction scale",
            )
            .clone()
    }

    fn equilibrium(&self, scale: f32) -> Arc<fracture::bondsolve::Assessment> {
        self.equilibria
            .get(&scale.to_bits())
            .expect("map admission prepares initial gravity for every admitted construction scale")
            .clone()
    }

    /// The friction element `k`'s colliders carry, standing or loose: its substance's.
    pub(crate) fn element_friction(&self, k: usize) -> avian3d::prelude::Friction {
        self.friction[usize::from(self.asset.elements[k].material)]
    }

    /// The friction law chunk `c`'s collider carries, standing or loose: its element's.
    pub(crate) fn chunk_friction(&self, c: usize) -> avian3d::prelude::Friction {
        self.element_friction(usize::from(self.layout.element_of[c]))
    }

    /// The bounds of the elements and their chunks in the construction frame at scale 1,
    /// `(min, max)`.
    pub(crate) fn bounds(&self) -> (Vec3, Vec3) {
        self.bounds
    }
}

/// The map's load gate ([`ConstructionLibrary::load`]) on asset `id`'s file text, against the shipped
/// substances and attachment kinds, at its baked scale: what the bake runs on each asset before it
/// writes it, so a refusal stops there in the game's own words.
pub fn admit(id: &str, text: &str) -> Result<(), String> {
    ConstructionKind::resolve(
        id,
        text,
        SubstanceRegistry::shipped_static(),
        Attachments::shipped_static(),
    )
    .map(drop)
}

/// Every construction asset the selected map places, keyed by asset id.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ConstructionLibrary {
    kinds: BTreeMap<String, Arc<ConstructionKind>>,
    /// The texts the kinds were read from, in id order, for the map's content digest.
    texts: Vec<String>,
}

impl ConstructionLibrary {
    /// Read and validate the assets `ids` name under `root`. Every file must exist and hold
    /// (ADR-0011): a map that places a construction this build cannot read is present-but-broken.
    pub(crate) fn load<'a>(root: &Path, ids: impl IntoIterator<Item = &'a str>) -> Self {
        let ids: BTreeSet<&str> = ids.into_iter().collect();
        let registry = SubstanceRegistry::shipped_static();
        let attachments = Attachments::shipped_static();
        let mut library = Self::default();
        for id in ids {
            let path = root
                .join(CONSTRUCTIONS_DIR)
                .join(id)
                .join(format!("{id}.construction.ron"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!("construction: {} cannot be read: {err}", path.display())
            });
            let mut kind = ConstructionKind::resolve(id, &text, registry, attachments)
                .unwrap_or_else(|err| panic!("construction: {}: {err}", path.display()));
            let dressed = path.with_file_name(format!("{id}.dressing.ron"));
            let dressing = std::fs::read_to_string(&dressed).unwrap_or_else(|err| {
                panic!("construction: {} cannot be read: {err}", dressed.display())
            });
            kind.dressing = view::Dressing::resolve(&dressing, &kind.asset, &kind.layout)
                .unwrap_or_else(|err| panic!("construction: {}: {err}", dressed.display()));
            library.texts.extend([text, dressing]);
            library.kinds.insert(id.to_owned(), Arc::new(kind));
        }
        library
    }

    /// The game's environment destruction model resolves each placement before play, retaining
    /// its checked structure. The caller supplies the manifest and instance in refusal messages.
    pub(crate) fn admit_placement(&mut self, id: &str, scale: [f32; 3]) -> Result<(), String> {
        let result = if !(scale.iter().all(|s| s.is_finite() && *s > 0.0)
            && scale[0] == scale[1]
            && scale[0] == scale[2])
        {
            Err(format!(
                "scale {scale:?}: author a finite positive uniform scale"
            ))
        } else {
            Arc::make_mut(
                self.kinds
                    .get_mut(id)
                    .expect("the map library loaded every declared construction asset"),
            )
            .admit_scale(scale[0])
        };
        result.map_err(|error| format!("{CONSTRUCTIONS_DIR}/{id}/{id}.construction.ron: {error}"))
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Arc<ConstructionKind>> {
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
pub(crate) struct Element(pub(crate) u16);

/// A construction's root: a structure, whose contacts the solver's reading keeps.
#[derive(Component, Debug, Clone, Copy)]
#[require(crate::contact_transfer::StructureBody)]
pub(crate) struct Body;

/// The collision filter of an intact element and of a standing chunk: hulls contact it and
/// rounds walk it.
pub(crate) fn element_layers() -> CollisionLayers {
    CollisionLayers::new([Layer::Terrain, Layer::Armor], LayerMask::ALL)
}

/// Element `k`'s collider entity, less its parent: its hull at the instance's `scale`, a
/// ballistic volume of its substance.
fn element_bundle(kind: &ConstructionKind, k: usize, scale: f32) -> impl Bundle {
    let element = &kind.asset.elements[k];
    let points: Vec<Vec3> = element.points().into_iter().map(|p| p * scale).collect();
    let material = usize::from(element.material);
    (
        Transform::IDENTITY,
        Collider::convex_hull(points)
            .unwrap_or_else(|| panic!("construction: element {} has no convex hull", element.name)),
        element_layers(),
        kind.element_friction(k),
        BallisticVolume {
            material: kind.materials[material],
            substance: kind.asset.materials[material].clone(),
        },
        Element(u16::try_from(k).expect("the schema bounds the element count")),
    )
}

/// Spawn one construction of `kind` at `pose`, the ground pose: the authored XZ, the grid's Y, the
/// authored yaw and uniform scale. A tilted or non-uniformly scaled instance is refused (ADR-0011):
/// a construction stands on its foundation plane. Returns the root; the body is whole without a view.
/// The root carries `Visibility` because a view, where one draws, is its child and inherits
/// visibility through it.
pub(crate) fn spawn(
    commands: &mut Commands,
    kind: &Arc<ConstructionKind>,
    identity: MapInstance,
    pose: Transform,
) -> Entity {
    let scale = pose.scale.x;
    assert!(
        pose.scale.y == scale && pose.scale.z == scale && scale > 0.0,
        "construction: instance {} has scale {:?}; a construction scales uniformly",
        identity.id,
        pose.scale,
    );
    assert!(
        (pose.rotation * Vec3::Y).y >= 1.0 - UPRIGHT_EPSILON,
        "construction: instance {} is tilted ({:?}); a construction stands upright",
        identity.id,
        pose.rotation,
    );
    let breakage = breakage::Breakage::new(kind.clone(), identity.ordinal, scale);
    let root = commands
        .spawn((
            Name::new(format!("construction {}", identity.id)),
            Transform {
                translation: pose.translation,
                rotation: pose.rotation,
                scale: Vec3::ONE,
            },
            Visibility::default(),
            RigidBody::Static,
            identity,
            Body,
            breakage,
        ))
        .id();
    for k in 0..kind.asset.elements.len() {
        commands.spawn((ChildOf(root), element_bundle(kind, k, scale)));
    }
    root
}

/// A construction's bounds at `pose`, in the [`crate::world::TerrainMap`] block encoding (a unit cube
/// posed and scaled): what spawn avoidance refuses a spawn inside of.
pub(crate) fn footprint(kind: &ConstructionKind, pose: Transform) -> Transform {
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

    /// Fixture: a column of three unit cubes, joined over their full faces, anchored at its base.
    /// It has no house topology or view asset; all masses derive from the registry's material density.
    pub(crate) fn column_construction() -> Arc<ConstructionKind> {
        let registry = crate::substances::SubstanceRegistry::shipped_static();
        let density = fracture::Material::of(registry, "Brick")
            .unwrap()
            .unwrap()
            .density;
        let hull = |lo: f32, hi: f32| {
            [-0.5, 0.5]
                .into_iter()
                .flat_map(|x| {
                    [lo, hi]
                        .into_iter()
                        .flat_map(move |y| [-0.5, 0.5].map(|z| [x, y, z]))
                })
                .collect()
        };
        let asset = ConstructionAsset {
            id: "column_fixture".into(),
            format: schema::FORMAT,
            foundation_y_m: 0.0,
            materials: vec!["Brick".into()],
            attachments: vec!["mortared".into()],
            components: vec!["column".into()],
            elements: vec![schema::Element {
                name: "column".into(),
                part: "pier".into(),
                component: 0,
                material: 0,
                surface_frame: schema::SurfaceFrame::Upright,
                hull: hull(0.0, 3.0),
                chunks: [0, 3],
            }],
            chunks: (0..3)
                .map(|c| schema::Chunk {
                    element: 0,
                    material: 0,
                    hull: hull(c as f32, c as f32 + 1.0),
                    volume_m3: 1.0,
                    mass_kg: density,
                    centroid: [0.0, c as f32 + 0.5, 0.0],
                })
                .collect(),
            joints: (0..2)
                .map(|a| schema::Joint {
                    a,
                    b: a + 1,
                    area_m2: 1.0,
                    centroid: [0.0, a as f32 + 1.0, 0.0],
                    normal: [0.0, 1.0, 0.0],
                    bond: schema::Bond::Seam,
                })
                .collect(),
            anchors: vec![schema::Anchor {
                chunk: 0,
                attachment: 0,
                area_m2: 1.0,
                centroid: [0.0; 3],
            }],
        };
        Arc::new(
            ConstructionKind::resolve(
                &asset.id,
                &ron::ser::to_string(&asset).unwrap(),
                registry,
                Attachments::shipped_static(),
            )
            .unwrap(),
        )
    }

    /// The immutable shipped fixture, resolved through the real loader once per test process.
    /// Loader refusal tests read their own inputs; sharing this fixture repeats no hull construction.
    pub(crate) fn shipped_house() -> Arc<ConstructionKind> {
        static HOUSE: std::sync::OnceLock<Arc<ConstructionKind>> = std::sync::OnceLock::new();
        HOUSE
            .get_or_init(|| {
                ConstructionLibrary::load(&shipped_assets(), ["house_1"])
                    .get("house_1")
                    .expect("the shipped house")
                    .clone()
            })
            .clone()
    }

    /// `kind` spawned at `pose` into a fresh world; its root.
    fn spawned(kind: &Arc<ConstructionKind>, pose: Transform) -> (World, Entity) {
        let mut world = World::new();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &world);
        let identity = MapInstance {
            id: "house_0001".to_owned(),
            ordinal: 0,
        };
        let mut admitted = (**kind).clone();
        admitted.admit_scale(pose.scale.x).unwrap();
        let root = spawn(&mut commands, &Arc::new(admitted), identity, pose);
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
            &Element,
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
            kind.asset
                .elements
                .iter()
                .map(|element| kind.asset.materials[usize::from(element.material)].clone())
                .collect()
        );
        assert!(world.get::<RigidBody>(root).is_some());
    }

    /// EVERY SHIPPED CONSTRUCTION IS ADMITTED AND STANDS AS ITS ELEMENTS (the game's environment
    /// destruction model), whatever it builds: each asset under `assets/constructions/` passes the
    /// map's load gate (law blocks, collider mass and inertia, attachments and root plates) and
    /// spawns one collider per element and none per chunk, its whole chunk table in its breakage
    /// state from spawn. A refused asset or a spawn of a collider per chunk fails here.
    #[test]
    fn every_shipped_construction_is_admitted_and_stands_as_its_elements() {
        let assets = shipped_assets();
        let mut ids: Vec<String> = std::fs::read_dir(assets.join(CONSTRUCTIONS_DIR))
            .expect("the shipped constructions folder")
            .map(|entry| entry.expect("a readable entry").path())
            .filter_map(|dir| {
                let id = dir.file_name()?.to_str()?.to_owned();
                dir.join(format!("{id}.construction.ron"))
                    .is_file()
                    .then_some(id)
            })
            .collect();
        ids.sort();
        assert!(!ids.is_empty(), "no shipped construction");
        let library = ConstructionLibrary::load(&assets, ids.iter().map(String::as_str));
        for id in &ids {
            let kind = library.get(id).expect("the library holds every loaded id");
            let (mut world, root) = spawned(kind, Transform::IDENTITY);
            let colliders = world.query::<&Collider>().iter(&world).count();
            assert_eq!(colliders, kind.asset.elements.len(), "{id}");
            let breakage = world
                .get::<breakage::Breakage>(root)
                .expect("a construction carries its breakage state from spawn");
            assert_eq!(
                breakage.kind.asset.chunks.len(),
                kind.asset.chunks.len(),
                "{id}"
            );
        }
    }

    /// THE BAKE ADMITS BY THE LOAD GATE (the game's environment destruction model): [`admit`] is
    /// the gate a map runs, on the text the bake is about to write, so the bake refuses what a map
    /// would, in the gate's words. Fixture: the shipped house with one chunk's hull pressed to a
    /// slab half as thick as the law's plane tolerance, which the schema passes and the law block
    /// refuses.
    #[test]
    fn the_bake_admits_by_the_load_gate() {
        let kind = shipped_house();
        let mut asset = kind.asset.clone();
        admit(
            &asset.id,
            &ron::to_string(&asset).expect("the schema serializes"),
        )
        .expect("the shipped house is admitted");
        let c = kind.layout.chunks[0] as usize;
        asset.chunks[c].hull = (0..8)
            .map(|i| {
                [
                    [0.0, 0.08][i & 1],
                    [0.0, 0.08][(i >> 1) & 1],
                    [0.0, fracture::ON_PLANE / 2.0][i >> 2],
                ]
            })
            .collect();
        let text = ron::to_string(&asset).expect("the schema serializes");
        let error = admit(&asset.id, &text).unwrap_err();
        assert!(
            error.contains(&format!("chunks[{c}].hull")) && error.contains("law block"),
            "{error}"
        );
    }

    /// EVERY CHUNK CARRIES ITS SUBSTANCE'S FRICTION UNDER THE ONE RULE, standing or loose: a
    /// collider's friction is its material's row (`substances::PAIR_RULE`), so a pair of two
    /// chunks, or of a chunk and anything else stating the rule, is the geometric mean of the two
    /// rows. A chunk under any other rule (the `Max` that let one side win), or without its row's
    /// value, fails here; so does a pair whose coefficient is not `√(a·b)`.
    #[test]
    fn every_chunk_carries_its_substances_friction_under_the_one_rule() {
        use crate::substances::{PAIR_RULE, contact_friction};
        let kind = shipped_house();
        let other = contact_friction(0.3);
        assert!(!kind.layout.chunks.is_empty());
        for c in 0..kind.layout.chunks.len() {
            let friction = kind.chunk_friction(c);
            let element = &kind.asset.elements[usize::from(kind.layout.element_of[c])];
            let row = SubstanceRegistry::shipped_static()
                .hard(&kind.asset.materials[usize::from(element.material)])
                .unwrap()
                .friction
                .unwrap();
            assert_eq!(friction, contact_friction(row));
            assert_eq!(friction.combine_rule, PAIR_RULE);
            let pair = friction.combine(other).dynamic_coefficient;
            assert!(
                (pair - (row * 0.3).sqrt()).abs() < 1e-6,
                "{pair} against {row}"
            );
        }
    }

    /// THE INSTANCE'S SCALE SCALES ELEMENTS, NOT THE ROOT: at scale 1.5
    /// (fixture data) every element's collider spans 1.5 times its authored extent and the root
    /// stays at unit scale; the footprint spawn avoidance reads is the scaled bounds, posed.
    #[test]
    fn the_instance_scale_scales_every_element_and_the_footprint() {
        let kind = shipped_house();
        let pose = Transform::from_xyz(5.0, 2.0, -3.0).with_scale(Vec3::splat(1.5));
        let (mut world, root) = spawned(&kind, pose);
        assert_eq!(world.get::<Transform>(root).unwrap().scale, Vec3::ONE);
        let mut colliders = world.query::<(&Element, &Collider)>();
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

    /// A tilted construction is refused where it is still a pose (ADR-0011): its foundation plane
    /// would not be level and its anchors would hold nothing.
    #[test]
    #[should_panic(expected = "a construction stands upright")]
    fn a_tilted_construction_is_refused() {
        spawned(
            &shipped_house(),
            Transform::from_rotation(Quat::from_rotation_x(0.3)),
        );
    }

    /// A FILE THAT BREAKS THE CONTRACT DOES NOT LOAD: the shipped house parses and holds, and the
    /// same text naming another id, with an anchor lifted off the foundation plane, a joint out
    /// of order, an element whose run skips a chunk, a seam between two components or a bond
    /// inside one, a bond naming an attachment kind past the table, a kind named twice, a kind
    /// the shared list does not hold, or a material the registry does not know, is refused naming
    /// why. The laws read a joint's strength from its bond, so a mislabelled joint would hold a
    /// rest in tension, let a wall's own seam fall open, or nail a roof down by nothing.
    #[test]
    fn a_construction_file_that_breaks_the_contract_is_refused() {
        let path = shipped_assets().join("constructions/house_1/house_1.construction.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        let registry = SubstanceRegistry::shipped_static();
        let attachments = Attachments::shipped_static();
        let resolve =
            |id: &str, text: &str| ConstructionKind::resolve(id, text, registry, attachments);
        resolve("house_1", &text).expect("the shipped house holds");
        assert!(resolve("house_2", &text).is_err());
        let mut asset = ConstructionAsset::from_ron(&text).unwrap();
        asset.elements[0].part = " ".into();
        assert!(asset.check().unwrap_err().contains(".part is blank"));
        let mut asset = ConstructionAsset::from_ron(&text).unwrap();
        asset.anchors[0].centroid[1] += 0.1;
        assert!(
            asset
                .check()
                .unwrap_err()
                .contains("off the foundation plane")
        );
        let mut asset = ConstructionAsset::from_ron(&text).unwrap();
        asset.joints.swap(0, 1);
        assert!(asset.check().unwrap_err().contains("out of order"));
        let mut asset = ConstructionAsset::from_ron(&text).unwrap();
        asset.elements[1].chunks[0] += 1;
        assert!(
            asset
                .check()
                .unwrap_err()
                .contains("do not continue the run")
        );
        let asset = ConstructionAsset::from_ron(&text).unwrap();
        let attached = |bond: schema::Bond| matches!(bond, schema::Bond::Attached(_));
        for (from, to, refusal) in [
            (true, schema::Bond::Seam, "seam between"),
            (false, schema::Bond::Attached(0), "inside one"),
        ] {
            let mut wrong = asset.clone();
            let joint = wrong
                .joints
                .iter_mut()
                .find(|joint| attached(joint.bond) == from)
                .expect("the shipped house holds seams and attached bonds");
            joint.bond = to;
            assert!(wrong.check().unwrap_err().contains(refusal), "{refusal}");
        }
        let mut wrong = asset.clone();
        let joint = wrong
            .joints
            .iter_mut()
            .find(|joint| attached(joint.bond))
            .expect("the shipped house holds attached bonds");
        joint.bond = schema::Bond::Attached(u8::MAX);
        assert!(wrong.check().unwrap_err().contains("past the table"));
        let mut wrong = asset;
        wrong.attachments.push(wrong.attachments[0].clone());
        assert!(wrong.check().unwrap_err().contains("named twice"));
        let glued = text.replacen("\"nailed\"", "\"glued\"", 1);
        assert_ne!(glued, text, "the shipped house names a nailed joint");
        let err = resolve("house_1", &glued).unwrap_err();
        for part in ["attachments[", "\"glued\"", attachments::ATTACHMENTS_PATH] {
            assert!(err.contains(part), "{part}: {err}");
        }
        let unknown = text.replacen("\"Brick\"", "\"Adobe\"", 1);
        let err = resolve("house_1", &unknown).unwrap_err();
        assert!(err.contains("Adobe"), "{err}");
    }

    /// A NON-FINITE OR MALFORMED GEOMETRIC FACT DOES NOT LOAD: NaN passes every `>` and `<`
    /// comparison the contract once made, so a NaN element or chunk hull corner, joint normal or
    /// anchor centroid, or an element hull of three corners, would reach the colliders, faces and
    /// laws that read them. Each is refused naming its element, chunk, joint or anchor.
    #[test]
    fn a_non_finite_or_malformed_geometric_fact_is_refused() {
        let path = shipped_assets().join("constructions/house_1/house_1.construction.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        let asset = ConstructionAsset::from_ron(&text).unwrap();
        let refused = |edit: &dyn Fn(&mut ConstructionAsset), part: &str| {
            let mut wrong = asset.clone();
            edit(&mut wrong);
            let err = wrong.check().expect_err(part);
            assert!(err.contains(part), "{part}: {err}");
        };
        refused(&|a| a.foundation_y_m = f32::NAN, "foundation_y_m");
        refused(&|a| a.elements[0].hull[0][0] = f32::NAN, "hull");
        refused(&|a| a.elements[0].hull.truncate(3), "four");
        refused(&|a| a.chunks[0].hull[0][1] = f32::NAN, "hull or centroid");
        refused(
            &|a| a.joints[0].normal = [f32::NAN; 3],
            "normal is not unit",
        );
        refused(
            &|a| a.joints[0].centroid[2] = f32::NAN,
            "centroid is not finite",
        );
        refused(
            &|a| a.anchors[0].centroid[0] = f32::NAN,
            "centroid is not finite",
        );
    }

    /// AN ELEMENT HULL THAT BOUNDS NO SOLID DOES NOT LOAD: an intact element's collider, its law
    /// shape and its drawn faces are all its hull, so a hull with every corner on one plane (the
    /// shipped first element flattened onto its first corner's height, fixture data) is refused
    /// before play naming the element's field, never left to fail when its collider is built.
    #[test]
    fn an_element_hull_that_bounds_no_solid_is_refused_before_play() {
        let path = shipped_assets().join("constructions/house_1/house_1.construction.ron");
        let mut asset =
            ConstructionAsset::from_ron(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let height = asset.elements[0].hull[0][1];
        for corner in &mut asset.elements[0].hull {
            corner[1] = height;
        }
        let text = ron::ser::to_string(&asset).unwrap();
        let err = ConstructionKind::resolve(
            "house_1",
            &text,
            SubstanceRegistry::shipped_static(),
            Attachments::shipped_static(),
        )
        .unwrap_err();
        assert!(err.contains("elements[0].hull"), "{err}");
    }
}
