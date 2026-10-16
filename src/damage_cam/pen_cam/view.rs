//! The struck tank's two views: one on the battlefield, seen through the shooter's sight, and one
//! on the pen cam stage. Both are the vehicle's view glb, named by the record's tank file, placed
//! so the hull node sits at the origin (the record's frame), with collision proxies hidden and every
//! servo the tank file declares turned to the record's angle for it, posed as the sim poses it
//! ([`servo_pose`] of the node's authored rotation).

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::{Corner, Library, Ready, Run};
use crate::render_policy::VisualScope;
use crate::spec::{TankSpec, ViewKind};
use crate::tank::{ServoRole, servo_pose};

/// The hull node every tank view names; the record's frame is its frame.
pub(super) const HULL_NODE: &str = "Hull";

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewRoot {
    World,
    Stage,
    /// The shooter's tank on the battlefield, placed by the kill cam.
    Shooter,
}

/// A mesh of a tank view, classified once: the names of its node and its ancestors up to the
/// view's root, and the material it was authored with.
#[derive(Component)]
pub struct ViewMesh {
    pub root: ViewRoot,
    pub names: Vec<String>,
    pub original: Handle<StandardMaterial>,
    pub tier: Tier,
}

/// What a mesh of a tank view is, by the tank's own data: membership is the substance material
/// it wears, and the tank file's volume entry for its nearest named node says what such a volume is
/// beyond armour.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tier {
    /// A substance volume with no entry: main armour.
    Armour,
    /// A substance volume the tank file tags external: skirts, wheels, tracks.
    External,
    /// A substance volume with component facets.
    Component,
    /// No substance: drawing only.
    Detail,
}

impl Tier {
    /// The tier of a mesh wearing material `material` (its datablock name) under the node chain
    /// `chain`, nearest first, of a tank whose volume entries are `volumes`.
    pub fn of(
        material: Option<&str>,
        chain: &[String],
        volumes: &std::collections::HashMap<String, crate::spec::VolumeSpec>,
    ) -> Self {
        let substance = material.is_some_and(|name| {
            crate::substances::SubstanceRegistry::shipped_static()
                .get(name)
                .is_ok()
        });
        if !substance {
            return Self::Detail;
        }
        match chain.iter().find_map(|name| volumes.get(name)) {
            None => Self::Armour,
            Some(crate::spec::VolumeSpec::External) => Self::External,
            Some(crate::spec::VolumeSpec::Component(_)) => Self::Component,
        }
    }
}

/// A collision proxy: never drawn.
#[derive(Component)]
pub struct Proxy;

/// One servo node of a view: its name in the tank file, its role and its authored rest rotation.
struct ServoNode {
    entity: Entity,
    name: String,
    role: ServoRole,
    rest: Quat,
}

#[derive(Resource)]
pub struct Views {
    spec: Handle<TankSpec>,
    pub world: Entity,
    pub stage: Entity,
    /// Every servo node, per root.
    servos: [Vec<ServoNode>; 2],
    /// The hull node, per root (world, stage): its frame is the record's.
    pub hull: [Option<Entity>; 2],
    placed: [bool; 2],
    /// The ground's height under the battlefield view, once measured.
    pub ground_y: Option<f32>,
    /// The shooter's tank file, once the kill cam stages a shooter: its view's proxies are its own.
    pub(super) shooter_spec: Option<Handle<TankSpec>>,
}

pub fn spawn(mut commands: Commands, asset_server: Res<AssetServer>, library: Res<Library>) {
    let glb = view_glb(&library.vehicle);
    let scene: Handle<WorldAsset> = asset_server.load(GltfAssetLabel::Scene(0).from_asset(glb));
    let world = commands
        .spawn((
            WorldAssetRoot(scene.clone()),
            Transform::IDENTITY,
            Visibility::Hidden,
            ViewRoot::World,
        ))
        .id();
    let stage = commands
        .spawn((
            WorldAssetRoot(scene),
            Transform::IDENTITY,
            Visibility::Hidden,
            ViewRoot::Stage,
            VisualScope::PEN_CAM_STAGE,
        ))
        .id();
    commands.insert_resource(Views {
        spec: asset_server.load(library.vehicle.clone()),
        world,
        stage,
        servos: [Vec::new(), Vec::new()],
        hull: [None, None],
        placed: [false, false],
        ground_y: None,
        shooter_spec: None,
    });
}

