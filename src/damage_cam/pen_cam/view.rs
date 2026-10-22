//! The struck tank's views: the stage's ([`StageView`]), which the corner draws, and the demo
//! harness's battlefield view ([`Views`]). Each is the vehicle's view glb, named by the record's
//! tank file, placed so the hull node sits at the origin (the record's frame), with collision
//! proxies hidden and every servo the tank file declares turned to the record's angle for it,
//! posed as the sim poses it ([`servo_pose`] of the node's authored rotation).

use std::collections::BTreeMap;

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::demo::{Library, Ready, Run};
use super::{Corner, CornerRig, Exhibits};
use crate::geometry_lod::SourceDetail;
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

/// The stage's view of the struck tank: the vehicle's view glb, named by the record's tank file,
/// placed so its hull node sits at the origin (the record's frame), drawn by the stage's cameras
/// alone ([`VisualScope::PEN_CAM_STAGE`]) and at source detail ([`SourceDetail`]): a band derived
/// from the battlefield's view measures nothing about a tank its own camera frames close.
#[derive(Resource)]
pub struct StageView {
    /// The tank file it shows.
    pub vehicle: String,
    spec: Handle<TankSpec>,
    pub root: Entity,
    servos: Vec<ServoNode>,
    /// The hull node, once placed: its frame is the record's.
    pub hull: Option<Entity>,
}

impl StageView {
    /// Spawn the stage view of the tank file `vehicle` (relative to the asset root).
    pub fn spawn(commands: &mut Commands, asset_server: &AssetServer, vehicle: &str) -> Self {
        let (scene, spec) = Self::load(asset_server, vehicle);
        let root = commands
            .spawn((
                WorldAssetRoot(scene),
                Transform::IDENTITY,
                Visibility::Hidden,
                ViewRoot::Stage,
                VisualScope::PEN_CAM_STAGE,
                SourceDetail,
                CornerRig,
            ))
            .id();
        Self {
            vehicle: vehicle.to_owned(),
            spec,
            root,
            servos: Vec::new(),
            hull: None,
        }
    }

    /// The assets a stage view of the tank file `vehicle` is spawned from: its scene and its tank
    /// file. A holder of these handles keeps them loaded between views.
    pub fn load(
        asset_server: &AssetServer,
        vehicle: &str,
    ) -> (Handle<WorldAsset>, Handle<TankSpec>) {
        (
            asset_server.load(GltfAssetLabel::Scene(0).from_asset(view_glb(vehicle))),
            asset_server.load(vehicle.to_owned()),
        )
    }

    /// The stage view of the tank file `spec` at `root`, for tests that stand a scene in for the
    /// glb.
    #[cfg(test)]
    pub(super) fn standing_in(spec: Handle<TankSpec>, root: Entity) -> Self {
        Self {
            vehicle: String::new(),
            spec,
            root,
            servos: Vec::new(),
            hull: None,
        }
    }

    /// Whether the view is placed: its scene spawned, classified and moved into the record's
    /// frame.
    pub fn placed(&self) -> bool {
        self.hull.is_some()
    }
}

/// The view glb beside a tank file: `<dir>/<id>.tank.ron` → `<dir>/<id>.glb`.
pub(super) fn view_glb(vehicle: &str) -> String {
    let stem = vehicle.strip_suffix(".tank.ron").unwrap_or(vehicle);
    format!("{stem}.glb")
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

/// What the mesh `entity` of a tank is, whose tank file is `spec`: the names of its node chain up
/// to the tank's root (where `is_root` holds), nearest first; its tier, wearing the material named
/// `material` (its datablock name); and whether it is a collision proxy. The one classification of
/// a tank's mesh, for a view's ([`classify`]) and the live tank's alike.
pub(super) fn chain_and_tier(
    entity: Entity,
    material: Option<&str>,
    spec: &TankSpec,
    parents: &Query<&ChildOf>,
    is_root: impl Fn(Entity) -> bool,
    names: &Query<&Name>,
) -> (Vec<String>, Tier, bool) {
    let mut chain = Vec::new();
    for node in std::iter::once(entity).chain(parents.iter_ancestors(entity)) {
        if is_root(node) {
            break;
        }
        if let Ok(name) = names.get(node) {
            chain.push(name.as_str().to_owned());
        }
    }
    let proxy = chain.iter().any(|name| spec.colliders.contains(name));
    let tier = Tier::of(material, &chain, &spec.volumes);
    (chain, tier, proxy)
}

/// What the view mesh `entity` of the view `root` is, whose vehicle's tank file is `spec`: the
/// names of its node chain up to the view's root (where `is_root` holds), nearest first, its
/// authored material `original` named `material`, and its tier; and whether it is a collision
/// proxy.
#[expect(
    clippy::too_many_arguments,
    reason = "classifies one mesh: its entity, root, materials and spec, and the hierarchy reads with the root predicate"
)]
fn classify(
    entity: Entity,
    root: ViewRoot,
    original: Handle<StandardMaterial>,
    material: Option<&str>,
    spec: &TankSpec,
    parents: &Query<&ChildOf>,
    is_root: impl Fn(Entity) -> bool,
    names: &Query<&Name>,
) -> (ViewMesh, bool) {
    let (chain, tier, proxy) = chain_and_tier(entity, material, spec, parents, is_root, names);
    (
        ViewMesh {
            root,
            names: chain,
            original,
            tier,
        },
        proxy,
    )
}

