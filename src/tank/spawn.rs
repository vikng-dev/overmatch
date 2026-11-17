use bevy::asset::LoadState;
use bevy::prelude::*;

use super::model::Tank;
use super::servo::{RemoteServos, ServoMechanism};
use super::template::TankTemplate;
use super::view::bind_tank_view;
use crate::spec::{TankSpec, TankSpecHandle};
use crate::track::bump_stop::{spawn_replica_track_colliders, spawn_track_colliders};

/// Presentation handles. Loading may gate admission or view attachment, never simulation data.
///
/// `scene` is `None` on the DEDICATED SERVER, which opens no view artifact at all (ADR-0035): the
/// glb is scene, textures and every rung, and nothing the server simulates reads any of it. The
/// walk's geometry comes from `bake`'s extraction of `<id>.sim.glb`, which is not an asset-server
/// load at all.
#[derive(Resource, Clone)]
pub(crate) struct PendingTankAssets {
    pub spec: Handle<TankSpec>,
    pub scene: Option<Handle<bevy::world_serialization::WorldAsset>>,
    /// Client presentation dependencies, loaded before admission; empty on the dedicated server.
    pub effects: Vec<(String, bevy::asset::UntypedHandle)>,
}

impl PendingTankAssets {
    /// Every presentation asset this composition asked for has resolved.
    ///
    /// THE ONE READINESS PREDICATE — single player, net client and net server all gate admission on
    /// it — so it is also the one place a FAILED load has to be caught. `LoadState::Failed` is not
    /// `Loaded`, and read as "not yet" it parks the app in `AppState::Loading` for the rest of the
    /// session: no tank, no error, no exit. Required data, so it panics in every build naming the
    /// asset (ADR-0011).
    pub(crate) fn loaded(&self, asset_server: &AssetServer) -> bool {
        let resolved = |what: &str, state: LoadState| match state {
            LoadState::Loaded => true,
            LoadState::Failed(err) => panic!(
                "required tank asset `{what}` failed to load: {err}. The tank's spec sheet and its                  view artifact are required data — there is no default tank and no detail level to                  fall back to"
            ),
            _ => false,
        };
        resolved(TANK_SPEC_PATH, asset_server.load_state(&self.spec))
            && self
                .scene
                .as_ref()
                .is_none_or(|scene| resolved(TANK_GLB_PATH, asset_server.load_state(scene)))
            && self
                .effects
                .iter()
                .all(|(path, handle)| resolved(path, asset_server.load_state(handle.id())))
    }

    /// Clone handles for a root; the spec handle remains available to presentation validation.
    pub(crate) fn presentation(&self) -> TankPresentation {
        TankPresentation::new(self.scene.clone(), self.spec.clone())
    }
}

/// Presentation-only root handles, deliberately separate from [`TankTemplate`].
#[derive(Clone)]
pub(crate) struct TankPresentation {
    scene: Option<Handle<bevy::world_serialization::WorldAsset>>,
    spec: Handle<TankSpec>,
}

impl TankPresentation {
    pub(crate) fn new(
        scene: Option<Handle<bevy::world_serialization::WorldAsset>>,
        spec: Handle<TankSpec>,
    ) -> Self {
        Self { scene, spec }
    }

    /// The view artifact's scene root, ABSENT on the dedicated server: no scene to instantiate,
    /// and therefore no view bind, no shadow compare and no textures.
    pub(super) fn scene_root(&self) -> Option<WorldAssetRoot> {
        self.scene.clone().map(WorldAssetRoot)
    }

    pub(super) fn root_bundle(&self) -> impl Bundle {
        (
            TankSpecHandle(self.spec.clone()),
            Tank,
            // The ROOT of this body's rendering policy: the glb lands asynchronously over many
            // frames and every leaf of it inherits from here, so the scope must exist before the
            // first mesh does. Ordinary world geometry until the local player takes control, at
            // which point `sight::mark_view_subject_body` flips this ONE component to
            // `VIEW_SUBJECT_BODY` and the whole body follows (`render_policy`).
            crate::render_policy::VisualScope::WORLD_SOLID,
        )
    }
}

/// The VIEW artifact — scene, textures and every certified rung. The presentation loader's file,
/// and no simulation's (ADR-0035).
pub(crate) const TANK_GLB_PATH: &str = crate::assets::tank_file!(".glb");