/// The view glb beside a tank file: `<dir>/<id>.tank.ron` → `<dir>/<id>.glb`.
pub(super) fn view_glb(vehicle: &str) -> String {
    let stem = vehicle.strip_suffix(".tank.ron").unwrap_or(vehicle);
    format!("{stem}.glb")
}

impl Views {
    /// Views of no scene, for tests of systems that read the resource.
    #[cfg(test)]
    pub(super) fn fixture() -> Self {
        Self {
            spec: Handle::default(),
            world: Entity::PLACEHOLDER,
            stage: Entity::PLACEHOLDER,
            servos: [Vec::new(), Vec::new()],
            hull: [None, None],
            placed: [false, false],
            ground_y: None,
            shooter_spec: None,
        }
    }

    /// Views of the tank file `spec` at the roots `world` and `stage`, on ground at `ground_y`, for
    /// tests that stand a scene in for the glb.
    #[cfg(test)]
    pub(super) fn standing_in(
        spec: Handle<TankSpec>,
        world: Entity,
        stage: Entity,
        ground_y: f32,
    ) -> Self {
        Self {
            spec,
            world,
            stage,
            ground_y: Some(ground_y),
            ..Self::fixture()
        }
    }

    /// The staged vehicle's tank file.
    pub(super) fn spec(&self) -> &Handle<TankSpec> {
        &self.spec
    }
}

/// The pose of `node` under its ancestor `root`, from local transforms (globals lag a frame).
pub(super) fn pose_under(
    node: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    local: impl Fn(Entity) -> Option<Transform>,
) -> Mat4 {
    let mut pose = Mat4::IDENTITY;
    for ancestor in std::iter::once(node).chain(parents.iter_ancestors(node)) {
        if ancestor == root {
            break;
        }
        if let Some(transform) = local(ancestor) {
            pose = transform.to_matrix() * pose;
        }
    }
    pose
}