/// Place the view at `root` once its hull node exists: the root moved so the hull sits at the
/// origin, and every servo the tank file `spec` declares found under it. `None` until the scene
/// has spawned.
fn place(
    root: Entity,
    spec: &TankSpec,
    children: &Query<&Children>,
    names: &Query<&Name>,
    parents: &Query<&ChildOf>,
    transforms: &mut Query<&mut Transform>,
) -> Option<(Entity, Vec<ServoNode>)> {
    let named = |wanted: &str| {
        children
            .iter_descendants(root)
            .find(|node| names.get(*node).is_ok_and(|n| n.as_str() == wanted))
    };
    let hull = named(HULL_NODE)?;
    let hull_pose = pose_under(hull, root, parents, |node| {
        transforms.get(node).ok().copied()
    });
    if let Ok(mut root_transform) = transforms.get_mut(root) {
        *root_transform = Transform::from_matrix(hull_pose.inverse());
    }
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
    Some((hull, servos))
}

/// Turn `servos` to the angles `servo_deg` (degrees, by node name); a servo it does not name is at
/// rest.
fn pose_servos(
    servos: &[ServoNode],
    servo_deg: &BTreeMap<String, f32>,
    transforms: &mut Query<&mut Transform>,
) {
    for servo in servos {
        let angle = servo_deg
            .get(&servo.name)
            .copied()
            .unwrap_or(0.0)
            .to_radians();
        if let Ok(mut transform) = transforms.get_mut(servo.entity) {
            let want = servo_pose(servo.role, servo.rest, angle);
            if transform.rotation != want {
                transform.rotation = want;
            }
        }
    }
}

/// Place the stage view once its scene has spawned and its tank file has loaded: every mesh under
/// its root classified at once, since a scene spawns whole, and the root placed. Idle once placed.
#[expect(
    clippy::too_many_arguments,
    reason = "places the stage view: its scene's meshes, hierarchy, names, transforms and the tank file are separate reads"
)]
pub fn prepare_stage(
    mut commands: Commands,
    mut view: ResMut<StageView>,
    specs: Res<Assets<TankSpec>>,
    meshes: Query<
        (
            &MeshMaterial3d<StandardMaterial>,
            Option<&bevy::gltf::GltfMaterialName>,
        ),
        (With<Mesh3d>, Without<ViewMesh>),
    >,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    names: Query<&Name>,
    mut transforms: Query<&mut Transform>,
) {
    if view.placed() {
        return;
    }
    let Some(spec) = specs.get(&view.spec) else {
        return;
    };
    let root = view.root;
    let Some((hull, servos)) = place(root, spec, &children, &names, &parents, &mut transforms)
    else {
        return;
    };
    for entity in children.iter_descendants(root) {
        let Ok((material, material_name)) = meshes.get(entity) else {
            continue;
        };
        let (mesh, proxy) = classify(
            entity,
            ViewRoot::Stage,
            material.0.clone(),
            material_name.map(|name| name.0.as_str()),
            spec,
            &parents,
            |node| node == root,
            &names,
        );
        let mut entity_commands = commands.entity(entity);
        entity_commands.insert(mesh);
        if proxy {
            entity_commands.insert((Proxy, Visibility::Hidden));
        }
    }
    view.hull = Some(hull);
    view.servos = servos;
}

