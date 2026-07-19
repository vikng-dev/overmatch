use std::collections::{HashMap, HashSet};

use avian3d::prelude::{
    AngularInertia, CenterOfMass, CoefficientCombine, Collider, CollisionLayers, Friction,
    LayerMask, Mass, NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, TrimeshFlags,
};
use bevy::asset::LoadState;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::integrity::authored_attachment;
use super::model::{
    Gun, GunBarrel, Hull, Muzzle, Rig, Roadwheel, Tank, TankRoot, TankSim, TankViews, TrackSide,
    Turret, ViewConfig, Weapon, WeaponIndex, WeaponState,
};
use super::servo::{ServoCommand, ServoIndex, ServoRest, ServoRole, ServoState};
use super::view::{SimParts, bind_tank_view};
use crate::Layer;
use crate::bake::{TankBlueprint, TankGeometry};
use crate::ballistics::{ArmorVolume, BallisticVolume, ComponentHealth, ComponentVolume};
use crate::damage::{Ammo, Crewman, TankCapabilities, VolumeOf};
use crate::firecontrol::RangeTable;
use crate::shooting::RecoilParams;
use crate::spec::{TankSpec, TankSpecHandle, Trigger, ViewKind};
use crate::track::sim::{TankTransmission, TrackGripElements};

/// Presentation handles. Loading may gate admission or view attachment, never simulation data.
#[derive(Resource, Clone)]
pub(crate) struct PendingTankAssets {
    pub spec: Handle<TankSpec>,
    pub scene: Handle<bevy::world_serialization::WorldAsset>,
}

impl PendingTankAssets {
    /// Both presentation assets have resolved.
    pub(crate) fn loaded(&self, asset_server: &AssetServer) -> bool {
        matches!(asset_server.load_state(&self.spec), LoadState::Loaded)
            && matches!(asset_server.load_state(&self.scene), LoadState::Loaded)
    }

    /// Clone handles for a root; the spec handle remains available to presentation validation.
    pub(crate) fn presentation(&self) -> TankPresentation {
        TankPresentation::new(self.scene.clone(), self.spec.clone())
    }
}

/// Presentation-only root handles, deliberately separate from [`TankContent`].
#[derive(Clone)]
pub(crate) struct TankPresentation {
    scene: Handle<bevy::world_serialization::WorldAsset>,
    spec: Handle<TankSpec>,
}

impl TankPresentation {
    pub(crate) fn new(
        scene: Handle<bevy::world_serialization::WorldAsset>,
        spec: Handle<TankSpec>,
    ) -> Self {
        Self { scene, spec }
    }

    pub(super) fn root_bundle(&self) -> impl Bundle {
        (
            WorldAssetRoot(self.scene.clone()),
            TankSpecHandle(self.spec.clone()),
            Tank,
        )
    }
}

/// Shared source path for the presentation loader and geometry extractor.
pub(crate) const TIGER_GLB_PATH: &str = "tiger_1/tiger_1.glb";

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
        })
    }
}

/// Opaque, asset-independent input to complete tank construction.
#[derive(Clone, Copy)]
pub(crate) struct TankContent<'a> {
    geometry: &'a TankGeometry,
    spec: &'a TankSpec,
}

impl<'a> TankContent<'a> {
    pub(super) fn geometry(self) -> &'a TankGeometry {
        self.geometry
    }

    pub(super) fn spec(self) -> &'a TankSpec {
        self.spec
    }
}

pub(crate) fn load_tank_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(PendingTankAssets {
        spec: asset_server.load("tiger_1/tiger_1.tank.ron"),
        scene: asset_server.load(GltfAssetLabel::Scene(0).from_asset(TIGER_GLB_PATH)),
    });
}

fn tank_transmission(spec: &TankSpec) -> TankTransmission {
    let params = spec
        .track
        .transmission_params()
        .expect("TankSpec transmission was validated before tank construction");
    params
        .as_ref()
        .map_or_else(TankTransmission::for_governor, TankTransmission::from_spec)
}