/// Classify new meshes, place each view once its hull node exists, measure the ground, and
/// declare the demo ready when both views are placed.
pub fn prepare(
    mut commands: Commands,
    mut views: ResMut<Views>,
    specs: Res<Assets<TankSpec>>,
    fresh: Query<
        (
            Entity,
            &MeshMaterial3d<StandardMaterial>,
            Option<&bevy::gltf::GltfMaterialName>,
        ),
        (With<Mesh3d>, Without<ViewMesh>),
    >,
    roots: Query<&ViewRoot>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    names: Query<&Name>,
    mut transforms: Query<&mut Transform>,
    bounds: Query<(&ViewMesh, &Aabb, &GlobalTransform)>,
    mut ready: ResMut<Ready>,
) {
    let Some(spec) = specs.get(&views.spec) else {
        return;
    };
    for (entity, material, material_name) in &fresh {
        let mut chain = Vec::new();
        let mut root = None;
        for node in std::iter::once(entity).chain(parents.iter_ancestors(entity)) {
            if let Ok(found) = roots.get(node) {
                root = Some(*found);
                break;
            }
            if let Ok(name) = names.get(node) {
                chain.push(name.as_str().to_owned());
            }
        }
        let Some(root) = root else {
            continue;
        };
        // Each view's proxies are named by its own vehicle's tank file; a shooter's mesh waits
        // for the shooter's.
        let root_spec = match root {
            ViewRoot::Shooter => views.shooter_spec.as_ref().and_then(|h| specs.get(h)),
            ViewRoot::World | ViewRoot::Stage => Some(spec),
        };
        let Some(root_spec) = root_spec else {
            continue;
        };
        let proxy = chain.iter().any(|name| root_spec.colliders.contains(name));
        let tier = Tier::of(
            material_name.map(|name| name.0.as_str()),
            &chain,
            &root_spec.volumes,
        );
        let mut entity_commands = commands.entity(entity);
        entity_commands.insert(ViewMesh {
            root,
            names: chain,
            original: material.0.clone(),
            tier,
        });
        if proxy {
            entity_commands.insert((Proxy, Visibility::Hidden));
        }
    }
    for (index, entity) in [(0, views.world), (1, views.stage)] {
        if views.placed[index] {
            continue;
        }
        let named = |wanted: &str| {
            children
                .iter_descendants(entity)
                .find(|node| names.get(*node).is_ok_and(|n| n.as_str() == wanted))
        };
        let Some(hull) = named(HULL_NODE) else {
            continue;
        };
        let hull_pose = pose_under(hull, entity, &parents, |node| {
            transforms.get(node).ok().copied()
        });
        if let Ok(mut root_transform) = transforms.get_mut(entity) {
            *root_transform = Transform::from_matrix(hull_pose.inverse());
        }
        views.hull[index] = Some(hull);
        let mut servos = Vec::new();
        for (name, servo) in &spec.servos {
            let Some(node) = named(name) else {
                warn!("pen cam: no node {name} for the tank file's servo; it is drawn at rest");
                continue;
            };
            if let Ok(transform) = transforms.get(node) {
                servos.push(ServoNode {
                    entity: node,
                    name: name.clone(),
                    role: servo.role(),
                    rest: transform.rotation,
                });
            }
        }
        views.servos[index] = servos;
        views.placed[index] = true;
    }
    if views.placed.iter().all(|placed| *placed) && views.ground_y.is_none() {
        let lowest = bounds
            .iter()
            .filter(|(mesh, ..)| mesh.root == ViewRoot::World)
            .map(|(_, aabb, global)| {
                let center = Vec3::from(aabb.center);
                let half = Vec3::from(aabb.half_extents);
                let mut lowest = f32::INFINITY;
                for corner in 0..8 {
                    let sign = Vec3::new(
                        if corner & 1 == 0 { -1.0 } else { 1.0 },
                        if corner & 2 == 0 { -1.0 } else { 1.0 },
                        if corner & 4 == 0 { -1.0 } else { 1.0 },
                    );
                    lowest = lowest.min(global.transform_point(center + half * sign).y);
                }
                lowest
            })
            .fold(f32::INFINITY, f32::min);
        if lowest.is_finite() {
            views.ground_y = Some(lowest);
            ready.0 = true;
        }
    }
}

/// The yaw servo above the gunner view's node.
pub(super) fn turret_node(
    spec: &TankSpec,
    named: &impl Fn(&str) -> Option<Entity>,
    parents: &Query<&ChildOf>,
    names: &Query<&Name>,
) -> Option<Entity> {
    let gunner = named(&spec.views.get(&ViewKind::Gunner)?.node)?;
    std::iter::once(gunner)
        .chain(parents.iter_ancestors(gunner))
        .find(|node| {
            names.get(*node).is_ok_and(|name| {
                spec.servos
                    .get(name.as_str())
                    .is_some_and(|servo| servo.role() == ServoRole::Yaw)
            })
        })
}

/// Turn each view's servos to the record it shows: the run's first shot on the battlefield, the
/// corner's record on the stage.
pub fn pose(
    views: Res<Views>,
    library: Res<Library>,
    run: Res<Run>,
    corner: Res<Corner>,
    mut transforms: Query<&mut Transform>,
    mut visibility: Query<&mut Visibility>,
) {
    let world_record = run.shots.first().map(|shot| shot.record);
    let stage_record = corner.showing.map(|showing| showing.record);
    for (index, record) in [(0, world_record), (1, stage_record)] {
        let Some(record) = record else {
            continue;
        };
        let angles = &library.records[record].report.servo_deg;
        for servo in &views.servos[index] {
            let angle = angles.get(&servo.name).copied().unwrap_or(0.0).to_radians();
            if let Ok(mut transform) = transforms.get_mut(servo.entity) {
                let want = servo_pose(servo.role, servo.rest, angle);
                if transform.rotation != want {
                    transform.rotation = want;
                }
            }
        }
    }
    if views.placed[0]
        && let Ok(mut v) = visibility.get_mut(views.world)
    {
        v.set_if_neq(Visibility::Inherited);
    }
    if let Ok(mut v) = visibility.get_mut(views.stage) {
        v.set_if_neq(if corner.showing.is_some() && views.placed[1] {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}
