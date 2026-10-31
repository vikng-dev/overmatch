use std::collections::{HashMap, HashSet};

use avian3d::prelude::{
    AngularInertia, CenterOfMass, CoefficientCombine, CollisionLayers, Friction, LayerMask, Mass,
    NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass,
};
use bevy::asset::LoadState;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::integrity::authored_attachment;
use super::model::{
    EngineSound, Gun, GunBarrel, Hull, Muzzle, Rig, Roadwheel, Tank, TankRoot, TankServos, TankSim,
    TankViews, TrackSide, Turret, TurretBody, ViewConfig, Weapon, WeaponGate, WeaponGateState,
    WeaponIndex, WeaponLoad, WeaponLoads, WeaponState,
};
use super::servo::{RemoteServos, ServoCommand, ServoIndex, ServoRest, ServoRole, ServoSpec};
use super::view::{SimParts, bind_tank_view};
use crate::Layer;
use crate::bake::{TankBlueprint, TankEnvelopes, TankGeometry};
use crate::ballistics::{
    ArmorVolume, BallisticSurfaces, BallisticVolume, ComponentHealth, ComponentVolume,
    ExternalArmour, VolumeMaterial, WitnessSurface, WitnessVolume,
};
use crate::damage::{Ammo, Crewman, TankCapabilities, VolumeOf};
use crate::firecontrol::RangeTable;
use crate::shooting::RecoilParams;
use crate::spec::{TankSpec, TankSpecHandle, Trigger, ViewKind, VolumeSpec, WeaponSpec};
use crate::substances::Matter;
use crate::track::sim::{DriveSignal, EngineRpm, TankTransmission, TrackGripElements};

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
    }

    /// Clone handles for a root; the spec handle remains available to presentation validation.
    pub(crate) fn presentation(&self) -> TankPresentation {
        TankPresentation::new(self.scene.clone(), self.spec.clone())
    }
}

/// Presentation-only root handles, deliberately separate from [`TankContent`].
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

/// Synchronous construction data. This source never reads Bevy asset readiness.
#[derive(SystemParam)]
pub(crate) struct TankSimSource<'w> {
    blueprint: Option<Res<'w, TankBlueprint>>,
}

impl TankSimSource<'_> {
    pub(crate) fn get(&self) -> Option<TankContent<'_>> {
        let blueprint = self.blueprint.as_deref()?;
        Some(TankContent {
            geometry: &blueprint.geometry,
            spec: &blueprint.spec,
            envelopes: blueprint.envelopes(),
        })
    }
}

/// Opaque, asset-independent input to complete tank construction.
#[derive(Clone, Copy)]
pub(crate) struct TankContent<'a> {
    geometry: &'a TankGeometry,
    spec: &'a TankSpec,
    envelopes: TankEnvelopes,
}

impl<'a> TankContent<'a> {
    pub(super) fn geometry(self) -> &'a TankGeometry {
        self.geometry
    }

    pub(crate) fn spec(self) -> &'a TankSpec {
        self.spec
    }
}

/// The windowed compositions' load: the spec sheet and the view artifact's scene.
pub(crate) fn load_tank_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(PendingTankAssets {
        spec: asset_server.load(TANK_SPEC_PATH),
        scene: Some(asset_server.load(GltfAssetLabel::Scene(0).from_asset(TANK_GLB_PATH))),
    });
}

/// The dedicated server's load: the spec sheet, and nothing else. The sim artifact reaches the
/// server through `bake`'s startup extraction, not through the asset server.
pub(crate) fn load_tank_sim_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(PendingTankAssets {
        spec: asset_server.load(TANK_SPEC_PATH),
        scene: None,
    });
}

fn tank_transmission(spec: &TankSpec) -> TankTransmission {
    // Only `engine.idle_rpm` reaches the replicated `TankTransmission` state (the crank seed); the
    // geometry-coupled params (gears, kappa, sprocket radius) live in `TrackGear.trans`, built from
    // the measured `RigGeom` in `track::sim::init_track_gear`. So nominal geometry here is
    // deterministic — it never enters replicated state, and construction was already validated at
    // asset load.
    let params = spec
        .track
        .transmission_params(1.0, 1.0)
        .expect("TankSpec transmission was validated before tank construction");
    // `None` ⇔ the spec EXPLICITLY declares `architecture: Governor` (a missing block is a
    // load-time validation error, so this branch is never a silent fallback).
    params
        .as_ref()
        .map_or_else(TankTransmission::for_governor, TankTransmission::from_spec)
}

/// Complete fire-gate state in the same name-sorted order used to assign every [`WeaponIndex`].
fn weapon_gate(spec: &TankSpec) -> WeaponGate {
    let mut weapons: Vec<_> = spec.weapons.iter().collect();
    weapons.sort_by_key(|(name, _)| name.as_str());
    WeaponGate {
        weapons: weapons
            .into_iter()
            .map(|(name, weapon)| {
                let cycle = weapon.fire_cycle().unwrap_or_else(|err| {
                    panic!("weapon `{name}`: {err} — the spec's validate() joins every mount with its gun")
                });
                WeaponGateState::for_cycle(&cycle)
            })
            .collect(),
    }
}

/// Every weapon slot on its default round, loaded and selected, in [`weapon_gate`]'s slot order.
fn weapon_loads(spec: &TankSpec) -> WeaponLoads {
    WeaponLoads {
        weapons: vec![WeaponLoad::default(); spec.weapons.len()],
    }
}