/// Spawn a root and its complete local simulation body in one command batch.
pub(crate) fn spawn_complete_tank<B: Bundle>(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    root_bundle: B,
) -> Entity {
    // The element-grip slabs ride the SAME insertion that adds `Tank`: pre-sized synchronously
    // from the spec's link count (the REV-14 fixed-size invariant — `track::sim::TrackGripElements`),
    // never an empty vector awaiting a first-tick resize.
    let mut root = commands.spawn((
        presentation.root_bundle(),
        TrackGripElements::for_links(content.spec().track.link_count),
        // Complete REV-14 transmission state, synchronously constructed from spec data before the
        // root can replicate or simulate.
        tank_transmission(content.spec()),
        root_bundle,
    ));
    root.observe(bind_tank_view);
    let entity = root.id();
    assemble_tank_body(commands, entity, content);
    entity
}

/// Transitional ADR-0014 exception for `net::rig`: attach to a replicated root with a valid pose
/// and its authoritative [`TankTransmission`] already present. Normal spawn paths must use
/// [`spawn_complete_tank`].
pub(crate) fn attach_replicated_tank_body<B: Bundle>(
    commands: &mut Commands,
    root: Entity,
    content: TankContent,
    presentation: TankPresentation,
    root_bundle: B,
) {
    commands
        .entity(root)
        .insert((
            presentation.root_bundle(),
            // Same spawn-sized element slabs as `spawn_complete_tank` — see the note there.
            TrackGripElements::for_links(content.spec().track.link_count),
            // TankTransmission arrived in the replication init snapshot. Do not overwrite a late
            // joiner's current authority state with a fresh spec-derived value here.
            root_bundle,
        ))
        .observe(bind_tank_view);
    assemble_tank_body(commands, root, content);
}

fn first_geometry_ancestor(
    geometry: &TankGeometry,
    mut index: usize,
    pred: impl Fn(usize) -> bool,
) -> Option<usize> {
    loop {
        if pred(index) {
            return Some(index);
        }
        index = geometry.nodes[index].parent?;
    }
}