/// Turn the stage view's servos to the corner's record, and show the view while the corner shows
/// one.
pub fn pose_stage(
    view: Res<StageView>,
    exhibits: Res<Exhibits>,
    corner: Res<Corner>,
    mut transforms: Query<&mut Transform>,
    mut visibility: Query<&mut Visibility>,
) {
    let exhibit = corner
        .showing
        .and_then(|showing| exhibits.0.get(showing.record));
    if let Some(exhibit) = exhibit {
        pose_servos(&view.servos, &exhibit.servo_deg, &mut transforms);
    }
    if let Ok(mut v) = visibility.get_mut(view.root) {
        v.set_if_neq(if exhibit.is_some() && view.placed() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
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

/// The demo's battlefield view of the struck tank, seen through the shooter's sight, and the
/// tank file of the shooter's tank the kill cam stages.
#[derive(Resource)]
pub struct Views {
    spec: Handle<TankSpec>,
    pub world: Entity,
    servos: Vec<ServoNode>,
    placed: bool,
    /// The ground's height under the battlefield view, once measured.
    pub ground_y: Option<f32>,
    /// The shooter's tank file, once the kill cam stages a shooter: its view's proxies are its own.
    pub(super) shooter_spec: Option<Handle<TankSpec>>,
}

/// The demo's views of the library's vehicle: the battlefield's and the stage's.
pub fn spawn(mut commands: Commands, asset_server: Res<AssetServer>, library: Res<Library>) {
    let scene: Handle<WorldAsset> =
        asset_server.load(GltfAssetLabel::Scene(0).from_asset(view_glb(&library.vehicle)));
    let world = commands
        .spawn((
            WorldAssetRoot(scene),
            Transform::IDENTITY,
            Visibility::Hidden,
            ViewRoot::World,
        ))
        .id();
    let stage = StageView::spawn(&mut commands, &asset_server, &library.vehicle);
    commands.insert_resource(stage);
    commands.insert_resource(Views {
        spec: asset_server.load(library.vehicle.clone()),
        world,
        servos: Vec::new(),
        placed: false,
        ground_y: None,
        shooter_spec: None,
    });
}

impl Views {
    /// The staged vehicle's tank file.
    pub(super) fn spec(&self) -> &Handle<TankSpec> {
        &self.spec
    }
}

/// Classify the battlefield views' new meshes, place the struck tank's once its hull node exists,
/// measure the ground, and declare the demo ready once the stage view is placed too.
#[expect(
    clippy::too_many_arguments,
    reason = "classifies the battlefield views: fresh meshes, roots, hierarchy, names, transforms, bounds and readiness"
)]
pub fn prepare(
    mut commands: Commands,
    mut views: ResMut<Views>,
    stage: Res<StageView>,
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
        let root = std::iter::once(entity)
            .chain(parents.iter_ancestors(entity))
            .find_map(|node| roots.get(node).ok().copied());
        // Each view's proxies are named by its own vehicle's tank file; a shooter's mesh waits
        // for the shooter's. The stage view's meshes are [`prepare_stage`]'s.
        let root_spec = match root {
            Some(ViewRoot::Shooter) => views.shooter_spec.as_ref().and_then(|h| specs.get(h)),
            Some(ViewRoot::World) => Some(spec),
            Some(ViewRoot::Stage) | None => None,
        };
        let (Some(root), Some(root_spec)) = (root, root_spec) else {
            continue;
        };
        let (mesh, proxy) = classify(
            entity,
            root,
            material.0.clone(),
            material_name.map(|name| name.0.as_str()),
            root_spec,
            &parents,
            |node| roots.contains(node),
            &names,
        );
        let mut entity_commands = commands.entity(entity);
        entity_commands.insert(mesh);
        if proxy {
            entity_commands.insert((Proxy, Visibility::Hidden));
        }
    }
    if !views.placed
        && let Some((_, servos)) = place(
            views.world,
            spec,
            &children,
            &names,
            &parents,
            &mut transforms,
        )
    {
        views.servos = servos;
        views.placed = true;
    }
    if views.placed && stage.placed() && views.ground_y.is_none() {
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

/// Turn the battlefield view's servos to the run's first shot, and show it once placed.
pub fn pose(
    views: Res<Views>,
    library: Res<Library>,
    run: Res<Run>,
    mut transforms: Query<&mut Transform>,
    mut visibility: Query<&mut Visibility>,
) {
    if let Some(shot) = run.shots.first() {
        let angles = &library.records[shot.record].report.servo_deg;
        pose_servos(&views.servos, angles, &mut transforms);
    }
    if views.placed
        && let Ok(mut v) = visibility.get_mut(views.world)
    {
        v.set_if_neq(Visibility::Inherited);
    }
}