fn tank_servos(spec: &TankSpec) -> TankServos {
    TankServos::for_count(spec.servos.len())
}

/// Spawn a root and its complete local simulation body in one command batch.
pub(crate) fn spawn_complete_tank<B: Bundle>(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    root_bundle: B,
) -> Entity {
    let transmission = tank_transmission(content.spec());
    // The element-grip slabs ride the SAME insertion that adds `Tank`: pre-sized synchronously
    // from the spec's link count (the REV-14 fixed-size invariant — `track::sim::TrackGripElements`),
    // never an empty vector awaiting a first-tick resize.
    let mut root = commands.spawn((
        presentation.root_bundle(),
        TrackGripElements::for_links(content.spec().track.link_count),
        // Complete REV-14 transmission state, synchronously constructed from spec data before the
        // root can replicate or simulate, and the public crank speed read off it.
        transmission,
        EngineRpm::of(&transmission),
        // The shaped drive signal starts at rest, as the command does.
        DriveSignal::default(),
        // Complete REV-17 weapon gate, synchronously constructed from the same sorted spec data as
        // its weapon slots. Replicated client attachment must preserve the arriving authority value.
        weapon_gate(content.spec()),
        // The round each slot has loaded and selected, beside the gate it is read with.
        weapon_loads(content.spec()),
        // Complete servo integrator inventory is data-built in the same spawn flush. The glb is a
        // view and never initializes replicated state.
        tank_servos(content.spec()),
        root_bundle,
    ));
    if let Some(scene) = presentation.scene_root() {
        root.insert(scene);
    }
    root.observe(bind_tank_view);
    let entity = root.id();
    assemble_tank_body(commands, entity, content);
    entity
}

/// Spawn the production simulation body without presentation assets.
///
/// Two instruments are deliberately headless and share this: the bitprobe differential harness and
/// the §13.6 ray fuzzer (`ballistics::fuzz`). Both need the same eagerly baked geometry,
/// spec-derived components, colliders, and rollback-visible state as an authority tank, and neither
/// wants a `WorldAssetRoot`, an asset handle, or a view-binding observer. The shared
/// [`assemble_tank_body`] remains the one simulation constructor.
pub(crate) fn spawn_headless_tank<B: Bundle>(
    commands: &mut Commands,
    content: TankContent,
    root_bundle: B,
) -> Entity {
    let transmission = tank_transmission(content.spec());
    let root = commands
        .spawn((
            Tank,
            TrackGripElements::for_links(content.spec().track.link_count),
            transmission,
            EngineRpm::of(&transmission),
            DriveSignal::default(),
            weapon_gate(content.spec()),
            weapon_loads(content.spec()),
            tank_servos(content.spec()),
            root_bundle,
        ))
        .id();
    assemble_tank_body(commands, root, content);
    root
}

/// Transitional ADR-0014 exception for `net::rig`: attach to a replicated root with a valid pose
/// and its authoritative [`EngineRpm`] (and, on the owner's own tank, [`TankTransmission`])
/// already present. Normal spawn paths must use [`spawn_complete_tank`].
pub(crate) fn attach_replicated_tank_body<B: Bundle>(
    commands: &mut Commands,
    root: Entity,
    content: TankContent,
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
            // Replicated authority state (EngineRpm, servo snapshots; the owner's TankTransmission,
            // DriveSignal, WeaponGate, WeaponLoads) arrived in the replication init snapshot. Do not
            // overwrite it with fresh spec-derived values.
            root_bundle,
        ))
        .observe(bind_tank_view);
    // Interpolated replicas chase the replicated public angles without manufacturing the
    // owner-private component.
    root_commands.insert(RemoteServos::for_count(content.spec().servos.len()));
    assemble_tank_body(commands, root, content);
}

/// Every spec-declared node resolved to a geometry index, in the deterministic sorted orders the
/// wire indices are assigned from. Produced by [`resolve_rig_nodes`], which asserts the rig
/// contract before returning — so every field here is a node that exists and downstream assembly
/// needs no unwrapping. An optional `barrel` is genuinely optional (a weapon that does not
/// reciprocate), not an unresolved name.
struct RigNodes<'a> {
    servo_entries: Vec<(&'a String, &'a ServoSpec)>,
    servo_nodes: Vec<usize>,
    weapon_entries: Vec<(&'a String, &'a WeaponSpec)>,
    weapon_nodes: Vec<(usize, Option<usize>)>,
    /// Every ballistic volume the MATERIAL rule found, name-sorted, paired with its spec entry
    /// where the spec declares one (`None` = main armour, which is most of the tank).
    volume_nodes: Vec<(&'a str, Option<&'a VolumeSpec>, usize)>,
    /// The gunner view's node — the main mount's Pitch servo, anchor of the gun chain.
    gunner_pitch: usize,
    hull: usize,
    center_of_mass: usize,
    /// The Yaw servo above [`Self::gunner_pitch`] in the extracted topology.
    turret: usize,
    /// The single `Primary` weapon's muzzle — the rig's main bore.
    primary_muzzle: usize,
}