/// Assemble only simulation-relevant geometry under `root`. Declared nodes resolve fail-fast, and
/// all index-bearing collections are sorted before entity creation. The GLB scene is not consulted.
fn assemble_tank_body(commands: &mut Commands, root: Entity, content: TankContent) {
    let geometry = content.geometry();
    let spec = content.spec();
    // Wire-derived indices must never depend on HashMap iteration order.
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
    // Volumes have no wire index, but stable creation order remains part of deterministic spawn.
    let mut volume_entries: Vec<_> = spec.volumes.iter().collect();
    volume_entries.sort_by_key(|(name, _)| name.as_str());
    let volume_nodes: Vec<_> = volume_entries
        .iter()
        .map(|(name, volume)| (*name, *volume, resolve(name)))
        .collect();
    // The gunner view's node is the main mount's Pitch servo — the anchor of the gun chain.
    let gunner_pitch = spec
        .views
        .get(&ViewKind::Gunner)
        .and_then(|view| resolve(&view.node));
    let hull_index = resolve("Hull");
    let com_index = resolve("Center_Of_Mass");

    // The extractor classifies these and returns roadwheels in their deterministic slot order.
    let wheel_nodes = &geometry.roadwheels;
    let collider_nodes = &geometry.collision_proxies;

    // The gunner's chain feeds the rig's `turret`/`gun` (optic, camera, launched-turret): the
    // declared Pitch node + the Yaw servo above it in the extracted topology — the binder never
    // guesses which of several yaw/pitch mounts is the main one.
    let yaw_indices: HashSet<usize> = servo_entries
        .iter()
        .zip(&servo_nodes)
        .filter(|((_, servo), _)| servo.role == ServoRole::Yaw)
        .filter_map(|(_, index)| *index)
        .collect();
    let turret_index = gunner_pitch
        .and_then(|pitch| first_geometry_ancestor(geometry, pitch, |i| yaw_indices.contains(&i)));
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
    if gunner_pitch.is_none() {
        missing.push("<a Pitch servo above the Primary weapon's muzzle>".into());
    }
    if turret_index.is_none() {
        missing.push("<a Yaw servo above the Primary weapon's muzzle>".into());
    }
    if collider_nodes.is_empty() {
        missing.push("*_Collider".into());
    }
    if !wheel_nodes.iter().any(|&(_, side)| side == TrackSide::Left) {
        missing.push("Wheel_L*".into());
    }
    if !wheel_nodes
        .iter()
        .any(|&(_, side)| side == TrackSide::Right)
    {
        missing.push("Wheel_R*".into());
    }
    assert!(
        missing.is_empty(),
        "tank model is missing required rig nodes: {missing:?}"
    );

    // Include every used node and its ancestor chain. Extraction order is parent-first.
    let mut needed: HashSet<usize> = HashSet::new();
    {
        let mut include = |mut index: usize| {
            while index != 0 && needed.insert(index) {
                index = geometry.nodes[index].parent.unwrap_or(0);
            }
        };
        for index in servo_nodes.iter().flatten() {
            include(*index);
        }
        for (muzzle, barrel) in &weapon_nodes {
            include(muzzle.expect("contract checked"));
            if let Some(barrel) = barrel {
                include(*barrel);
            }
        }
        for (_, _, index) in &volume_nodes {
            include(index.expect("contract checked"));
        }
        for &(index, _) in wheel_nodes {
            include(index);
        }
        for &index in collider_nodes {
            include(index);
        }
        include(hull_index.expect("contract checked"));
        include(turret_index.expect("contract checked"));
        // The COM node is deliberately NOT spawned: its position is pure data, applied to the
        // root below — nothing addresses it as an entity anymore.
    }
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
    let entity_at = |index: usize| entities[index].expect("needed nodes were spawned above");

    // Servo rest rotations are spawn data, never first-tick captures.
    for (slot, ((_, servo), index)) in servo_entries.iter().zip(&servo_nodes).enumerate() {
        let index = index.expect("contract checked");
        commands.entity(entity_at(index)).insert((
            (*servo).clone(),
            ServoCommand::default(),
            ServoIndex(slot),
            TankRoot(root),
            servo.role,
            ServoRest(geometry.nodes[index].transform.rotation),
        ));
    }

    // A muzzle and optional barrel share one weapon slot; recoil rest is authored data.
    for (slot, ((weapon_name, weapon), (muzzle_index, barrel_index))) in
        weapon_entries.iter().zip(&weapon_nodes).enumerate()
    {
        let muzzle = entity_at(muzzle_index.expect("contract checked"));
        let barrel = barrel_index.map(&entity_at);
        let weapon_component = Weapon {
            name: (*weapon_name).clone(),
            speed: weapon.speed,
            caliber: weapon.caliber,
            mass: weapon.mass,
            fire_mode: weapon.fire_mode,
            recoil: weapon.recoil.clone(),
            barrel,
            fire: weapon.fire.clone(),
            load: weapon.load.clone(),
            trigger: weapon.trigger,
        };
        let range_table = RangeTable::for_weapon(
            weapon_component.speed,
            weapon_component.caliber,
            weapon_component.mass,
        );
        commands.entity(muzzle).insert((
            Muzzle,
            TankRoot(root),
            WeaponIndex(slot),
            weapon_component,
            range_table,
        ));
        if let (Some(barrel), Some(barrel_index)) = (barrel, *barrel_index) {
            commands
                .entity(barrel)
                .insert((GunBarrel, WeaponIndex(slot), TankRoot(root)));
            if let Some(recoil) = weapon.recoil.as_ref() {
                commands.entity(barrel).insert(RecoilParams {
                    rest: geometry.nodes[barrel_index].transform.translation,
                    stiffness: recoil.stiffness,
                    damping: recoil.damping,
                });
            }
        }
    }

    // --- Ballistic volumes: the volume bundle (design `armor-penetration-and-damage.md` §12;
    // composition, not a `kind` enum — `material_factor` every volume has, optional facets layer
    // roles on top) + a query-only trimesh collider per captured primitive, built from the
    // extracted buffers. `trimesh_with_config(…, MERGE_DUPLICATE_VERTICES)` is the exact parry
    // construction avian's `TrimeshFromMesh` performs (design §7.1, vendored-source proven), on
    // the `Armor` layer with NO collision response (`filters = NONE`) so it never perturbs the
    // body — watertight solids may be concave, fine for the march's raycast (ADR-0008).
    for (name, volume, index) in &volume_nodes {
        let index = index.expect("contract checked");
        let node = &geometry.nodes[index];
        let entity = entity_at(index);
        assert!(
            volume.hp.is_some()
                || (volume.crew.is_none() && !volume.ammo && volume.function.is_none()),
            "tank volume `{name}` declares a consequence facet but has no hp"
        );
        // A declared volume without captured mesh data would be invisible to penetration queries.
        assert!(
            !node.primitives.is_empty(),
            "ballistic volume `{name}` captured no mesh data (does its node name follow the \
             `*_Ballistic` capture rule?)"
        );
        {
            let mut entity = commands.entity(entity);
            entity.insert((
                BallisticVolume {
                    material_factor: volume.material_factor,
                },
                VolumeOf(root),
            ));
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
            match volume.hp {
                // Damageable (module/crew/ammo): an HP pool the march depletes.
                Some(hp) => {
                    entity.insert((
                        ComponentVolume,
                        ComponentHealth {
                            current: hp,
                            max: hp,
                        },
                    ));
                }
                // Pure armour: resists + shadows spall, nothing to lose.
                None => {
                    entity.insert(ArmorVolume);
                }
            }
        }
        for primitive in &node.primitives {
            let vertices: Vec<Vec3> = primitive
                .positions
                .iter()
                .copied()
                .map(Vec3::from)
                .collect();
            let triangles: Vec<[u32; 3]> = primitive
                .indices
                .chunks_exact(3)
                .map(|t| [t[0], t[1], t[2]])
                .collect();
            // Name the broken volume before Avian rejects an empty triangle list.
            assert!(
                !triangles.is_empty(),
                "ballistic volume `{name}` has an unindexed or triangle-less mesh primitive"
            );
            commands.spawn((
                ChildOf(entity),
                // ADR-0015 shields this authored local pose from position sync.
                authored_attachment(Transform::IDENTITY),
                Collider::trimesh_with_config(
                    vertices,
                    triangles,
                    TrimeshFlags::MERGE_DUPLICATE_VERTICES,
                ),
                CollisionLayers::new([Layer::Armor], LayerMask::NONE),
            ));
        }
    }

    // --- Collision proxies: a convex hull per captured primitive on the Vehicle layer.
    // `Collider::convex_hull(points)` is exactly avian's `ConvexHullFromMesh` (it ignores
    // indices — design §7.1). Collision-only: contributes no mass (the root authors its own).
    for &index in collider_nodes {
        let node = &geometry.nodes[index];
        assert!(
            !node.primitives.is_empty(),
            "collision proxy `{}` captured no mesh data",
            node.name
        );
        for primitive in &node.primitives {
            let points: Vec<Vec3> = primitive
                .positions
                .iter()
                .copied()
                .map(Vec3::from)
                .collect();
            let collider = Collider::convex_hull(points).unwrap_or_else(|| {
                panic!(
                    "collision proxy `{}` has a degenerate hull source",
                    node.name
                )
            });
            commands.spawn((
                ChildOf(entity_at(index)),
                // ADR-0015 shields this authored local pose from position sync.
                authored_attachment(Transform::IDENTITY),
                collider,
                CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
                // Penetration backstops ONLY: the analytic belt model owns ALL tangential
                // ground physics (phase B). Avian's default friction on these hulls would
                // silently add grip/wall-climb beneath it (codex phase-B blocker 10).
                Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
            ));
        }
    }

    // --- Wheels: rig stations in name-sorted order (the track view reads their side/pose; the
    // belt force model uses the BAKED rest circles — articulation is view-only).
    for &(index, side) in wheel_nodes {
        commands.entity(entity_at(index)).insert(Roadwheel { side });
    }

    // --- Structural markers.
    let hull = entity_at(hull_index.expect("contract checked"));
    let gun = entity_at(gunner_pitch.expect("contract checked"));
    let turret = entity_at(turret_index.expect("contract checked"));
    let muzzle = entity_at(primary_muzzle_index.expect("contract checked"));
    commands.entity(hull).insert(Hull);
    commands.entity(gun).insert(Gun);
    commands.entity(turret).insert(Turret);

    // ADR-0011: mass, inertia extents, and center of mass are authored; proxies add no mass.
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
        CenterOfMass(geometry.nodes[com_index.expect("contract checked")].root_position),
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
                            fov: view.fov,
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
        // `TankSim` sized to the spawned rig: every slot exists from birth (reloads start 0.0 =
        // loaded, automatic belts start full; servo rests are spawned config, not captured state).
        // Weapon slots follow `weapon_entries`' sorted-by-name order — the same order the
        // `WeaponIndex` loop above assigned, so slot i's state matches slot i's `Weapon`.
        TankSim {
            servos: vec![ServoState::default(); spec.servos.len()],
            weapons: weapon_entries
                .iter()
                .map(|(_, weapon)| WeaponState::for_mode(&weapon.fire_mode))
                .collect(),
        },
        Rig {
            hull,
            turret,
            gun,
            muzzle,
        },
        SimParts(parts),
    ));
}