/// The SIM artifact — rung-0 geometry and material names, no textures, no UVs, no rungs. The ONE
/// file the ballistic/armour walk is extracted from, on the dedicated server and on the client
/// alike, so both sides walk identical accessor bytes by construction rather than by convention.
pub(crate) const TANK_SIM_GLB_PATH: &str = crate::assets::tank_file!(".sim.glb");

/// The spec sheet beside them.
pub(crate) const TANK_SPEC_PATH: &str = crate::assets::tank_file!(".tank.ron");

/// The windowed compositions' load: the spec sheet and the view artifact's scene.
pub(crate) fn load_tank_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    effects: Option<Res<crate::assets::PresentationLoads>>,
) {
    commands.insert_resource(PendingTankAssets {
        spec: asset_server.load(TANK_SPEC_PATH),
        scene: Some(asset_server.load(GltfAssetLabel::Scene(0).from_asset(TANK_GLB_PATH))),
        // No view plugins means no client effects (headless test composition).
        effects: effects.map_or_else(Vec::new, |loads| loads.0.clone()),
    });
}

/// The dedicated server's load: the spec sheet, and nothing else. The sim artifact reaches the
/// server through `bake`'s startup extraction, not through the asset server.
pub(crate) fn load_tank_sim_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(PendingTankAssets {
        spec: asset_server.load(TANK_SPEC_PATH),
        scene: None,
        effects: Vec::new(),
    });
}

/// Spawn a root and its complete local simulation body in one command batch.
pub(crate) fn spawn_complete_tank<B: Bundle>(
    commands: &mut Commands,
    template: &TankTemplate,
    presentation: TankPresentation,
    root_bundle: B,
) -> Entity {
    let mut root = commands.spawn((
        presentation.root_bundle(),
        template.authority_state(),
        root_bundle,
    ));
    if let Some(scene) = presentation.scene_root() {
        root.insert(scene);
    }
    root.observe(bind_tank_view);
    let entity = root.id();
    template.assemble(commands, entity);
    spawn_track_colliders(commands, entity, template.track_colliders());
    entity
}

/// Spawn the production simulation body without presentation assets.
///
/// Two instruments are deliberately headless and share this: the bitprobe differential harness and
/// the §13.6 ray fuzzer (`ballistics::fuzz`). Both need the same template-built body, colliders and
/// rollback-visible state as an authority tank, and neither wants a `WorldAssetRoot`, an asset
/// handle, or a view-binding observer. [`TankTemplate::assemble`] remains the one simulation
/// constructor.
pub(crate) fn spawn_headless_tank<B: Bundle>(
    commands: &mut Commands,
    template: &TankTemplate,
    root_bundle: B,
) -> Entity {
    let root = commands
        .spawn((Tank, template.authority_state(), root_bundle))
        .id();
    template.assemble(commands, root);
    spawn_track_colliders(commands, root, template.track_colliders());
    root
}

/// Transitional ADR-0014 exception for `net::rig`: attach to a replicated root with a valid pose
/// and its authoritative [`EngineRpm`](crate::track::sim::EngineRpm) (and, on the owner's own tank,
/// [`TankTransmission`](crate::track::sim::TankTransmission)) already present. Normal spawn paths
/// must use [`spawn_complete_tank`].
pub(crate) fn attach_replicated_tank_body<B: Bundle>(
    commands: &mut Commands,
    root: Entity,
    template: &TankTemplate,
    presentation: TankPresentation,
    root_bundle: B,
) {
    let mut root_commands = commands.entity(root);
    if let Some(scene) = presentation.scene_root() {
        root_commands.insert(scene);
    }
    root_commands
        .insert((
            presentation.root_bundle(),
            ServoMechanism::new(template.transmission()),
            // Replicated authority state (EngineRpm, the public ServoAngles; the owner's
            // TankTransmission, DriveSignal, WeaponGate, WeaponLoads) arrived in the replication
            // init snapshot. Do not overwrite it with fresh spec-derived values.
            root_bundle,
        ))
        .observe(bind_tank_view);
    // A replica's client servo state; the authority's `TankServos` is never manufactured here.
    root_commands.insert(RemoteServos::for_count(template.spec().servos.len()));
    template.assemble(commands, root);
    spawn_replica_track_colliders(commands, root, template.track_colliders());
}