/// Resolve every spec-declared node name against the extracted geometry, fail-fast: a single
/// assertion names ALL missing nodes at once rather than panicking on the first. Index-bearing
/// collections are sorted here because wire-derived indices must never depend on `HashMap`
/// iteration order.
fn resolve_rig_nodes<'a>(geometry: &'a TankGeometry, spec: &'a TankSpec) -> RigNodes<'a> {
    let mut servo_entries: Vec<_> = spec.servos.iter().collect();
    servo_entries.sort_by_key(|(node, _)| node.as_str());
    let mut weapon_entries: Vec<_> = spec.weapons.iter().collect();
    weapon_entries.sort_by_key(|(name, _)| name.as_str());

    let mut missing: Vec<String> = Vec::new();
    let mut resolve = |name: &str| -> Option<usize> {
        let found = geometry.by_name.get(name).copied();
        if found.is_none() {
            missing.push(name.to_string());
        }
        found
    };
    let servo_nodes: Vec<Option<usize>> = servo_entries
        .iter()
        .map(|(node, _)| resolve(node))
        .collect();
    let weapon_nodes: Vec<(Option<usize>, Option<usize>)> = weapon_entries
        .iter()
        .map(|(_, weapon)| {
            (
                resolve(&weapon.muzzle),
                weapon.barrel.as_deref().and_then(&mut resolve),
            )
        })
        .collect();
    // Volumes are the EXTRACTOR's list (membership is the sheet's declaration, §12), already
    // name-sorted — volumes have no wire index, but stable creation order remains part of
    // deterministic spawn. The spec contributes each volume's role and facets, joined by node name.
    let volume_nodes: Vec<_> = geometry
        .ballistic_volumes
        .iter()
        .map(|&index| {
            let name = geometry.nodes[index].name.as_str();
            (name, spec.volumes.get(name), index)
        })
        .collect();
    // The other direction: a declared component whose node is not a ballistic volume is a facet
    // hung on something the march will never meet — silently inert, so it fails here by name.
    let mut inert: Vec<&str> = spec
        .volumes
        .keys()
        .map(String::as_str)
        .filter(|name| !geometry.is_ballistic(name))
        .collect();
    inert.sort_unstable();
    assert!(
        inert.is_empty(),
        "spec declares volume entries on nodes that are not ballistic volumes (no primitive of \
         theirs wears a registry substance material, so the march can never reach them): \
         {inert:?}"
    );
    // The gunner view's node is the main mount's Pitch servo — the anchor of the gun chain.
    let gunner_pitch = spec
        .views
        .get(&ViewKind::Gunner)
        .and_then(|view| resolve(&view.node));
    let hull_index = resolve("Hull");
    let com_index = resolve("Center_Of_Mass");

    // The gunner's chain feeds the rig's `turret`/`gun` (optic, camera, launched-turret): the
    // declared Pitch node + the Yaw servo above it in the extracted topology
    // (`bake::turret_node`, which the envelope bake reads too) — the binder never guesses which of
    // several yaw/pitch mounts is the main one.
    let servo_nodes_with_role = |role: ServoRole| -> HashSet<usize> {
        servo_entries
            .iter()
            .zip(&servo_nodes)
            .filter(|((_, servo), _)| servo.role == role)
            .filter_map(|(_, index)| *index)
            .collect()
    };
    let pitch_indices = servo_nodes_with_role(ServoRole::Pitch);
    let turret_index = crate::bake::turret_node(geometry, spec);
    // The single `Primary` weapon supplies the rig's main bore (`Rig.muzzle`) — what the bore HUD
    // reads and LMB fires. Trigger never speaks to aiming; the chain handles come from the gunner
    // view above.
    let primary_muzzle_index = weapon_entries
        .iter()
        .zip(&weapon_nodes)
        .find(|((_, weapon), _)| weapon.trigger == Trigger::Primary)
        .and_then(|(_, (muzzle, _))| *muzzle);

    // Contract: declared nodes, the main-bore chain, a collider, and roadwheels on both tracks.
    if primary_muzzle_index.is_none() {
        missing.push("<a Primary weapon>".into());
    }
    // `spec.views` and `spec.servos` are independent maps: nothing in the spec format stops the
    // gunner view naming a node that is not a declared servo. Assembly assumes it IS one — only
    // servo nodes (plus hull/turret/weapon/volume/wheel nodes) are entered into `needed_nodes`, so a
    // non-servo gunner node would resolve, pass this contract, then fail far downstream in
    // `entity_at` with "needed nodes were spawned above". Checking the role here keeps the failure
    // where the contract is stated, and the message names the offending node.
    match gunner_pitch {
        None => missing.push("<a Pitch servo above the Primary weapon's muzzle>".into()),
        Some(index) if !pitch_indices.contains(&index) => missing.push(format!(
            "<the Gunner view's node {:?}, declared as a Pitch servo>",
            geometry.nodes[index].name
        )),
        Some(_) => {}
    }
    if turret_index.is_none() {
        missing.push("<a Yaw servo above the Primary weapon's muzzle>".into());
    }
    // The declared lists resolved at extraction (a missing name fails there, naming it), so these
    // only restate the structural minimum the assembler needs.
    if geometry.collision_proxies.is_empty() {
        missing.push("<a declared collision proxy>".into());
    }
    for side in [TrackSide::Left, TrackSide::Right] {
        if !geometry.roadwheels.iter().any(|&(_, s)| s == side) {
            missing.push(format!("<a declared {side:?} roadwheel>"));
        }
    }
    assert!(
        missing.is_empty(),
        "tank model is missing required rig nodes: {missing:?}"
    );

    let checked = |index: Option<usize>| index.expect("contract checked");
    RigNodes {
        servo_entries,
        servo_nodes: servo_nodes.into_iter().map(checked).collect(),
        weapon_entries,
        weapon_nodes: weapon_nodes
            .into_iter()
            .map(|(muzzle, barrel)| (checked(muzzle), barrel))
            .collect(),
        volume_nodes,
        gunner_pitch: checked(gunner_pitch),
        hull: checked(hull_index),
        center_of_mass: checked(com_index),
        turret: checked(turret_index),
        primary_muzzle: checked(primary_muzzle_index),
    }
}

