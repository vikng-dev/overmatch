//! The struck tank's two views: one on the battlefield, seen through the shooter's sight, and one
//! on the pen cam stage. Both are the vehicle's view glb, named by the record's tank file, placed
//! so the hull node sits at the origin (the record's frame), with collision proxies hidden and the
//! turret traversed to the record's pose. The turret is the yaw servo above the gunner view's node
//! (the tank file's `views` and `servos`), as the rig binder finds it.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::{Corner, Library, Ready, Run};
use crate::render_policy::VisualScope;
use crate::spec::{TankSpec, ViewKind};
use crate::tank::ServoRole;

/// The hull node every tank view names; the record's frame is its frame.
const HULL_NODE: &str = "Hull";

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewRoot {
    World,
    Stage,
}

/// A mesh of a tank view, classified once: the names of its node and its ancestors up to the
/// view's root, and the material it was authored with.
#[derive(Component)]
pub struct ViewMesh {
    pub root: ViewRoot,
    pub names: Vec<String>,
    pub original: Handle<StandardMaterial>,
}

impl ViewMesh {
    /// Whether this mesh belongs to the volume named `name`. A record names a mixed node's
    /// substance `node/substance#k`; the node part matches.
    pub fn is(&self, name: &str) -> bool {
        let node = name.split('/').next().unwrap_or(name);
        self.names.iter().any(|n| n == node)
    }
}

/// A collision proxy: never drawn.
#[derive(Component)]
pub struct Proxy;

#[derive(Resource)]
pub struct Views {
    spec: Handle<TankSpec>,
    pub world: Entity,
    pub stage: Entity,
    /// The turret node and its rest rotation, per root.
    turret: [Option<(Entity, Quat)>; 2],
    /// The hull node, per root (world, stage): its frame is the record's.
    pub hull: [Option<Entity>; 2],
    placed: [bool; 2],
    /// The ground's height under the battlefield view, once measured.
    pub ground_y: Option<f32>,
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
        turret: [None, None],
        hull: [None, None],
        placed: [false, false],
        ground_y: None,
    });
}

/// The view glb beside a tank file: `<dir>/<id>.tank.ron` → `<dir>/<id>.glb`.
fn view_glb(vehicle: &str) -> String {
    let stem = vehicle.strip_suffix(".tank.ron").unwrap_or(vehicle);
    format!("{stem}.glb")
}

fn slot(root: ViewRoot) -> usize {
    match root {
        ViewRoot::World => 0,
        ViewRoot::Stage => 1,
    }
}

/// Classify new meshes, place each view once its hull node exists, measure the ground, and
/// declare the demo ready when both views are placed.
pub fn prepare(
    mut commands: Commands,
    mut views: ResMut<Views>,
    specs: Res<Assets<TankSpec>>,
    fresh: Query<(Entity, &MeshMaterial3d<StandardMaterial>), (With<Mesh3d>, Without<ViewMesh>)>,
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
    for (entity, material) in &fresh {
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
        let proxy = chain.iter().any(|name| spec.colliders.contains(name));
        let mut entity_commands = commands.entity(entity);
        entity_commands.insert(ViewMesh {
            root,
            names: chain,
            original: material.0.clone(),
        });
        if proxy {
            entity_commands.insert((Proxy, Visibility::Hidden));
        }
    }
    for (root, entity) in [
        (ViewRoot::World, views.world),
        (ViewRoot::Stage, views.stage),
    ] {
        let index = slot(root);
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
        // The hull's pose under the root, from local transforms (globals lag a frame).
        let mut hull_pose = Mat4::IDENTITY;
        for node in std::iter::once(hull).chain(parents.iter_ancestors(hull)) {
            if node == entity {
                break;
            }
            if let Ok(local) = transforms.get(node) {
                hull_pose = local.to_matrix() * hull_pose;
            }
        }
        if let Ok(mut root_transform) = transforms.get_mut(entity) {
            *root_transform = Transform::from_matrix(hull_pose.inverse());
        }
        views.hull[index] = Some(hull);
        views.turret[index] = turret_node(spec, &named, &parents, &names).and_then(|node| {
            transforms
                .get(node)
                .ok()
                .map(|transform| (node, transform.rotation))
        });
        if views.turret[index].is_none() {
            warn!("pen cam: no turret node found; the record's turret pose is not shown");
        }
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
fn turret_node(
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

/// Traverse each view's turret to the record it shows: the run's first shot on the battlefield,
/// the corner's record on the stage.
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
        let Some((turret, rest)) = views.turret[index] else {
            continue;
        };
        let Some(record) = record else {
            continue;
        };
        let yaw = library.records[record].report.turret_yaw_deg.to_radians();
        if let Ok(mut transform) = transforms.get_mut(turret) {
            let want = Quat::from_rotation_y(yaw) * rest;
            if transform.rotation != want {
                transform.rotation = want;
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
