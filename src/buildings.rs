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
//! NOT TERRAIN. No element enters the [`crate::world::TerrainMap`] block list, so the terrain
//! field stays immutable and the floor inside is the terrain (playtest fork F9). The belts meet a
//! standing element or chunk only as a collider of the rubble overlay (`track::rubble`'s
//! `RubbleQuery`), as they meet loose pieces. Spawn avoidance reads each
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

pub mod attachments;
pub(crate) mod breakage;
pub(crate) mod fracture;
pub mod schema;
pub(crate) mod view;

use attachments::Attachments;
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
    /// Per entry of the asset's material table: the friction its substance's colliders carry
    /// (`substances::HardSubstance::contact_friction`).
    friction: Vec<avian3d::prelude::Friction>,
    /// The bounds of the elements and of every chunk that tiles them, in the building frame at
    /// scale 1, `(min, max)`: every collider the building ever stands, intact or broken.
    bounds: (Vec3, Vec3),
    /// What the fracture laws read of it.
    pub(crate) layout: breakage::Layout,
}

impl BuildingKind {
    /// Resolve asset `id`'s text against the substance registry and the attachment kinds. Every
    /// refusal is a broken ship (ADR-0011): the message names what, the caller names the file.
    fn resolve(
        id: &str,
        text: &str,
        registry: &SubstanceRegistry,
        attachments: &Attachments,
    ) -> Result<Self, String> {
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
        let masonry = asset
            .materials
            .iter()
            .map(|name| fracture::Masonry::of(registry, name))
            .collect::<Result<Vec<_>, _>>()?;
        let layout = breakage::Layout::of(&asset, &masonry, attachments)?;
        Ok(Self {
            asset,
            materials,
            friction,
            bounds,
            layout,
        })
    }

    /// The friction element `k`'s colliders carry, standing or loose: its substance's.
    pub(crate) fn element_friction(&self, k: usize) -> avian3d::prelude::Friction {
        self.friction[usize::from(self.asset.elements[k].material)]
    }

