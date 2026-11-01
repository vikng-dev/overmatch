//! The game's damage-model view, game state only.
//!
//! The struck tank's stage view ([`StageView`]), which the corner draws: the vehicle's view glb,
//! named by the record's tank file, placed so the hull node sits at the origin (the record's
//! frame), with collision proxies hidden and every servo the tank file declares turned to the
//! record's angle for it, posed as the sim poses it ([`servo_pose`] of the node's authored
//! rotation).

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::{Corner, CornerRig, Exhibits};
use crate::geometry_lod::SourceDetail;
use crate::render_policy::VisualScope;
use crate::spec::TankSpec;
use crate::tank::{ServoRole, servo_pose};

/// The hull node every tank view names; the record's frame is its frame.
pub(super) const HULL_NODE: &str = "Hull";

/// A mesh of a tank view, classified once: the names of its node and its ancestors up to the
/// view's root, and the material it was authored with.
#[derive(Component)]
pub struct ViewMesh {
    pub names: Vec<String>,
    pub original: Handle<StandardMaterial>,
    pub tier: Tier,
}

/// What a mesh of a tank view is, by the tank's own data: the tank file's volume entry for its
/// nearest declared node says what the volume is (main armour, external armour or a component),
/// and a mesh under no declared node is drawing only, whatever material it wears.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tier {
    /// A declared main-armour volume.
    Armour,
    /// A declared external volume: skirts, wheels, tracks.
    External,
    /// A declared volume with component facets.
    Component,
    /// Not a declared volume, or wearing no substance: drawing only.
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
            None => Self::Detail,
            Some(crate::spec::VolumeSpec::MainArmour) => Self::Armour,
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
    let proxy = chain
        .iter()
        .any(|name| spec.colliders.iter().any(|collider| collider.node == *name));
    let tier = Tier::of(material, &chain, &spec.volumes);
    (chain, tier, proxy)
}

/// What the view mesh `entity` is, whose vehicle's tank file is `spec`: the
/// names of its node chain up to the view's root (where `is_root` holds), nearest first, its
/// authored material `original` named `material`, and its tier; and whether it is a collision
/// proxy.
fn classify(
    entity: Entity,
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
    vehicle: &str,
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
        Some(
            *transforms
                .get(node)
                .expect("the game's damage-model view's spawned glTF nodes carry Transform"),
        )
    });
    *transforms
        .get_mut(root)
        .expect("the game's damage-model stage root is spawned with Transform") =
        Transform::from_matrix(hull_pose.inverse());
    let mut servos = Vec::new();
    for (name, servo) in &spec.servos {
        let node = named(name).unwrap_or_else(|| {
            panic!("the game's damage-model view: {vehicle} servos[{name:?}] has no declared view node; export the declared node and rebuild the certified artifacts")
        });
        let transform = transforms.get(node).expect(
            "the game's damage-model view poses every declared servo from its spawned Transform",
        );
        servos.push(ServoNode {
            entity: node,
            name: name.clone(),
            role: servo.role(),
            rest: transform.rotation,
        });
    }
    Some((hull, servos))
}

/// Turn `servos` to the angles `servo_deg` (degrees, by node name); a servo it does not name is at
/// rest, which only a synthetic clip asks for (a simulated record names every declared servo).
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
    let Some((hull, servos)) = place(
        root,
        &view.vehicle,
        spec,
        &children,
        &names,
        &parents,
        &mut transforms,
    ) else {
        return;
    };
    for entity in children.iter_descendants(root) {
        let Ok((material, material_name)) = meshes.get(entity) else {
            continue;
        };
        let (mesh, proxy) = classify(
            entity,
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// The game's damage-model stage must reject a certified view that lacks a declared servo;
    /// otherwise the recorded path is drawn against an invented rest pose. FIXTURE DATA: one yaw.
    #[test]
    #[should_panic(expected = "servos[\"fixture servo\"] has no declared view node")]
    fn a_spawned_stage_requires_its_declared_servo_node() {
        let text =
            std::fs::read_to_string(crate::assets::asset_root().join(crate::tank::TANK_SPEC_PATH))
                .unwrap();
        let mut spec: TankSpec = ron::from_str(&text).unwrap();
        let servo = ron::from_str(
            "(role: Yaw, max_speed: 1.0, accel: 1.0, travel: Continuous, requires: [])",
        )
        .unwrap();
        spec.servos = std::collections::HashMap::from([("fixture servo".to_owned(), servo)]);
        let mut world = World::new();
        let root = world.spawn(Transform::IDENTITY).id();
        world.spawn((Name::new(HULL_NODE), Transform::IDENTITY, ChildOf(root)));
        world
            .run_system_once(
                move |children: Query<&Children>,
                      names: Query<&Name>,
                      parents: Query<&ChildOf>,
                      mut transforms: Query<&mut Transform>| {
                    place(
                        root,
                        "fixture.tank.ron",
                        &spec,
                        &children,
                        &names,
                        &parents,
                        &mut transforms,
                    );
                },
            )
            .unwrap();
    }
}