/// Every node the simulation body spawns: the used nodes plus their ancestor chains. The COM node
/// is deliberately absent — its position is pure data, applied to the root, and nothing addresses
/// it as an entity anymore.
fn needed_nodes(geometry: &TankGeometry, nodes: &RigNodes) -> HashSet<usize> {
    let mut needed: HashSet<usize> = HashSet::new();
    let mut include = |mut index: usize| {
        while index != 0 && needed.insert(index) {
            index = geometry.nodes[index].parent.unwrap_or(0);
        }
    };
    for &index in &nodes.servo_nodes {
        include(index);
    }
    for &(muzzle, barrel) in &nodes.weapon_nodes {
        include(muzzle);
        if let Some(barrel) = barrel {
            include(barrel);
        }
    }
    for &(_, _, index) in &nodes.volume_nodes {
        include(index);
    }
    for &(index, _) in &geometry.roadwheels {
        include(index);
    }
    for &index in &geometry.collision_proxies {
        include(index);
    }
    include(nodes.hull);
    include(nodes.turret);
    needed
}

/// Spawn one entity per needed node, parented per the extracted topology. Extraction order is
/// parent-first, so a child always finds its parent already spawned. Returns the node-index →
/// entity table; `None` marks a node the body does not need.
fn spawn_node_entities(
    commands: &mut Commands,
    geometry: &TankGeometry,
    root: Entity,
    needed: &HashSet<usize>,
) -> Vec<Option<Entity>> {
    let mut entities: Vec<Option<Entity>> = vec![None; geometry.nodes.len()];
    for (index, node) in geometry.nodes.iter().enumerate().skip(1) {
        if !needed.contains(&index) {
            continue;
        }
        // Parent 0 is the loader's scene-wrapper node — identity transform while bevy_gltf's
        // coordinate conversion stays off (shadow-pinned), so folding it into the root is
        // pose-neutral.
        let parent = match node.parent {
            Some(0) | None => root,
            Some(p) => entities[p].expect("extraction order puts parents before children"),
        };
        let entity = commands
            .spawn((
                Name::new(node.name.clone()),
                node.transform,
                ChildOf(parent),
            ))
            .id();
        entities[index] = Some(entity);
    }
    entities
}

/// Give each servo node its slot, role, and rest rotation. Servo rest rotations are spawn data,
/// never first-tick captures.
fn insert_servos(
    commands: &mut Commands,
    geometry: &TankGeometry,
    root: Entity,
    nodes: &RigNodes,
    entity_at: impl Fn(usize) -> Entity,
) {
    for (slot, ((_, servo), &index)) in nodes
        .servo_entries
        .iter()
        .zip(&nodes.servo_nodes)
        .enumerate()
    {
        commands.entity(entity_at(index)).insert((
            (*servo).clone(),
            ServoCommand::default(),
            ServoIndex(slot),
            TankRoot(root),
            servo.role,
            ServoRest(geometry.nodes[index].transform.rotation),
        ));
    }
}

/// Bind each weapon's muzzle and optional recoiling barrel to one weapon slot. Recoil rest is
/// authored data.
fn insert_weapons(
    commands: &mut Commands,
    geometry: &TankGeometry,
    root: Entity,
    nodes: &RigNodes,
    entity_at: impl Fn(usize) -> Entity,
) {
    for (slot, ((weapon_name, weapon), &(muzzle_index, barrel_index))) in nodes
        .weapon_entries
        .iter()
        .zip(&nodes.weapon_nodes)
        .enumerate()
    {
        let muzzle = entity_at(muzzle_index);
        let barrel = barrel_index.map(&entity_at);
        let weapon_component = Weapon::of_spec(weapon_name, weapon, barrel).unwrap_or_else(|err| {
            panic!("weapon `{weapon_name}`: {err} — the spec's validate() resolves every round")
        });
        // The sight's range table is graduated for the default round.
        let default = &weapon_component.rounds[0].round;
        let range_table = RangeTable::for_weapon(default.muzzle_speed, default.drag_k);
        commands.entity(muzzle).insert((
            Muzzle,
            TankRoot(root),
            WeaponIndex(slot),
            weapon_component,
            range_table,
        ));
        if let (Some(barrel), Some(barrel_index)) = (barrel, barrel_index) {
            commands
                .entity(barrel)
                .insert((GunBarrel, WeaponIndex(slot), TankRoot(root)));
            if let Some(recoil) = weapon.recoil.as_ref() {
                commands.entity(barrel).insert(RecoilParams {
                    rest: geometry.nodes[barrel_index].transform.translation,
                    stiffness: recoil.stiffness,
                    damping: recoil.damping,
                    max_length: recoil.max_length.0,
                });
            }
        }
    }
}