    /// The friction law chunk `c`'s collider carries, standing or loose: its element's.
    pub(crate) fn chunk_friction(&self, c: usize) -> avian3d::prelude::Friction {
        self.element_friction(usize::from(self.layout.element_of[c]))
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
        let attachments = Attachments::shipped_static();
        let mut library = Self::default();
        for id in ids {
            let path = root
                .join(BUILDINGS_DIR)
                .join(id)
                .join(format!("{id}.building.ron"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
                panic!("buildings: {} cannot be read: {err}", path.display())
            });
            let kind = BuildingKind::resolve(id, &text, registry, attachments)
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

/// A building's root: a structure, whose contacts the solver's reading keeps.
#[derive(Component, Debug, Clone, Copy)]
#[require(crate::contact_transfer::StructureBody)]
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
        kind.element_friction(k),
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

    /// The immutable shipped fixture, resolved through the real loader once per test process.
    /// Loader refusal tests read their own inputs; sharing this fixture repeats no hull construction.
    pub(crate) fn shipped_house() -> Arc<BuildingKind> {
        static HOUSE: std::sync::OnceLock<Arc<BuildingKind>> = std::sync::OnceLock::new();
        HOUSE
            .get_or_init(|| {
                BuildingLibrary::load(&shipped_assets(), ["house_1"])
                    .get("house_1")
                    .expect("the shipped house")
                    .clone()
            })
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
    /// of order, an element whose run skips a chunk, a seam between two components or a bond
    /// inside one, a bond naming an attachment kind past the table, a kind named twice, a kind
    /// the shared list does not hold, or a material the registry does not know, is refused naming
    /// why. The laws read a joint's strength from its bond, so a mislabelled joint would hold a
    /// rest in tension, let a wall's own seam fall open, or nail a roof down by nothing.
    #[test]
    fn a_building_file_that_breaks_the_contract_is_refused() {
        let path = shipped_assets().join("buildings/house_1/house_1.building.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        let registry = SubstanceRegistry::shipped_static();
        let attachments = Attachments::shipped_static();
        let resolve = |id: &str, text: &str| BuildingKind::resolve(id, text, registry, attachments);
        resolve("house_1", &text).expect("the shipped house holds");
        assert!(resolve("house_2", &text).is_err());
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
        let asset = BuildingAsset::from_ron(&text).unwrap();
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
    /// comparison the contract once made, so a NaN profile, frame axis, hull corner, joint normal
    /// or anchor centroid, a skewed frame or a concave profile would reach the colliders, faces
    /// and laws that read them. Each is refused naming its element, chunk, joint or anchor.
    #[test]
    fn a_non_finite_or_malformed_geometric_fact_is_refused() {
        let path = shipped_assets().join("buildings/house_1/house_1.building.ron");
        let text = std::fs::read_to_string(&path).unwrap();
        let asset = BuildingAsset::from_ron(&text).unwrap();
        let refused = |edit: &dyn Fn(&mut BuildingAsset), part: &str| {
            let mut wrong = asset.clone();
            edit(&mut wrong);
            let err = wrong.check().expect_err(part);
            assert!(err.contains(part), "{part}: {err}");
        };
        refused(&|a| a.foundation_y_m = f32::NAN, "foundation_y_m");
        refused(&|a| a.elements[0].profile[0][0] = f32::NAN, "profile");
        refused(&|a| a.elements[0].frame.u[0] = f32::NAN, "frame");
        refused(
            &|a| a.elements[0].frame.v = a.elements[0].frame.u,
            "orthogonal",
        );
        refused(&|a| a.elements[0].depth[1] = f32::INFINITY, "depth");
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
        refused(
            &|a| {
                let profile = &mut a
                    .elements
                    .iter_mut()
                    .find(|element| element.profile.len() >= 4)
                    .expect("the shipped house holds a profile of four corners or more")
                    .profile;
                let mean = profile
                    .iter()
                    .fold([0.0; 2], |sum, p| [sum[0] + p[0], sum[1] + p[1]]);
                let n = profile.len() as f32;
                // A corner pulled just past the centre dents the polygon and keeps its area positive.
                let centre = [mean[0] / n, mean[1] / n];
                profile[0] = [
                    centre[0] - 0.25 * (profile[0][0] - centre[0]),
                    centre[1] - 0.25 * (profile[0][1] - centre[1]),
                ];
            },
            "not convex",
        );
    }

    /// Collider and face readers require a simple convex boundary. Local left turns alone
    /// admit a self-intersecting star (ASSUMED fixture geometry), whose faces cross its hull.
    #[test]
    fn a_self_intersecting_profile_is_refused_before_play() {
        let mut asset = shipped_house().asset.clone();
        asset.elements[0].profile = (0..5)
            .map(|k| {
                let angle = (k * 2) as f32 * std::f32::consts::TAU / 5.0;
                [angle.cos(), angle.sin()]
            })
            .collect();
        let error = asset.check().expect_err("a star is no convex boundary");
        assert!(
            error.contains("profile") && error.contains("split"),
            "{error}"
        );
    }
    /// A boundary traced twice has only left turns and passes every edge half-plane test,
    /// but doubles the face list and profile area. Collider and face readers need one boundary.
    #[test]
    fn a_profile_traced_twice_is_refused_before_play() {
        let mut asset = shipped_house().asset.clone();
        let boundary = asset.elements[0].profile.clone();
        asset.elements[0].profile.extend(boundary);
        let error = asset
            .check()
            .expect_err("a repeated boundary is no simple polygon");
        assert!(
            error.contains("profile") && error.contains("remove repeated vertices"),
            "{error}"
        );
    }
    /// Collider and face readers require a simple boundary even when crossings fit inside
    /// the plane tolerance. ASSUMED fixture: a unit-radius 501-gon visited in steps of two.
    #[test]
    fn a_dense_star_profile_is_refused_before_play() {
        let mut asset = shipped_house().asset.clone();
        asset.elements[0].profile = (0..501)
            .map(|k| {
                let angle = (k * 2) as f32 * std::f32::consts::TAU / 501.0;
                [angle.cos(), angle.sin()]
            })
            .collect();
        let error = asset
            .check()
            .expect_err("crossing edges cannot bound a prism");
        assert!(
            error.contains("profile") && error.contains("split"),
            "{error}"
        );
    }
    /// Collinear edges that retrace one another have no proper crossing and no strict
    /// orientation change, yet trace the boundary twice. ASSUMED fixture: distinct vertices,
    /// positive area and every vertex within the plane tolerance of every edge half-plane.
    #[test]
    fn a_profile_retracing_a_collinear_edge_is_refused_before_play() {
        let mut asset = shipped_house().asset.clone();
        asset.elements[0].profile = vec![
            [0.0, 0.0],
            [4.0, 0.0],
            [1.0, 0.0],
            [3.0, 0.0],
            [5.0, 0.00005],
            [-1.0, 0.00005],
        ];
        let error = asset
            .check()
            .expect_err("a retraced edge cannot bound a prism");
        assert!(
            error.contains("profile") && error.contains("split"),
            "{error}"
        );
    }
}