#[cfg(test)]
mod proxy_tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::Layer;
    use crate::bake::TankGeometry;
    use crate::ballistics::WitnessVolume;
    use crate::ballistics::armour_set::{ArmourSet, Role};
    use crate::substances::Matter;
    use crate::tank::template::shipped_construction;
    use crate::tank::{Rig, SimParts};
    use avian3d::prelude::{Collider, CollisionLayers};

    /// An authority tank and a replica of the shipped fixture, spawned into one world.
    fn authority_and_replica(world: &mut World) -> (Entity, Entity) {
        use crate::track::sim::{EngineRpm, TankTransmission};
        use avian3d::prelude::{Position, RigidBody, Rotation};
        let (_, template) = shipped_construction();
        let mut commands = world.commands();
        let authority = spawn_headless_tank(
            &mut commands,
            &template,
            (RigidBody::Dynamic, Transform::IDENTITY),
        );
        let replica = commands
            .spawn((
                Position::default(),
                Rotation::default(),
                EngineRpm::of(&TankTransmission::from_spec(template.transmission())),
            ))
            .id();
        attach_replicated_tank_body(
            &mut commands,
            replica,
            &template,
            TankPresentation::new(None, Handle::default()),
            (RigidBody::Static, Transform::IDENTITY),
        );
        world.flush();
        (authority, replica)
    }

    /// Two tanks spawned from one template are each their own: every entity a tank's body names
    /// (its rig, its view's part table, its armour set's frames and volumes, each node's and
    /// part's tank link, a weapon's barrel) lies in its own hierarchy. A template holding one
    /// instance's entities, or an instantiation reusing another's member table, hands the second
    /// tank the first's turret, armour or crew HP pools. Every spawn after the first relies on it.
    /// Fixture: the shipped tank, twice as authority.
    #[test]
    fn two_tanks_from_one_template_name_only_their_own_entities() {
        use crate::ballistics::armour_set::ArmourPart;
        use crate::damage::VolumeOf;
        use crate::tank::{TankRoot, Weapon};
        use avian3d::prelude::RigidBody;
        let (_, template) = shipped_construction();
        let mut world = World::new();
        let mut commands = world.commands();
        let tanks = [(); 2].map(|()| {
            spawn_headless_tank(
                &mut commands,
                &template,
                (RigidBody::Dynamic, Transform::IDENTITY),
            )
        });
        world.flush();
        for tank in tanks {
            let mut own = HashSet::from([tank]);
            let mut walk = vec![tank];
            while let Some(entity) = walk.pop() {
                for &child in world.get::<Children>(entity).into_iter().flatten() {
                    own.insert(child);
                    walk.push(child);
                }
            }
            let rig = world
                .get::<Rig>(tank)
                .expect("a spawned tank carries its rig");
            let set = world
                .get::<ArmourSet>(tank)
                .expect("a spawned tank carries its armour set");
            let mut named: Vec<Entity> = vec![rig.hull, rig.turret, rig.gunner_yaw, rig.gun];
            named.push(rig.muzzle);
            named.extend(
                world
                    .get::<SimParts>(tank)
                    .expect("its part table")
                    .0
                    .values(),
            );
            named.extend(set.frame_nodes());
            named.extend(set.roles().map(Role::volume));
            for &entity in &own {
                let linked = world.entity(entity);
                named.extend(linked.get::<TankRoot>().map(|root| root.0));
                named.extend(linked.get::<VolumeOf>().map(|root| root.0));
                named.extend(linked.get::<ArmourPart>().map(ArmourPart::tank));
                named.extend(linked.get::<Weapon>().and_then(|weapon| weapon.barrel));
            }
            let strays: Vec<Entity> = named
                .into_iter()
                .filter(|entity| !own.contains(entity))
                .collect();
            assert!(
                strays.is_empty(),
                "tank {tank} names entities outside its hierarchy: {strays:?}"
            );
        }
    }

    /// The game's netcode: a replica's body attaches around the authority state its join-in-progress
    /// snapshot carried. The crank and the owner's drivetrain, weapon gate and grip field survive
    /// attachment as they arrived, and the root gains the client's servo
    /// integrator, never the authority's. A late joiner relies on it to see the tank in play, not
    /// a fresh one rebuilt from the spec. Fixture: the shipped tank's spawn state, moved off it.
    #[test]
    fn a_replica_attaches_around_its_join_in_progress_snapshot() {
        use crate::sim_tick::SimTick;
        use crate::tank::{TankServos, WeaponGate, WeaponTiming};
        use crate::track::sim::{EngineRpm, TankTransmission, TrackGripElements};
        use avian3d::prelude::{Position, RigidBody, Rotation};
        let (_, template) = shipped_construction();
        let mut world = World::new();
        let mut commands = world.commands();
        let authority = spawn_headless_tank(
            &mut commands,
            &template,
            (RigidBody::Dynamic, Transform::IDENTITY),
        );
        world.flush();
        let mut transmission = *world.get::<TankTransmission>(authority).unwrap();
        transmission.0.gear += 1;
        let mut gate = world.get::<WeaponGate>(authority).unwrap().clone();
        for state in &mut gate.weapons {
            state.timing = WeaponTiming::Interval {
                ready_tick: SimTick::new(4321),
            };
        }
        let mut grip = world.get::<TrackGripElements>(authority).unwrap().clone();
        grip.sides[0].strain[0] = Vec3::X;
        let snapshot = (EngineRpm(1234), transmission, gate, grip);
        let replica = world
            .spawn((Position::default(), Rotation::default(), snapshot.clone()))
            .id();
        let mut commands = world.commands();
        attach_replicated_tank_body(
            &mut commands,
            replica,
            &template,
            TankPresentation::new(None, Handle::default()),
            (RigidBody::Static, Transform::IDENTITY),
        );
        world.flush();
        let attached = world.entity(replica);
        assert_eq!(attached.get::<EngineRpm>(), Some(&snapshot.0));
        assert_eq!(attached.get::<TankTransmission>(), Some(&snapshot.1));
        assert_eq!(attached.get::<WeaponGate>(), Some(&snapshot.2));
        assert_eq!(attached.get::<TrackGripElements>(), Some(&snapshot.3));
        assert!(
            attached.contains::<RemoteServos>() && !attached.contains::<TankServos>(),
            "a replica integrates RemoteServos only"
        );
    }

    /// Replica tanks retain every queried collider layer while only authority bodies receive soft
    /// runs. Clients render the server stream and solve no contact with a loose piece.
    #[test]
    fn replica_tanks_keep_queried_surfaces_without_soft_runs() {
        // Fixture: the selected tank supplies construction data, not a collider-count oracle.
        let mut world = World::new();
        let (authority, replica) = authority_and_replica(&mut world);

        let collider_counts = |world: &mut World, root| {
            let mut counts = [0; 3];
            for (mut entity, layers) in world.query::<(Entity, &CollisionLayers)>().iter(world) {
                while let Some(parent) = world.get::<ChildOf>(entity) {
                    entity = parent.parent();
                    if entity == root {
                        for (count, layer) in counts.iter_mut().zip([
                            Layer::Vehicle,
                            Layer::TrackSide,
                            Layer::TrackBelt,
                        ]) {
                            *count += usize::from(layers.memberships.has_all(layer));
                        }
                        break;
                    }
                }
            }
            counts
        };
        let authority_counts = collider_counts(&mut world, authority);
        let replica_counts = collider_counts(&mut world, replica);
        assert!(authority_counts[2] > 0);
        assert_eq!(
            replica_counts[2], 0,
            "replicas have no consumer for a soft run"
        );
        assert_eq!(replica_counts[..2], authority_counts[..2]);
    }

    /// The game's damage model: a tank's armour volumes and crew boxes are no colliders, and a
    /// replica's aim and sight rays still meet its armour, from its set posed live (a replica keeps
    /// no pose history). A part given a collider again costs every tree, broad phase and pose sync
    /// a body per primitive for nothing; a reader still taking a tank's armour from avian's tree
    /// misses it. The player's aim dots and gun lay on a net client rely on it. Fixture: the shipped
    /// tank as authority and replica, overlapping at the origin; the authority aims, so its own
    /// armour is excluded; avian's tree is empty.
    #[test]
    fn a_replica_aims_at_a_tank_from_its_armour_set_alone() {
        use crate::ballistics::armour_set::{ArmourPart, live_box};
        let mut world = World::new();
        let (authority, replica) = authority_and_replica(&mut world);
        let parts = world
            .query_filtered::<(), With<ArmourPart>>()
            .iter(&world)
            .count();
        let colliders = world
            .query_filtered::<(), (With<ArmourPart>, With<Collider>)>()
            .iter(&world)
            .count();
        assert!(parts > 0, "the tanks carry armour parts");
        assert_eq!(colliders, 0, "no armour part is a collider");
        let (min, max) = live_box(&world, replica).expect("the replica carries its armour set");
        let centre = (min + max) * 0.5;
        // Fixture standoff beyond the set's box, m.
        let reach = (max - min).length() + 10.0;
        let rays = [Dir3::NEG_X, Dir3::Z, Dir3::NEG_Y]
            .map(|direction| Ray3d::new(centre - direction * reach, direction));
        let aims = aimed(&mut world, &rays, 2.0 * reach, authority);
        assert!(
            aims.iter().all(|&distance| distance < reach),
            "every ray stops on the replica's armour: {aims:?}"
        );
    }

    /// Each ray's [`crate::aim::aim_distance`] out to `max`, aimed by `own`, with avian's tree
    /// empty: tanks answer from their sets alone.
    fn aimed(world: &mut World, rays: &[Ray3d], max: f32, own: Entity) -> Vec<f32> {
        use crate::ballistics::armour_set::LiveArmour;
        use avian3d::prelude::SpatialQuery;
        use bevy::ecs::system::RunSystemOnce;
        world.init_resource::<avian3d::collider_tree::ColliderTrees>();
        let rays = rays.to_vec();
        world
            .run_system_once(move |spatial: SpatialQuery, armour: LiveArmour| {
                rays.iter()
                    .map(|&ray| crate::aim::aim_distance(&spatial, &armour, ray, max, own))
                    .collect::<Vec<f32>>()
            })
            .expect("the aim probe runs")
    }

    /// A lookup of the entity under `tank` that model node `name` names; each name names one.
    fn named(world: &World, tank: Entity) -> impl Fn(&str) -> Entity {
        let mut by_name: HashMap<String, Vec<Entity>> = HashMap::new();
        let mut walk = vec![tank];
        while let Some(entity) = walk.pop() {
            for &child in world.get::<Children>(entity).into_iter().flatten() {
                if let Some(name) = world.get::<Name>(child) {
                    by_name
                        .entry(name.as_str().to_owned())
                        .or_default()
                        .push(child);
                }
                walk.push(child);
            }
        }
        move |name| {
            let [entity] = by_name[name][..] else {
                panic!("one entity under the tank is named `{name}`");
            };
            entity
        }
    }

    /// The game's damage model: a spawned tank's set holds one crew box part per witness primitive
    /// of its geometry, each naming its witness volume's node, and one frame per node that moves
    /// its armour: the root, each servo, each recoiling barrel and the turret assembly a launch
    /// detaches. A witness primitive left out leaves its crew HP pool taking no dose; a frame left
    /// out holds its armour at rest while its node moves. Every spawned tank relies on it; the
    /// march's fixtures build their sets another way (`armour_set::fixture`). Fixture: the shipped
    /// tank as authority.
    #[test]
    fn a_spawned_tanks_set_holds_its_crew_boxes_and_moving_frames() {
        use std::collections::BTreeSet;
        let mut world = World::new();
        let (tank, _) = authority_and_replica(&mut world);
        let (blueprint, _) = shipped_construction();
        let (geometry, spec) = (&blueprint.geometry, &blueprint.spec);
        let node = named(&world, tank);
        let set = world
            .get::<ArmourSet>(tank)
            .expect("a spawned tank carries its armour set");

        let mut crew: Vec<Entity> = set
            .roles()
            .filter_map(|role| match role {
                Role::CrewBox(volume) => Some(volume),
                Role::Armour(_) => None,
            })
            .collect();
        let mut witnesses: Vec<Entity> = geometry
            .nodes
            .iter()
            .flat_map(|authored| {
                authored
                    .primitives
                    .iter()
                    .filter(|primitive| {
                        primitive
                            .substance
                            .as_ref()
                            .is_some_and(|substance| substance.matter == Matter::Witness)
                    })
                    .map(|_| node(&authored.name))
            })
            .collect();
        crew.sort_unstable();
        witnesses.sort_unstable();
        assert!(!witnesses.is_empty(), "the fixture tank has crew boxes");
        assert_eq!(
            crew, witnesses,
            "one crew box part per witness primitive, under its node"
        );
        assert!(
            crew.iter()
                .all(|&volume| world.get::<WitnessVolume>(volume).is_some()),
            "each crew box part names a witness volume"
        );

        let frames: BTreeSet<Entity> = set.frame_nodes().collect();
        let moving: BTreeSet<Entity> = std::iter::once(tank)
            .chain(spec.servos.keys().map(|name| node(name)))
            .chain(
                spec.weapons
                    .values()
                    .filter(|weapon| weapon.recoil.is_some())
                    .filter_map(|weapon| weapon.barrel.as_deref())
                    .map(&node),
            )
            .chain([node(&spec.turret.node)])
            .collect();
        assert_eq!(
            frames, moving,
            "a frame for exactly the root and each servo, recoiling barrel and turret assembly"
        );
    }

    /// `tank`'s hard armour primitives from the authored geometry, each shaped by its bake and
    /// posed at its node's world pose composed from the hierarchy: the root's `Position` and
    /// `Rotation`, then every local `Transform` down to the node. Independent of the armour set,
    /// so a set posing a part elsewhere disagrees with it. Each comes with its node's entity.
    fn hierarchy_armour(
        world: &mut World,
        tank: Entity,
        geometry: &TankGeometry,
    ) -> Vec<(Entity, Collider, Transform)> {
        use avian3d::prelude::{Position, Rotation};
        const BODY: &str = "the tank root is a body with a pose";
        const LOCAL: &str = "every node carries its local pose";
        let root = Transform::from_translation(world.get::<Position>(tank).expect(BODY).0)
            .with_rotation(world.get::<Rotation>(tank).expect(BODY).0);
        let named = named(world, tank);
        let mut armour = Vec::new();
        for &index in &geometry.ballistic_volumes {
            let node = &geometry.nodes[index];
            let entity = named(&node.name);
            let mut pose = *world.get::<Transform>(entity).expect(LOCAL);
            let mut above = entity;
            loop {
                above = world.get::<ChildOf>(above).expect(LOCAL).parent();
                if above == tank {
                    break;
                }
                pose = world
                    .get::<Transform>(above)
                    .expect(LOCAL)
                    .mul_transform(pose);
            }
            for primitive in &node.primitives {
                if primitive
                    .substance
                    .as_ref()
                    .is_some_and(|substance| !matches!(substance.matter, Matter::Witness))
                {
                    armour.push((
                        entity,
                        primitive.trimesh_collider(),
                        root.mul_transform(pose),
                    ));
                }
            }
        }
        armour
    }

    /// The nearest of `armour` (posed as [`hierarchy_armour`] poses it) `ray` meets within `max`.
    fn nearest(
        armour: &[(Entity, Collider, Transform)],
        ray: Ray3d,
        max: f32,
    ) -> Option<(f32, Entity)> {
        armour
            .iter()
            .filter_map(|(entity, collider, at)| {
                collider
                    .cast_ray(
                        at.translation,
                        at.rotation,
                        ray.origin,
                        *ray.direction,
                        max,
                        true,
                    )
                    .map(|(distance, _)| (distance, *entity))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The game's damage model: an aim ray passes its own tank and meets another. Every aim ray
    /// starts inside or behind its own armour, so dropping the exclusion welds the player's aim
    /// dots to their own tank, and inverting it makes every other tank transparent to them.
    /// Fixture: the shipped tank as replica at the origin, aiming, and as authority two box
    /// diagonals off along +X, yawed a quarter turn; avian's tree is empty.
    #[test]
    fn an_aim_ray_passes_its_own_tank_and_meets_another() {
        use crate::ballistics::armour_set::live_box;
        use avian3d::prelude::{Position, Rotation};
        let mut world = World::new();
        let (authority, replica) = authority_and_replica(&mut world);
        let (min, max) = live_box(&world, replica).expect("the replica carries its armour set");
        let centre = (min + max) * 0.5;
        let span = (max - min).length();
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        // The authority's box centred two diagonals along +X: the two boxes cannot overlap.
        let beside = centre + Vec3::X * 2.0 * span;
        world
            .entity_mut(authority)
            .insert((Position(beside - turn * centre), Rotation(turn)));
        let (other_min, other_max) =
            live_box(&world, authority).expect("the authority carries its armour set");
        let reach = 4.0 * span;
        let [away, toward] = [Dir3::NEG_X, Dir3::X].map(|direction| Ray3d::new(centre, direction));
        let aims = aimed(&mut world, &[away, toward], reach, replica);
        assert_eq!(
            aims[0], reach,
            "a ray from inside its own tank meets nothing of it"
        );
        let at = toward.get_point(aims[1]);
        // Resolution: a surface point's f32 rounding against the box it bounds, m.
        let slack = Vec3::splat(1.0e-3);
        assert!(
            aims[1] < reach
                && at.cmpge(other_min - slack).all()
                && at.cmple(other_max + slack).all(),
            "a ray from inside its own tank meets the other tank: met {at} at {}",
            aims[1]
        );
    }

    /// The game's damage model: a replica's aim rays meet its turret, turned, and its gun, pitched,
    /// where the hierarchy poses them. A replica's set keeps no pose history, so each frame is
    /// posed from its node's present local `Transform`; a set posing frames at rest, or ignoring
    /// that local pose, puts the turret where it is not. The player's aim dots on a net client
    /// rely on it. Fixture: the shipped tank as replica, aimed at by the authority (excluded) from
    /// the front with a 3×3 grid across its turret's box; the yaw servo turned and the gun pitched
    /// by fixture angles; avian's tree is empty.
    #[test]
    fn a_replica_aims_at_its_turned_turret_where_the_hierarchy_poses_it() {
        use avian3d::prelude::SimpleCollider;
        let mut world = World::new();
        let (authority, replica) = authority_and_replica(&mut world);
        let (blueprint, _) = shipped_construction();
        let rig = world
            .get::<Rig>(replica)
            .expect("the replica carries its rig");
        let (yaw, gun) = (rig.gunner_yaw, rig.gun);
        // Whether `node` rides the yaw servo.
        let turret = |world: &World, node: Entity| {
            std::iter::successors(Some(node), |&node| {
                world.get::<ChildOf>(node).map(ChildOf::parent)
            })
            .any(|node| node == yaw)
        };
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        for (entity, collider, at) in hierarchy_armour(&mut world, replica, &blueprint.geometry) {
            if turret(&world, entity) {
                let bounds = collider.aabb(at.translation, at.rotation, 0.0);
                lo = lo.min(bounds.min);
                hi = hi.max(bounds.max);
            }
        }
        assert!(lo.x < hi.x, "the replica's yaw servo carries armour");
        // Fixture standoff in front of the turret's box (the front is -Z), m.
        let standoff = (hi - lo).length() + 5.0;
        let reach = 3.0 * standoff;
        let rays: Vec<Ray3d> = (0..9)
            .map(|cell| {
                let [u, v] = [cell % 3, cell / 3].map(|step| 0.25 + 0.25 * step as f32);
                let origin = Vec3::new(
                    lo.x + (hi.x - lo.x) * u,
                    lo.y + (hi.y - lo.y) * v,
                    lo.z - standoff,
                );
                Ray3d::new(origin, Dir3::Z)
            })
            .collect();
        // Resolution: f32 rounding of one rigid chain composed in two orders (the set's frame
        // offsets, the hierarchy's links), m.
        let close = |a: f32, b: f32| (a - b).abs() < 1.0e-4;
        let agree = |set: f32, hierarchy: Option<(f32, Entity)>| {
            close(set, hierarchy.map_or(reach, |(distance, _)| distance))
        };
        let read = |world: &mut World| {
            let set = aimed(world, &rays, reach, authority);
            let armour = hierarchy_armour(world, replica, &blueprint.geometry);
            let hierarchy: Vec<_> = rays
                .iter()
                .map(|&ray| nearest(&armour, ray, reach))
                .collect();
            for (index, (&set, &hierarchy)) in set.iter().zip(&hierarchy).enumerate() {
                assert!(
                    agree(set, hierarchy),
                    "ray {index} meets the set at {set}, the hierarchy's armour at {hierarchy:?}"
                );
            }
            (set, hierarchy)
        };
        let (rest, at_rest) = read(&mut world);
        assert!(
            at_rest
                .iter()
                .flatten()
                .any(|&(_, part)| turret(&world, part)),
            "a fixture ray meets the turret at rest: {at_rest:?}"
        );
        // Fixture angles, rad: the turret a radian round, the gun pitched a tenth.
        world.get_mut::<Transform>(yaw).unwrap().rotate_local_y(1.0);
        world
            .get_mut::<Transform>(gun)
            .unwrap()
            .rotate_local_x(-0.1);
        let (turned, _) = read(&mut world);
        assert!(
            at_rest
                .iter()
                .zip(rest.iter().zip(&turned))
                .any(|(hit, (&rest, &turned))| {
                    hit.is_some_and(|(_, part)| turret(&world, part)) && !close(turned, rest)
                }),
            "a ray that met the turret at rest meets it elsewhere once turned: {rest:?} → {turned:?}"
        );
    }
}