/// Ballistic volumes: the volume bundle (design `armor-penetration-and-damage.md` §12; composition,
/// not a `kind` enum — the substance carries resistance, optional facets layer roles on top) + a
/// query-only trimesh collider per captured primitive, built once from the extracted buffers and
/// shared by every spawn ([`crate::bake::MeshGeometry::trimesh_collider`]: the exact parry
/// construction avian's `TrimeshFromMesh` performs, design §7.1), on the `Armor` layer with NO
/// collision response (`filters = NONE`) so it never perturbs the body — watertight solids may be
/// concave, fine for the march's raycast (ADR-0008).
///
/// The extracted buffers are node-LOCAL, and each collider is spawned as a child of its volume's
/// node entity with an IDENTITY local transform: avian's `ColliderTransform` composes the ancestor
/// `Transform` scales onto the shape, and a ballistic volume's chain is unit-scale by contract
/// (`bake::manifold_gate`), so the shape parry gets is the buffer the gate certified. `collect`
/// re-checks the live scale before it walks one.
///
/// # Where the factor lands, and why it is not always the node
///
/// A `BallisticVolume` is the unit the walk reads a factor from AND the entity damage is addressed
/// to (`ballistics::walk`'s `VolumeTable`, `resolve`'s deposits), so those two must be the same
/// entity. That works exactly while a node is ONE substance, which is every part of this tank but
/// the road wheels — §13.7 authors a mixed-substance part as one object holding one closed shell per
/// substance region, and the wheel is `MildSteel` bodies + `Rubber` rims in a single object.
///
/// So: a single-substance node carries `BallisticVolume` itself (unchanged). A mixed-substance node
/// hands the role down to its per-primitive collider entities — one glTF primitive is one material
/// by construction, so each is a volume of exactly one substance, and the walk (which already tracks
/// presence per primitive) sees two honest volumes instead of one averaged lie.
///
/// A witness node (ADR-0040) carries `WitnessVolume` instead of `BallisticVolume`, and its colliders
/// `WitnessSurface` on [`Layer::Witness`]: no walk reads it, and the witness pass finds it by that
/// marker. It is never split — L2.WITNESS_ROLE certifies a witness node one substance and crew.
///
/// The cost is that a mixed node cannot address damage: `hp` lives on the node, the deposits name
/// the shard. That is REFUSED here rather than silently dropped. Lifting it means keying the walk's
/// factor table per primitive instead of per volume — a §13 core change, deliberately not smuggled
/// into this slice.
fn insert_ballistic_volumes(
    commands: &mut Commands,
    geometry: &TankGeometry,
    root: Entity,
    nodes: &RigNodes,
    entity_at: impl Fn(usize) -> Entity,
) {
    for &(name, facets, index) in &nodes.volume_nodes {
        let node = &geometry.nodes[index];
        let entity = entity_at(index);
        let groups = node.substance_groups();
        // The extractor only lists a node here if a primitive of its wears a substance.
        assert!(
            !groups.is_empty(),
            "ballistic volume `{name}` captured no substance primitive"
        );
        let split = groups.len() > 1;
        let component = facets.and_then(VolumeSpec::component);
        let external = matches!(facets, Some(VolumeSpec::External));
        assert!(
            !(split && component.is_some()),
            "volume `{name}` is authored from {} substances AND declares component facets — the \
             facets sit on the node while each substance is its own volume, so its HP would never \
             be deposited to. Author it as one substance, or split the object",
            groups.len()
        );

        {
            let mut entity = commands.entity(entity);
            entity.insert(VolumeOf(root));
            if let Some(volume) = component {
                if let Some(crew) = volume.crew {
                    // Seat role + its native occupant (topology B): `home == seat` at spawn, so
                    // competence is 1.0 until a backfill swap moves an occupant to a foreign seat.
                    entity.insert((crew, Crewman { home: crew }));
                }
                if volume.ammo {
                    entity.insert(Ammo);
                }
                if let Some(function) = volume.function {
                    entity.insert(function);
                }
                // Damageable (module/crew/ammo): an HP pool the march depletes.
                entity.insert((ComponentVolume, ComponentHealth::full(volume.hp)));
            } else {
                // Pure armour: resists + shadows spall, nothing to lose.
                entity.insert(ArmorVolume);
                // On the entity that carries the volume: the node, or each substance's shard.
                if external && !split {
                    entity.insert(ExternalArmour);
                }
            }
            if !split {
                let (substance, primitives) = groups.iter().next().expect("exactly one group");
                let resolved = primitives[0]
                    .substance
                    .as_ref()
                    .expect("grouped by substance");
                match resolved.matter {
                    Matter::Hard {
                        factor,
                        debris_ratio,
                    } => {
                        entity.insert(BallisticVolume {
                            material: VolumeMaterial {
                                substance: resolved.id,
                                factor,
                                debris_ratio,
                            },
                            substance: (*substance).to_owned(),
                        });
                    }
                    Matter::Witness => {
                        entity.insert(WitnessVolume {
                            substance: (*substance).to_owned(),
                        });
                    }
                }
            }
        }
        // L2.WITNESS_ROLE certified a witness node single-substance and crew, so `split` is hard.
        let witness = !split
            && groups.values().next().is_some_and(|primitives| {
                primitives[0]
                    .substance
                    .as_ref()
                    .is_some_and(|resolved| resolved.matter == Matter::Witness)
            });

        for (substance, primitives) in &groups {
            for primitive in primitives {
                let triangles = primitive.indices.as_chunks::<3>().0.len();
                // Name the broken volume before Avian rejects an empty triangle list.
                assert!(
                    triangles > 0,
                    "ballistic volume `{name}` has an unindexed or triangle-less mesh primitive"
                );
                // The bake's manifold certificate, index-aligned with those triangles. Armour
                // without it is armour the walk cannot pair, so it is refused here rather than
                // collected ambiguously later.
                let certificate = primitive
                    .certificate
                    .as_ref()
                    .unwrap_or_else(|| panic!("ballistic volume `{name}` was never certified"));
                assert_eq!(
                    certificate.shells.len(),
                    triangles,
                    "ballistic volume `{name}` has {triangles} triangles and {} certified faces",
                    certificate.shells.len(),
                );
                let mut collider = commands.spawn((
                    ChildOf(entity),
                    // ADR-0015 shields this authored local pose from position sync.
                    authored_attachment(Transform::IDENTITY),
                    primitive.trimesh_collider(),
                    BallisticSurfaces {
                        shells: certificate.shells.clone(),
                        corners: certificate.corners.clone(),
                    },
                    CollisionLayers::new(
                        [if witness {
                            Layer::Witness
                        } else {
                            Layer::Armor
                        }],
                        LayerMask::NONE,
                    ),
                ));
                if witness {
                    collider.insert(WitnessSurface);
                }
                if split {
                    // The primitive IS the volume here (see the doc above): it carries the factor
                    // the walk reads and the ownership the impulse path resolves.
                    collider.insert((
                        BallisticVolume {
                            material: {
                                let resolved =
                                    primitive.substance.as_ref().expect("grouped by substance");
                                let Matter::Hard {
                                    factor,
                                    debris_ratio,
                                } = resolved.matter
                                else {
                                    unreachable!("L2.WITNESS_ROLE: a witness node is one substance")
                                };
                                VolumeMaterial {
                                    substance: resolved.id,
                                    factor,
                                    debris_ratio,
                                }
                            },
                            substance: (*substance).to_owned(),
                        },
                        VolumeOf(root),
                    ));
                    if external {
                        collider.insert(ExternalArmour);
                    }
                }
            }
        }
    }
}

/// Collision proxies: one convex hull per loose part of each captured primitive, on the Vehicle
/// layer ([`MeshGeometry::convex_hull_colliders`](crate::bake::MeshGeometry::convex_hull_colliders)).
/// Every part is a child collider of the proxy's node entity, so all parts of a proxy ride the
/// same body. Collision-only: contributes no mass (the root authors its own).
fn insert_collision_proxies(
    commands: &mut Commands,
    geometry: &TankGeometry,
    entity_at: impl Fn(usize) -> Entity,
) {
    for &index in &geometry.collision_proxies {
        let node = &geometry.nodes[index];
        assert!(
            !node.primitives.is_empty(),
            "collision proxy `{}` captured no mesh data",
            node.name
        );
        for primitive in &node.primitives {
            spawn_proxy_parts(commands, entity_at(index), &node.name, primitive);
        }
    }
}

/// One child collider under `parent` per loose part of `primitive`.
fn spawn_proxy_parts(
    commands: &mut Commands,
    parent: Entity,
    node_name: &str,
    primitive: &crate::bake::MeshGeometry,
) {
    let colliders = primitive
        .convex_hull_colliders()
        .unwrap_or_else(|| panic!("collision proxy `{node_name}` has a degenerate hull source"));
    for collider in colliders {
        commands.spawn((
            ChildOf(parent),
            // ADR-0015 shields this authored local pose from position sync.
            authored_attachment(Transform::IDENTITY),
            collider,
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            // Penetration backstops ONLY: the analytic belt model owns ALL tangential
            // ground physics (phase B). Avian's default friction on these hulls would
            // silently add grip/wall-climb beneath it.
            Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
        ));
    }
}

/// The root's own authored body: ADR-0011 mass, inertia extents, and center of mass (proxies add
/// no mass), the capability + view configuration, the per-slot sim state, and the rig handles.
#[expect(
    clippy::too_many_arguments,
    reason = "the root's authored body: each argument is a separate input of its construction"
)]
fn insert_root_components(
    commands: &mut Commands,
    root: Entity,
    spec: &TankSpec,
    geometry: &TankGeometry,
    entities: &[Option<Entity>],
    com_index: usize,
    weapon_count: usize,
    rig: Rig,
) {
    let (ex, ey, ez) = spec.inertia_extents;
    let parts: HashMap<String, Entity> = entities
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| entity.map(|e| (geometry.nodes[index].name.clone(), e)))
        .collect();
    commands.entity(root).insert((
        Mass(spec.mass),
        AngularInertia::from_shape(&Cuboid::new(ex, ey, ez), spec.mass),
        NoAutoMass,
        NoAutoAngularInertia,
        NoAutoCenterOfMass,
        CenterOfMass(geometry.nodes[com_index].root_position),
        // Per-tank capability requirements (design §7b) — drives `capability_effectiveness`.
        TankCapabilities(spec.capabilities.clone()),
        // Per-view FOV + gating requirement (camera FOV, view-death gate).
        TankViews(
            spec.views
                .iter()
                .map(|(kind, view)| {
                    (
                        *kind,
                        ViewConfig {
                            fov: view.optics.vertical_fov(),
                            requires: view.requires.clone(),
                        },
                    )
                })
                .collect(),
        ),
        // Root visibility owns the gunner-view hide: set to `Hidden`, `InheritedVisibility`
        // propagates `HIDDEN` to every descendant mesh, so the gunner optic (camera parked at
        // the gun pivot, inside the mantlet) sees no own-tank geometry — no near-plane clipping.
        Visibility::Inherited,
        // `TankSim` sized to the spawned rig: every local recoil/tracer slot exists from birth;
        // authoritative readiness and belt supply live in the root's `WeaponGate`, while servo
        // integration lives in the root's separately constructed `TankServos`/`RemoteServos`.
        // Weapon slots follow `weapon_entries`' sorted-by-name order — the same order
        // [`insert_weapons`] assigned `WeaponIndex` in, so slot i's state matches slot i's `Weapon`.
        TankSim {
            weapons: vec![WeaponState::default(); weapon_count],
        },
        rig,
        SimParts(parts),
    ));
    if let Some(sound) = engine_sound(spec) {
        commands.entity(root).insert(sound);
    }
}

/// The engine's recording and the crank facts its playback speed derives from, lifted out of the
/// declared transmission. `None` for a vehicle with no declared engine (`architecture: Governor`)
/// or one whose engine authors no `sound` — either way the tank runs silent.
fn engine_sound(spec: &TankSpec) -> Option<EngineSound> {
    let engine = spec
        .track
        .powertrain
        .transmission
        .as_ref()?
        .engine
        .as_ref()?;
    let sound = engine.sound.as_ref()?;
    Some(EngineSound {
        clip: sound.clip.clone(),
        clip_pop_hz: sound.clip_pop_hz,
        cylinders: engine.cylinders,
        idle_rpm: engine.idle_rpm,
        governed_rpm: engine.governed_rpm,
    })
}

/// Assemble only simulation-relevant geometry under `root`. Declared nodes resolve fail-fast, and
/// all index-bearing collections are sorted before entity creation. The GLB scene is not consulted.
fn assemble_tank_body(commands: &mut Commands, root: Entity, content: TankContent) {
    let geometry = content.geometry();
    let spec = content.spec();
    let nodes = resolve_rig_nodes(geometry, spec);

    let needed = needed_nodes(geometry, &nodes);
    let entities = spawn_node_entities(commands, geometry, root, &needed);
    let entity_at = |index: usize| entities[index].expect("needed nodes were spawned above");

    insert_servos(commands, geometry, root, &nodes, entity_at);
    insert_weapons(commands, geometry, root, &nodes, entity_at);
    insert_ballistic_volumes(commands, geometry, root, &nodes, entity_at);
    insert_collision_proxies(commands, geometry, entity_at);

    // --- Wheels: rig stations in name-sorted order (the track view reads their side/pose; the
    // belt force model uses the BAKED rest circles — articulation is view-only).
    //
    // The station entity is ALSO the wheel's ballistic volume: the wheel ships as one unified mesh,
    // so `Roadwheel` lands on an entity [`insert_ballistic_volumes`] already gave `BallisticVolume`
    // + `ArmorVolume` + a trimesh child. Nothing here conflicts — the two roles are disjoint
    // components — and it is deliberate: the armour follows the station's rest pose, and the
    // wheels being pure armour (no `hp`) means no damage path can despawn a station out from
    // under the track rig.
    //
    // These nodes are also scene ROOTS in the current export (was: children of `Track_*` under
    // `Hull`). The spawn loop maps `parent: Some(0) | None` to the tank root, so a top-level wheel
    // simply parents to the root instead of the hull entity — and since `Hull`/`Track_*` were
    // identity, every composed pose the sim reads (`root_position`, `rig_world_pose`, and the
    // wheel's own local `Transform` the track view sorts by) is numerically unchanged.
    for &(index, side) in &geometry.roadwheels {
        commands.entity(entity_at(index)).insert(Roadwheel { side });
    }

    // --- Structural markers.
    let hull = entity_at(nodes.hull);
    let gun = entity_at(nodes.gunner_pitch);
    let turret = entity_at(nodes.turret);
    let muzzle = entity_at(nodes.primary_muzzle);
    commands.entity(hull).insert(Hull);
    commands.entity(gun).insert(Gun);
    // Each body's envelope, from the kind's bake: the tank's on its root, the turret's on the
    // turret, live once a cook-off launches it free.
    let envelopes = content.envelopes;
    if let Some(envelope) = envelopes.tank {
        commands
            .entity(root)
            .insert(crate::ballistics::Envelope(envelope));
    }
    if let Some(envelope) = envelopes.turret {
        commands
            .entity(turret)
            .insert(crate::ballistics::Envelope(envelope));
    }
    let (tx, ty, tz) = spec.turret.inertia_extents;
    let friction = crate::substances::SubstanceRegistry::shipped_static()
        .hard(&spec.turret.substance)
        .map_err(|err| err.to_string())
        .and_then(|hard| hard.friction_of(&spec.turret.substance))
        .unwrap_or_else(|err| panic!("turret substance: {err}"));
    commands.entity(turret).insert((
        Turret,
        TurretBody {
            mass: spec.turret.mass,
            inertia_extents: Vec3::new(tx, ty, tz),
            friction,
        },
    ));

    insert_root_components(
        commands,
        root,
        spec,
        geometry,
        &entities,
        nodes.center_of_mass,
        nodes.weapon_entries.len(),
        Rig {
            hull,
            turret,
            gun,
            muzzle,
        },
    );

    // UNIT SCALE AT THE ROOT, BIT-EXACTLY (§13.6). `bake::manifold_gate` refuses an authored scale
    // other than 1 and `ballistics::collect` refuses a collider that reaches the world at one, but
    // the collector only judges a CANDIDATE: a scaled root moves and shrinks every collider's
    // world AABB, so the swept query can miss the armour entirely and leave nothing to refuse. The
    // root bundle is arbitrary caller data, so this is where that seam closes — before the first
    // corridor, by name, on the whole hierarchy at once.
    commands.queue(move |world: &mut World| {
        let scale = world.get::<Transform>(root).map_or(Vec3::ONE, |t| t.scale);
        assert_eq!(
            scale.to_array().map(f32::to_bits),
            [1.0f32.to_bits(); 3],
            "tank root {root} was spawned at scale {scale:?}, not 1 — armour is certified in the \
             buffer it was authored in and reaches the world through an unscaled hierarchy"
        );
    });
}

#[cfg(test)]
mod proxy_tests {
    use super::*;
    use crate::bake::MeshGeometry;

    fn tetra(offset: f32) -> MeshGeometry {
        MeshGeometry {
            positions: vec![
                [offset, 0.0, 0.0],
                [offset + 1.0, 0.0, 0.0],
                [offset, 1.0, 0.0],
                [offset, 0.0, 1.0],
            ],
            indices: vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
            substance: None,
            certificate: None,
            colliders: Default::default(),
        }
    }

    /// Invariant: every loose part of one proxy primitive is a child collider of the same node
    /// entity, so the parts move as one body. The tank body's collision shape relies on it.
    #[test]
    fn the_parts_of_one_proxy_share_one_parent() {
        let mut primitive = tetra(0.0);
        let far = tetra(5.0);
        primitive.positions.extend(far.positions);
        primitive.indices.extend(far.indices.iter().map(|i| i + 4));

        let mut world = World::new();
        let node = world.spawn_empty().id();
        let mut commands = world.commands();
        spawn_proxy_parts(&mut commands, node, "proxy", &primitive);
        world.flush();

        let parents: Vec<Entity> = world
            .query::<(&avian3d::prelude::Collider, &ChildOf)>()
            .iter(&world)
            .map(|(_, child_of)| child_of.parent())
            .collect();
        assert_eq!(parents, vec![node, node]);
    }

    /// EVERY SPAWNED TANK CARRIES ITS KIND'S ENVELOPES, HOLDING EVERY COLLIDER IT STANDS (the
    /// game's damage model: a round's crossing of a body runs until it has left the body's
    /// envelope, design §13.5, "The envelope"). The root carries the kind's tank envelope and the
    /// rig's turret node the kind's turret envelope, no other node one, and every collider under
    /// the root at its spawn pose lies inside the root's. A spawn that drops either insert, or puts
    /// one on another node, fails here. The shipped kind is fixture data; the assertions are the
    /// class invariant every kind's spawn keeps.
    #[test]
    fn a_spawned_tank_carries_its_kinds_envelopes() {
        let glb = crate::assets::asset_root().join(crate::tank::TANK_SIM_GLB_PATH);
        let spec = crate::track::rig_geom::tiger_spec();
        let geometry = crate::bake::extract_tank_geometry(
            &glb,
            &spec,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .expect("the shipped glb extracts");
        let blueprint =
            crate::bake::TankBlueprint::new(crate::tank::TANK_SPEC_PATH.to_owned(), geometry, spec);
        let envelopes = blueprint.envelopes();
        let content = TankContent {
            geometry: &blueprint.geometry,
            spec: &blueprint.spec,
            envelopes,
        };
        let mut world = World::new();
        let root = world.spawn(Transform::IDENTITY).id();
        assemble_tank_body(&mut world.commands(), root, content);
        world.flush();

        use bevy::math::bounding::Aabb3d;
        let turret = world
            .get::<Rig>(root)
            .expect("the root carries its rig")
            .turret;
        let carried: Vec<(Entity, Aabb3d)> = world
            .query::<(Entity, &crate::ballistics::Envelope)>()
            .iter(&world)
            .map(|(entity, envelope)| (entity, envelope.0))
            .collect();
        let expected: Vec<(Entity, Aabb3d)> = [(root, envelopes.tank), (turret, envelopes.turret)]
            .into_iter()
            .filter_map(|(entity, envelope)| Some((entity, envelope?)))
            .collect();
        assert!(
            envelopes.tank.is_some() && envelopes.turret.is_some(),
            "the fixture kind has captured geometry and a launchable turret, so both inserts run"
        );
        assert_eq!(carried.len(), expected.len(), "{carried:?}");
        for pair in &expected {
            assert!(carried.contains(pair), "{pair:?} not among {carried:?}");
        }
        let held = crate::ballistics::assert_envelope_holds(&world, root);
        assert!(held > 0, "the root holds the tank's colliders");
    }
}
