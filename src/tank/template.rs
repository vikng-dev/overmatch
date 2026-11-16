//! A tank kind's construction template: what every instance of the kind has in common, prepared
//! once from the kind's data when `bake` admits it at startup. A spawn ([`super::spawn`]) then only
//! allocates the instance's entities and inserts these values with the instance's links
//! ([`TankTemplate::assemble`]); the pose, identity and owner its caller gives the root are all
//! that differ between instances. Built from data alone, never from a loaded asset (ADR-0014). A
//! kind whose template cannot be built is refused here, at load, naming its file.
//!
//! The template names the entities one instance creates by member id, their index in creation
//! order: [`ROOT`], then each node in [`TankTemplate::nodes`] order, then each armour volume or
//! crew box primitive in volume order. Nothing names a collision proxy's or a track's colliders,
//! which are created after them.

use std::collections::HashSet;
use std::sync::Arc;

use avian3d::prelude::{
    AngularInertia, CenterOfMass, Collider, CollisionLayers, Friction, LayerMask, Mass,
    NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass,
};
use bevy::prelude::*;

use super::integrity::authored_attachment;
use super::model::{
    EngineSound, Gun, GunBarrel, Hull, Muzzle, Rig, Roadwheel, TankRoot, TankServos, TankSim,
    TankViews, TrackSide, Turret, TurretBody, ViewConfig, Weapon, WeaponGate, WeaponGateState,
    WeaponIndex, WeaponLoad, WeaponLoads, WeaponState,
};
use super::servo::{ServoCommand, ServoIndex, ServoMechanism, ServoRest, ServoSpec};
use super::view::SimParts;
use crate::Layer;
use crate::bake::{MeshGeometry, TankBlueprint, TankGeometry};
use crate::ballistics::armour_set::{ArmourTemplate, Node, PartSource, Role};
use crate::ballistics::{
    ArmorVolume, BallisticSurfaces, BallisticVolume, ComponentHealth, ComponentVolume,
    ExternalArmour, VolumeMaterial, WitnessVolume,
};
use crate::damage::{Ammo, Crewman, TankCapabilities, VolumeOf};
use crate::firecontrol::RangeTable;
use crate::shooting::RecoilParams;
use crate::spec::{ComponentSpec, TankSpec, Trigger, ViewKind, VolumeSpec, WeaponSpec};
use crate::substances::Matter;
use crate::track::bump_stop::TrackColliders;
use crate::track::driver::DriverSense;
use crate::track::sim::{DriveSignal, EngineRpm, TankTransmission, TrackGripElements};
use crate::track::transmission::TransmissionParams;

/// The root's member id.
const ROOT: usize = 0;

/// One tank kind's construction, shared by every instance of it.
///
/// Required resource: `bake` inserts it with the kind's blueprint in one `Startup` command flush,
/// before any schedule a spawn system runs in, so a system reading it in a world without it is a
/// composition bug and fails its parameter validation.
#[derive(Resource)]
pub(crate) struct TankTemplate {
    spec: Arc<TankSpec>,
    /// The nominal transmission the sheet's admission parsed ([`TankBlueprint`]).
    transmission: Arc<TransmissionParams>,
    track_colliders: TrackColliders,
    authority: AuthorityState,
    /// The nodes the body spawns: the used nodes and their ancestor chains, in extraction
    /// (parent-first) order. The centre-of-mass node is absent: its position is data, applied to
    /// the root, and nothing addresses it as an entity.
    nodes: Vec<NodeEntry>,
    /// In the name-sorted order [`ServoIndex`] numbers them.
    servos: Vec<ServoEntry>,
    /// In the name-sorted order [`WeaponIndex`] numbers them.
    weapons: Vec<WeaponEntry>,
    /// In the extractor's name-sorted order.
    volumes: Vec<VolumeEntry>,
    armour: ArmourTemplate,
    proxies: Vec<ProxyEntry>,
    /// Each road wheel station and its side, in the bake's order.
    roadwheels: Vec<(usize, TrackSide)>,
    hull: usize,
    turret: usize,
    turret_body: TurretBody,
    gunner_yaw: usize,
    /// The gunner view's node: the main mount's Pitch servo, anchor of the gun chain.
    gun: usize,
    /// The single `Primary` weapon's muzzle: the rig's main bore.
    muzzle: usize,
    root: RootBody,
    /// DERIVED ([`Self::contact_radius`]).
    contact_radius: f32,
    /// DERIVED: the root-local points [`Self::stance`] encloses.
    stance_points: Vec<Vec3>,
}

/// The authority root's simulation state at spawn.
struct AuthorityState {
    /// Pre-sized at force resolution (`track::sim::TrackGripElements` owns the fixed-size
    /// invariant), never an empty vector awaiting a first-tick resize.
    grip: TrackGripElements,
    /// Only `engine.idle_rpm` reaches it (the crank seed); the geometry-coupled params (gears,
    /// kappa, sprocket radius) live in `TrackGear.trans`, built from the measured `RigGeom` in
    /// `track::sim::init_track_gear`.
    transmission: TankTransmission,
    /// The public crank speed read off `transmission`.
    rpm: EngineRpm,
    /// The Driver's ledger, empty: nothing has been felt before the first tick. It averages one
    /// shift cut, a gearbox datum the nominal params carry.
    sense: DriverSense,
    gate: WeaponGate,
    /// Every weapon slot on its default round, loaded and selected.
    loads: WeaponLoads,
    servos: TankServos,
}

struct NodeEntry {
    name: Name,
    /// Its local rest pose, as authored.
    transform: Transform,
    /// Its parent's member id; a child of the loader's scene wrapper hangs under the root.
    parent: usize,
}

struct ServoEntry {
    node: usize,
    spec: ServoSpec,
    /// Spawn data, never a first-tick capture.
    rest: ServoRest,
}

struct WeaponEntry {
    muzzle: usize,
    barrel: Option<BarrelEntry>,
    /// The weapon as every instance carries it but for [`Weapon::barrel`], the instance's barrel
    /// entity.
    weapon: Weapon,
    /// Graduated for the weapon's default round.
    range_table: RangeTable,
}

/// A weapon's recoiling barrel node, and its recoil where the weapon reciprocates; the rest is
/// authored data.
struct BarrelEntry {
    node: usize,
    recoil: Option<RecoilParams>,
}

/// The game's damage model: one ballistic volume ([`classify_volume`]).
struct VolumeEntry {
    node: usize,
    /// Its armour lies outside the main armour (`VolumeSpec::External`), marked on whatever carries
    /// the volume.
    external: bool,
    matter: VolumeMatter,
}

/// The game's damage model: what a volume's node and its primitives' parts carry.
enum VolumeMatter {
    /// One substance, on the node, over `parts` parts.
    Single {
        /// Its HP pool and facets; `None` for armour, which has nothing to lose.
        component: Option<ComponentSpec>,
        substance: NodeSubstance,
        parts: usize,
    },
    /// Several, armour only: each primitive's part, in creation order, a volume of its own
    /// substance.
    Split(Vec<BallisticVolume>),
}

enum NodeSubstance {
    Hard(BallisticVolume),
    /// A crew box's (ADR-0040): its parts are crew box parts of the set.
    Witness(WitnessVolume),
}

/// One collision proxy node's convex hulls, every primitive's in order, and its friction.
struct ProxyEntry {
    node: usize,
    colliders: Vec<Collider>,
    friction: Friction,
}

/// The root's authored body.
struct RootBody {
    mass: Mass,
    angular_inertia: AngularInertia,
    centre_of_mass: CenterOfMass,
    capabilities: TankCapabilities,
    views: TankViews,
    /// Every local recoil/tracer slot, in weapon slot order.
    sim: TankSim,
    /// `None` for an engine that authors no `sound`: the tank runs silent.
    engine_sound: Option<EngineSound>,
}

impl TankTemplate {
    /// Prepare the kind `blueprint` describes, its track shapes `track_colliders`. Every refusal
    /// names the blueprint's file.
    pub(crate) fn new(blueprint: &TankBlueprint, track_colliders: TrackColliders) -> Self {
        let (file, geometry, spec) = (&blueprint.file, &*blueprint.geometry, &*blueprint.spec);
        let rig = resolve_rig_nodes(file, geometry, spec);
        let needed = needed_nodes(geometry, &rig);
        // Geometry index to member id.
        let mut member: Vec<Option<usize>> = vec![None; geometry.nodes.len()];
        let mut nodes = Vec::new();
        for (index, node) in geometry.nodes.iter().enumerate().skip(1) {
            if !needed.contains(&index) {
                continue;
            }
            // Parent 0 is the loader's scene-wrapper node — identity transform while bevy_gltf's
            // coordinate conversion stays off (shadow-pinned), so folding it into the root is
            // pose-neutral.
            let parent = match node.parent {
                Some(0) | None => ROOT,
                Some(parent) => {
                    member[parent].expect("extraction order puts parents before children")
                }
            };
            nodes.push(NodeEntry {
                name: Name::new(node.name.clone()),
                transform: node.transform,
                parent,
            });
            member[index] = Some(nodes.len());
        }
        let at = |index: usize| member[index].expect("a needed node is a member");

        let servos = rig
            .servo_entries
            .iter()
            .zip(&rig.servo_nodes)
            .map(|((_, servo), &index)| ServoEntry {
                node: at(index),
                spec: (*servo).clone(),
                rest: ServoRest(geometry.nodes[index].transform.rotation),
            })
            .collect();
        let weapons = rig
            .weapon_entries
            .iter()
            .zip(&rig.weapon_nodes)
            .map(|(&(name, weapon), &(muzzle, barrel))| {
                let built = Weapon::of_spec(name, weapon, None).unwrap_or_else(|err| {
                    panic!(
                        "{file}: weapon `{name}`: {err} — the spec's validate() resolves every \
                         round"
                    )
                });
                let default = &built.rounds[0].round;
                WeaponEntry {
                    muzzle: at(muzzle),
                    barrel: barrel.map(|index| BarrelEntry {
                        node: at(index),
                        recoil: weapon.recoil.as_ref().map(|recoil| RecoilParams {
                            rest: geometry.nodes[index].transform.translation,
                            stiffness: recoil.stiffness,
                            damping: recoil.damping,
                            max_length: recoil.max_length.0,
                        }),
                    }),
                    range_table: RangeTable::for_weapon(default.muzzle_speed, default.drag_k),
                    weapon: built,
                }
            })
            .collect();

        // The parts, numbered after the nodes, each with its node's index in `nodes`.
        let mut parts = Vec::new();
        let volumes = rig
            .volume_nodes
            .iter()
            .map(|&(name, facets, index)| {
                let first = 1 + nodes.len() + parts.len();
                let node = &geometry.nodes[index];
                let (volume, shapes) = classify_volume(file, name, facets, node, at(index), first);
                for (id, (shape, surfaces, role)) in (first..).zip(shapes) {
                    parts.push((id, at(index) - 1, shape, surfaces, role));
                }
                volume
            })
            .collect();
        let armour = armour_template(&rig, &nodes, &at, &parts);
        let turret_body = TurretBody {
            mass: spec.turret.mass.0,
            angular_inertia: spec.turret.angular_inertia(),
            centre_of_mass: armour
                .mass_centre(at(rig.turret), &|node| nodes[node - 1].transform)
                .expect("the turret node is a frame of the tank's armour set"),
        };

        let proxies = geometry
            .collision_proxies
            .iter()
            .map(|&index| {
                let node = &geometry.nodes[index];
                assert!(
                    !node.primitives.is_empty(),
                    "{file}: collision proxy `{}` captured no mesh data",
                    node.name
                );
                ProxyEntry {
                    node: at(index),
                    colliders: node
                        .primitives
                        .iter()
                        .flat_map(|primitive| proxy_hulls(&node.name, primitive))
                        .collect(),
                    friction: proxy_friction(spec, &node.name),
                }
            })
            .collect();

        let params = &*blueprint.transmission;
        let transmission = TankTransmission::from_spec(params);
        let resolution = crate::track::forces::CONTACT_RESOLUTION;
        let authority = AuthorityState {
            grip: TrackGripElements::for_links(resolution.stations, resolution.lateral_points),
            transmission,
            rpm: EngineRpm::of(&transmission),
            sense: DriverSense::for_cut(params.shift_ticks),
            gate: WeaponGate {
                weapons: rig
                    .weapon_entries
                    .iter()
                    .map(|(_, weapon)| WeaponGateState::for_cycle(&weapon.fire_cycle))
                    .collect(),
            },
            loads: WeaponLoads {
                weapons: vec![WeaponLoad::default(); spec.weapons.len()],
            },
            servos: TankServos::for_count(spec.servos.len()),
        };
        let root = RootBody {
            mass: Mass(spec.mass),
            angular_inertia: spec.hull_angular_inertia(),
            centre_of_mass: CenterOfMass(geometry.nodes[rig.center_of_mass].root_position),
            // Per-tank capability requirements (design §7b) — read by `track::sim::drive_target`.
            capabilities: TankCapabilities {
                requirements: spec.capabilities.clone(),
                drive_controls: spec.drive_controls.clone(),
            },
            // Per-view FOV + gating requirement (camera FOV, view-death gate).
            views: TankViews {
                gunner: authored_view(spec, ViewKind::Gunner),
                commander: authored_view(spec, ViewKind::Commander),
            },
            sim: TankSim {
                weapons: vec![WeaponState::default(); rig.weapon_entries.len()],
            },
            engine_sound: engine_sound(spec),
        };
        let contact_radius = contact_radius(geometry, &track_colliders);
        let stance_points = stance_points(geometry, &track_colliders);
        Self {
            spec: Arc::clone(&blueprint.spec),
            transmission: Arc::clone(&blueprint.transmission),
            track_colliders,
            authority,
            nodes,
            servos,
            weapons,
            volumes,
            armour,
            proxies,
            roadwheels: geometry
                .roadwheels
                .iter()
                .map(|&(index, side)| (at(index), side))
                .collect(),
            hull: at(rig.hull),
            turret: at(rig.turret),
            turret_body,
            gunner_yaw: at(rig.gunner_yaw),
            gun: at(rig.gunner_pitch),
            muzzle: at(rig.primary_muzzle),
            root,
            contact_radius,
            stance_points,
        }
    }

    pub(crate) fn spec(&self) -> &TankSpec {
        &self.spec
    }

    pub(super) fn transmission(&self) -> &TransmissionParams {
        &self.transmission
    }

    pub(crate) fn track_colliders(&self) -> &TrackColliders {
        &self.track_colliders
    }

    /// DERIVED conservative body radius in metres, from declared collision proxies and prepared
    /// track shapes. Each ancestor contributes its translation and largest scale to a sphere;
    /// rotations preserve that sphere, including arbitrary servo angles.
    pub(crate) fn contact_radius(&self) -> f32 {
        self.contact_radius
    }

    /// DERIVED: how the tank stands over the ground it spawns on, posed by `rotation`: the stance
    /// enclosing its collision construction, the declared proxies at their authored pose and the
    /// prepared track shapes' bounds, the rest belt's ground face among them. Collision geometry
    /// alone, as for [`Self::contact_radius`]: it is what meets the ground.
    pub(crate) fn stance(&self, rotation: Quat) -> crate::terrain_grid::Stance {
        crate::terrain_grid::Stance::enclosing(
            self.stance_points.iter().map(|&point| rotation * point),
        )
    }

    /// The authority root's simulation state, inserted with `Tank` before the root can replicate
    /// or simulate. The glb is a view and never initializes replicated state.
    pub(super) fn authority_state(&self) -> impl Bundle {
        let state = &self.authority;
        (
            state.grip.clone(),
            state.transmission,
            state.rpm,
            state.sense.clone(),
            // The shaped drive signal starts at rest, as the command does.
            DriveSignal::default(),
            // Replicated client attachment must preserve the arriving authority value.
            state.gate.clone(),
            state.loads.clone(),
            state.servos.clone(),
            ServoMechanism::new(&self.transmission),
        )
    }

    /// Assemble one instance's simulation body under `root`, in one command batch, in the order
    /// its members are numbered. The GLB scene is not consulted.
    pub(super) fn assemble(&self, commands: &mut Commands, root: Entity) {
        let mut members = Vec::with_capacity(1 + self.nodes.len());
        members.push(root);
        for node in &self.nodes {
            let parent = members[node.parent];
            members.push(
                commands
                    .spawn((node.name.clone(), node.transform, ChildOf(parent)))
                    .id(),
            );
        }
        for (slot, servo) in self.servos.iter().enumerate() {
            commands.entity(members[servo.node]).insert((
                servo.spec.clone(),
                ServoCommand::default(),
                ServoIndex(slot),
                TankRoot(root),
                servo.spec.role,
                servo.rest,
            ));
        }
        for (slot, weapon) in self.weapons.iter().enumerate() {
            commands.entity(members[weapon.muzzle]).insert((
                Muzzle,
                TankRoot(root),
                WeaponIndex(slot),
                Weapon {
                    barrel: weapon.barrel.as_ref().map(|barrel| members[barrel.node]),
                    ..weapon.weapon.clone()
                },
                weapon.range_table.clone(),
            ));
            if let Some(barrel) = &weapon.barrel {
                let entity = members[barrel.node];
                commands
                    .entity(entity)
                    .insert((GunBarrel, WeaponIndex(slot), TankRoot(root)));
                if let Some(recoil) = barrel.recoil {
                    commands.entity(entity).insert(recoil);
                }
            }
        }
        for volume in &self.volumes {
            let node = members[volume.node];
            let mut entity = commands.entity(node);
            entity.insert(VolumeOf(root));
            // A part is the primitive's identity; its shape and surfaces live in the set alone.
            match &volume.matter {
                VolumeMatter::Single {
                    component,
                    substance,
                    parts,
                } => {
                    if let Some(component) = component {
                        if let Some(crew) = component.crew {
                            // Seat role + its native occupant (topology B): `home == seat` at
                            // spawn, so competence is 1.0 until a backfill swap moves an occupant
                            // to a foreign seat.
                            entity.insert((crew, Crewman { home: crew }));
                        }
                        if component.ammo {
                            entity.insert(Ammo);
                        }
                        if let Some(function) = component.function {
                            entity.insert((function.role, function.supply));
                        }
                        // Damageable (module/crew/ammo): an HP pool the march depletes.
                        entity.insert((ComponentVolume, ComponentHealth::full(component.hp)));
                    } else {
                        // Pure armour: resists + shadows spall, nothing to lose.
                        entity.insert(ArmorVolume);
                        if volume.external {
                            entity.insert(ExternalArmour);
                        }
                    }
                    match substance {
                        NodeSubstance::Hard(volume) => entity.insert(volume.clone()),
                        NodeSubstance::Witness(volume) => entity.insert(volume.clone()),
                    };
                    for _ in 0..*parts {
                        members.push(commands.spawn((ChildOf(node), Transform::IDENTITY)).id());
                    }
                }
                VolumeMatter::Split(parts) => {
                    entity.insert(ArmorVolume);
                    for shard in parts {
                        let mut part = commands.spawn((ChildOf(node), Transform::IDENTITY));
                        // The primitive IS the volume here ([`classify_volume`]): it carries the
                        // factor the walk reads and the ownership the impulse path resolves.
                        part.insert((shard.clone(), VolumeOf(root)));
                        if volume.external {
                            part.insert(ExternalArmour);
                        }
                        members.push(part.id());
                    }
                }
            }
        }
        self.armour.instantiate(&members).insert(commands, root);
        for proxy in &self.proxies {
            spawn_proxy_parts(
                commands,
                members[proxy.node],
                &proxy.colliders,
                proxy.friction,
            );
        }
        // The track view reads the stations' side and pose; the belt force model uses the BAKED
        // rest circles, so articulation is view-only. A station is also its wheel's ballistic
        // volume (pure armour, no `hp`), so no damage path can despawn one out from under the
        // track rig.
        for &(node, side) in &self.roadwheels {
            commands.entity(members[node]).insert(Roadwheel { side });
        }
        commands.entity(members[self.hull]).insert(Hull);
        commands.entity(members[self.gun]).insert(Gun);
        commands
            .entity(members[self.turret])
            .insert((Turret, self.turret_body));
        let body = &self.root;
        commands.entity(root).insert((
            body.mass,
            body.angular_inertia,
            NoAutoMass,
            NoAutoAngularInertia,
            NoAutoCenterOfMass,
            body.centre_of_mass,
            body.capabilities.clone(),
            body.views.clone(),
            // Root visibility owns the gunner-view hide: set to `Hidden`, `InheritedVisibility`
            // propagates `HIDDEN` to every descendant mesh, so the gunner optic (camera parked at
            // the gun pivot, inside the mantlet) sees no own-tank geometry — no near-plane
            // clipping.
            Visibility::Inherited,
            // Authoritative readiness and belt supply live in the root's `WeaponGate`, servo
            // integration in its `TankServos`/`RemoteServos`.
            body.sim.clone(),
            Rig {
                hull: members[self.hull],
                turret: members[self.turret],
                gunner_yaw: members[self.gunner_yaw],
                gun: members[self.gun],
                muzzle: members[self.muzzle],
            },
            SimParts(
                self.nodes
                    .iter()
                    .zip(&members[1..])
                    .map(|(node, &entity)| (node.name.as_str().to_owned(), entity))
                    .collect(),
            ),
        ));
        if let Some(sound) = &body.engine_sound {
            commands.entity(root).insert(sound.clone());
        }

        // UNIT SCALE AT THE ROOT, BIT-EXACTLY (§13.6). `bake::manifold_gate` refuses an authored
        // scale other than 1 and `ballistics::collect` refuses a collider that reaches the world at
        // one, but the collector only judges a CANDIDATE: a scaled root moves and shrinks every
        // collider's world AABB, so the swept query can miss the armour entirely and leave nothing
        // to refuse. The root bundle is arbitrary caller data, so this is where that seam closes —
        // before the first corridor, by name, on the whole hierarchy at once.
        commands.queue(move |world: &mut World| {
            let scale = world.get::<Transform>(root).map_or(Vec3::ONE, |t| t.scale);
            assert_eq!(
                scale.to_array().map(f32::to_bits),
                [1.0f32.to_bits(); 3],
                "tank root {root} was spawned at scale {scale:?}, not 1 — armour is certified in \
                 the buffer it was authored in and reaches the world through an unscaled hierarchy"
            );
        });
    }
}

/// Every spec-declared node resolved to a geometry index, in the deterministic sorted orders the
/// wire indices are assigned from. Produced by [`resolve_rig_nodes`], which asserts the rig
/// contract before returning — so every field here is a node that exists. An optional `barrel` is
/// genuinely optional (a weapon that does not reciprocate), not an unresolved name.
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
    gunner_yaw: usize,
    hull: usize,
    center_of_mass: usize,
    /// Structural assembly root declared by `TurretSpec::node`, independently of the optic chain.
    turret: usize,
    /// The single `Primary` weapon's muzzle — the rig's main bore.
    primary_muzzle: usize,
}

/// Resolve every spec-declared node name against the extracted geometry, fail-fast: a single
/// assertion names `file` and ALL missing nodes at once rather than panicking on the first.
/// Index-bearing collections are sorted here because wire-derived indices must never depend on
/// `HashMap` iteration order.
fn resolve_rig_nodes<'a>(
    file: &str,
    geometry: &'a TankGeometry,
    spec: &'a TankSpec,
) -> RigNodes<'a> {
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
        "{file}: `volumes` declares entries on nodes that are not ballistic volumes (no primitive \
         of theirs wears a registry substance material, so the march can never reach them): \
         {inert:?} — give each a substance material in the model or remove its entry"
    );
    let (gunner_yaw, gunner_pitch) = match spec.gunner_aim() {
        Some((yaw, pitch)) => (resolve(yaw), resolve(pitch)),
        None => (None, None),
    };
    let turret_index = resolve(&spec.turret.node);
    let hull_index = resolve("Hull");
    let com_index = resolve("Center_Of_Mass");

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
        "{file}: the tank model is missing required rig nodes: {missing:?} — add them to the \
         model, or name the model's own nodes in the sheet"
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
        gunner_yaw: checked(gunner_yaw),
        hull: checked(hull_index),
        center_of_mass: checked(com_index),
        turret: checked(turret_index),
        primary_muzzle: checked(primary_muzzle_index),
    }
}

/// The geometry indices of every node the body spawns ([`TankTemplate::nodes`]).
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

/// One armour volume or crew box primitive's shape, surfaces and role.
type PartShape = (Collider, BallisticSurfaces, Role<usize>);

/// The game's damage model: ballistic volume `name`, member `node` with its parts numbered from
/// member `first`, as the template spawns it (the volume bundle, design
/// `armor-penetration-and-damage.md` §12: composition, not a `kind` enum — the substance carries
/// resistance, optional facets layer roles on top), and a query-only trimesh shape per captured
/// primitive, built once from the extracted buffers ([`MeshGeometry::trimesh_collider`]: the exact
/// parry construction avian's `TrimeshFromMesh` performs, design §7.1). The shape lives in the
/// tank's armour set, never in physics, so it never perturbs the body — watertight solids may be
/// concave, fine for the march's raycast (ADR-0008).
///
/// The extracted buffers are node-LOCAL, and each primitive's entity sits under its volume's node
/// at an IDENTITY local transform. The set composes the node's chain rigidly, and a ballistic
/// volume's chain is unit-scale by contract (`bake::manifold_gate`, asserted again where the set
/// links it), so the shape the walk meets is the buffer the gate certified.
///
/// # Where the factor lands, and why it is not always the node
///
/// A `BallisticVolume` is the unit the walk reads a factor from AND the entity damage is addressed
/// to (`ballistics::walk`'s `VolumeTable`, `resolve`'s deposits), so those two must be the same
/// entity. That works exactly while a node is ONE substance, which is every part of this tank but
/// the road wheels — §13.7 authors a mixed-substance part as one object holding one closed shell
/// per substance region, and the wheel is `MildSteel` bodies + `Rubber` rims in a single object.
///
/// So: a single-substance node carries `BallisticVolume` itself. A mixed-substance node hands the
/// role down to its per-primitive part entities — one glTF primitive is one material by
/// construction, so each is a volume of exactly one substance, and the walk (which already tracks
/// presence per primitive) sees two honest volumes instead of one averaged lie.
///
/// A witness node (ADR-0040) carries `WitnessVolume` instead of `BallisticVolume`, and its
/// primitives are its set's crew box parts: no walk reads them; the witness pass does. It is never
/// split — L2.WITNESS_ROLE certifies a witness node crew, and L2.COMPONENT_SUBSTANCE certifies a
/// crew volume one substance.
///
/// The cost is that a mixed node cannot address damage: `hp` lives on the node, the deposits name
/// the shard. L2.COMPONENT_SUBSTANCE refuses such an asset at admission. Lifting it means keying
/// the walk's factor table per primitive instead of per volume — a §13 core change.
fn classify_volume(
    file: &str,
    name: &str,
    facets: Option<&VolumeSpec>,
    geometry: &crate::bake::NodeGeometry,
    node: usize,
    first: usize,
) -> (VolumeEntry, Vec<PartShape>) {
    let groups = geometry.substance_groups();
    // The extractor only lists a node here if a primitive of its wears a substance.
    assert!(
        !groups.is_empty(),
        "{file}: ballistic volume `{name}` captured no substance primitive"
    );
    let component = facets.and_then(VolumeSpec::component);
    // `Some` for a node of one substance, which the node carries.
    let single = (groups.len() == 1).then(|| {
        let (substance, primitives) = groups.iter().next().expect("exactly one group");
        let resolved = primitives[0]
            .substance
            .as_ref()
            .expect("grouped by substance");
        match resolved.matter {
            Matter::Hard {
                factor,
                debris_ratio,
            } => NodeSubstance::Hard(hard_substance(resolved, factor, debris_ratio)),
            Matter::Witness => NodeSubstance::Witness(WitnessVolume {
                substance: (*substance).to_owned(),
            }),
        }
    });
    assert!(
        single.is_some() || component.is_none(),
        "{file}: L2.COMPONENT_SUBSTANCE certified component volume `{name}` one substance, yet it \
         wears {}",
        groups.len()
    );
    let mut shards = Vec::new();
    let mut shapes = Vec::new();
    for primitives in groups.values() {
        for primitive in primitives {
            let triangles = primitive.indices.as_chunks::<3>().0.len();
            // Name the broken volume before Avian rejects an empty triangle list.
            assert!(
                triangles > 0,
                "{file}: ballistic volume `{name}` has an unindexed or triangle-less mesh primitive"
            );
            // The bake's manifold certificate, index-aligned with those triangles. Armour without
            // it is armour the walk cannot pair, so it is refused here rather than collected
            // ambiguously later.
            let certificate = primitive
                .certificate
                .as_ref()
                .unwrap_or_else(|| panic!("{file}: ballistic volume `{name}` was never certified"));
            assert_eq!(
                certificate.shells.len(),
                triangles,
                "{file}: ballistic volume `{name}` has {triangles} triangles and {} certified \
                 faces",
                certificate.shells.len(),
            );
            let surfaces = BallisticSurfaces {
                shells: certificate.shells.clone(),
                corners: certificate.corners.clone(),
            };
            // The volume a part belongs to: the witness node, the node, or (split) the primitive
            // itself.
            let role = match &single {
                Some(NodeSubstance::Witness(_)) => Role::CrewBox(node),
                Some(NodeSubstance::Hard(_)) => Role::Armour(node),
                None => {
                    let resolved = primitive.substance.as_ref().expect("grouped by substance");
                    let Matter::Hard {
                        factor,
                        debris_ratio,
                    } = resolved.matter
                    else {
                        unreachable!(
                            "L2.WITNESS_ROLE and L2.COMPONENT_SUBSTANCE: a witness node is one \
                             substance"
                        )
                    };
                    shards.push(hard_substance(resolved, factor, debris_ratio));
                    Role::Armour(first + shapes.len())
                }
            };
            shapes.push((primitive.trimesh_collider(), surfaces, role));
        }
    }
    let matter = match single {
        Some(substance) => VolumeMatter::Single {
            component: component.cloned(),
            substance,
            parts: shapes.len(),
        },
        None => VolumeMatter::Split(shards),
    };
    let volume = VolumeEntry {
        node,
        external: matches!(facets, Some(VolumeSpec::External)),
        matter,
    };
    (volume, shapes)
}

/// The walk's reading of hard primitive substance `resolved`, its matter's `factor` and
/// `debris_ratio`.
fn hard_substance(
    resolved: &crate::bake::PrimitiveSubstance,
    factor: f32,
    debris_ratio: f32,
) -> BallisticVolume {
    BallisticVolume {
        material: VolumeMaterial {
            substance: resolved.id,
            feedback: crate::substances::SubstanceRegistry::shipped_static()
                .hard(&resolved.name)
                .expect("the bake certified the volume's hard substance")
                .feedback,
            factor,
            debris_ratio,
        },
        substance: resolved.name.clone(),
    }
}

/// The tank's armour set ([`ArmourTemplate`]): its nodes, the frames among them (each servo, each
/// recoiling barrel, and the turret assembly a launch detaches) and its volumes' primitives,
/// `parts` as `(member id, node index in nodes, shape, surfaces, role)`.
fn armour_template(
    rig: &RigNodes,
    nodes: &[NodeEntry],
    at: &dyn Fn(usize) -> usize,
    parts: &[(usize, usize, Collider, BallisticSurfaces, Role<usize>)],
) -> ArmourTemplate {
    let frames: HashSet<usize> =
        rig.servo_nodes
            .iter()
            .copied()
            .chain(rig.weapon_entries.iter().zip(&rig.weapon_nodes).filter_map(
                |((_, weapon), &(_, barrel))| barrel.filter(|_| weapon.recoil.is_some()),
            ))
            .chain([rig.turret])
            .map(at)
            .collect();
    let armour_nodes: Vec<Node> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| Node {
            name: node.name.as_str(),
            id: index + 1,
            parent: (node.parent != ROOT).then(|| node.parent - 1),
            transform: node.transform,
            frame: frames.contains(&(index + 1)),
        })
        .collect();
    ArmourTemplate::build(
        ROOT,
        &armour_nodes,
        parts
            .iter()
            .map(|(id, node, shape, surfaces, role)| PartSource {
                id: *id,
                node: Some(*node),
                pose: Transform::IDENTITY,
                shape,
                surfaces: Some(surfaces),
                role: *role,
            }),
    )
}

/// Collision proxy `node_name`'s hulls for `primitive`: one convex hull per loose part, on the
/// Vehicle layer once spawned ([`MeshGeometry::convex_hull_colliders`]). Collision-only: they
/// contribute no mass (the root authors its own).
fn proxy_hulls(node_name: &str, primitive: &MeshGeometry) -> Vec<Collider> {
    primitive.convex_hull_colliders().unwrap_or_else(|| {
        panic!(
            "collision proxy `{node_name}`: the bake gate certifies every proxy's hulls \
             (declared_roles_resolve)"
        )
    })
}

/// The friction collision proxy `node`'s hulls carry: the substance the sheet's `colliders` entry
/// names. Both panics are bugs: the bake's proxies are the sheet's `colliders`, and the bake
/// refuses a sheet whose collider substance the registry gives no friction for
/// (`bake::collider_substances`, `L2.SPEC`).
pub(crate) fn proxy_friction(spec: &TankSpec, node: &str) -> Friction {
    let collider = spec
        .colliders
        .iter()
        .find(|collider| collider.node == node)
        .unwrap_or_else(|| {
            panic!("collision proxy `{node}` is not declared in `colliders`, yet the bake made it")
        });
    crate::substances::SubstanceRegistry::shipped_static()
        .contact_friction(&collider.substance)
        .unwrap_or_else(|err| {
            panic!("collision proxy `{node}`: {err}, yet the bake certified its sheet")
        })
}

/// One child collider under `parent` per hull in `colliders`, carrying `friction`: all parts of a
/// proxy ride the same body.
fn spawn_proxy_parts(
    commands: &mut Commands,
    parent: Entity,
    colliders: &[Collider],
    friction: Friction,
) {
    for collider in colliders {
        commands.spawn((
            ChildOf(parent),
            // ADR-0015 shields this authored local pose from position sync.
            authored_attachment(Transform::IDENTITY),
            collider.clone(),
            CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            // Its substance's, like every collider's: the belt model owns the hull's grip
            // wherever the belts carry the load, and the proxies sit clear of the ground there.
            friction,
        ));
    }
}

/// The complete rig's camera record: the spec gate requires every kind before spawn.
fn authored_view(spec: &TankSpec, kind: ViewKind) -> ViewConfig {
    let view = spec
        .views
        .get(&kind)
        .expect("the spec gate certifies every playable view");
    ViewConfig {
        fov: view.optics.vertical_fov(),
        requires: view.requires.clone(),
    }
}

/// The engine's recording and the crank facts its playback speed derives from, lifted out of the
/// declared transmission. `None` for an engine that authors no `sound`.
fn engine_sound(spec: &TankSpec) -> Option<EngineSound> {
    let engine = &spec.track.powertrain.transmission.engine;
    let sound = engine.sound.as_ref()?;
    Some(EngineSound {
        clip: sound.clip.clone(),
        clip_pop_hz: sound.clip_pop_hz,
        cylinders: engine.cylinders,
        idle_rpm: engine.idle_rpm,
        governed_rpm: engine.governed_rpm,
    })
}

/// [`TankTemplate::contact_radius`].
fn contact_radius(geometry: &TankGeometry, track_colliders: &TrackColliders) -> f32 {
    let mut radius = track_colliders.contact_radius();
    for &index in &geometry.collision_proxies {
        let node = &geometry.nodes[index];
        let mut proxy = node
            .primitives
            .iter()
            .flat_map(|primitive| &primitive.positions)
            .map(|point| Vec3::from(*point).length())
            .fold(0.0, f32::max);
        let mut ancestor = Some(index);
        while let Some(index) = ancestor {
            let node = &geometry.nodes[index];
            proxy = transformed_radius(proxy, &node.transform);
            ancestor = node.parent;
        }
        radius = radius.max(proxy);
    }
    assert!(
        radius.is_finite() && radius > 0.0,
        "the eager bake certifies finite, nonempty tank collision geometry"
    );
    radius
}

/// DERIVED triangle-inequality bound for a sphere through one authored local transform.
fn transformed_radius(radius: f32, transform: &Transform) -> f32 {
    transform.translation.length() + transform.scale.abs().max_element() * radius
}

/// [`TankTemplate::stance_points`]: every declared proxy point at its authored root-local pose,
/// then the track shapes' bound corners.
fn stance_points(geometry: &TankGeometry, track_colliders: &TrackColliders) -> Vec<Vec3> {
    geometry
        .collision_proxies
        .iter()
        .flat_map(|&index| {
            let node = &geometry.nodes[index];
            node.primitives
                .iter()
                .flat_map(|primitive| &primitive.positions)
                .map(|&point| {
                    node.root_position + node.root_rotation * (node.root_scale * Vec3::from(point))
                })
        })
        .chain(track_colliders.bound_corners())
        .collect()
}

#[cfg(test)]
/// The shipped tank's construction data, a fixture: its blueprint and template.
pub(crate) fn shipped_construction() -> (TankBlueprint, TankTemplate) {
    let (spec, glb) = shipped_sheet();
    let geometry = crate::bake::extract_tank_geometry(
        &glb,
        &spec,
        &crate::substances::SubstanceRegistry::shipped(),
    )
    .unwrap();
    let blueprint = TankBlueprint::new(
        super::spawn::TANK_SPEC_PATH.into(),
        crate::bake::CertifiedAsset::fixture(spec, geometry),
    );
    let template = TankTemplate::new(&blueprint, TrackColliders::of_blueprint(&blueprint, &glb));
    (blueprint, template)
}

#[cfg(test)]
/// The shipped tank's sheet and the path of its sim model.
fn shipped_sheet() -> (TankSpec, std::path::PathBuf) {
    let spec = crate::provenance::from_shipped_str(
        super::spawn::TANK_SPEC_PATH,
        include_str!(concat!(
            "../../assets/",
            crate::assets::tank_file!(".tank.ron")
        )),
    )
    .unwrap();
    (
        spec,
        crate::assets::asset_root().join(super::spawn::TANK_SIM_GLB_PATH),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structural node and a yaw actuator are distinct roles. Complete construction
    /// must resolve the sheet's handles separately even when the shipped fixture shares them.
    #[test]
    fn rig_resolution_keeps_assembly_and_gunner_yaw_separate() {
        let (mut spec, glb) = shipped_sheet();
        let mut geometry = crate::bake::extract_tank_geometry(
            &glb,
            &spec,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .unwrap();
        // Fixture construction adds an independent assembly, without assuming the selected
        // vehicle's hull or any existing node is separate from its declared yaw.
        let name = "FixtureAssembly";
        assert!(!geometry.by_name.contains_key(name));
        geometry.by_name.insert(name.into(), geometry.nodes.len());
        geometry.nodes.push(crate::bake::NodeGeometry {
            name: name.into(),
            parent: Some(geometry.by_name["Hull"]),
            transform: Transform::IDENTITY,
            root_position: Vec3::ZERO,
            root_rotation: Quat::IDENTITY,
            root_scale: Vec3::ONE,
            primitives: Vec::new(),
        });
        spec.turret.node = name.into();
        let nodes = resolve_rig_nodes("fixture", &geometry, &spec);
        assert_eq!(nodes.turret, geometry.by_name[&spec.turret.node]);
        assert_eq!(
            nodes.gunner_yaw,
            geometry.by_name[spec.gunner_yaw.as_ref().unwrap()]
        );
        assert_ne!(nodes.turret, nodes.gunner_yaw);
    }

    /// A tank kind whose sheet names a node its model lacks is refused when its template is built,
    /// at load, naming its file and the node; a template deferring resolution to spawn lets a
    /// server start and fail at its first join. Fixture: the shipped sheet with a servo renamed to
    /// a node no model holds.
    #[test]
    #[should_panic(expected = "fixture.tank.ron: the tank model is missing required rig nodes")]
    fn a_kind_naming_a_missing_node_is_refused_when_its_template_is_built() {
        let (mut spec, glb) = shipped_sheet();
        let geometry = crate::bake::extract_tank_geometry(
            &glb,
            &spec,
            &crate::substances::SubstanceRegistry::shipped(),
        )
        .unwrap();
        let servo = spec
            .servos
            .keys()
            .next()
            .expect("the fixture tank has a servo")
            .clone();
        let entry = spec.servos.remove(&servo).expect("listed above");
        spec.servos.insert("FixtureMissingNode".into(), entry);
        let blueprint = TankBlueprint::new(
            "fixture.tank.ron".into(),
            crate::bake::CertifiedAsset::fixture(spec, geometry),
        );
        TankTemplate::new(&blueprint, TrackColliders::of_blueprint(&blueprint, &glb));
    }

    /// Spawn clearance must enclose constructed points at any rotation, including mirrored and
    /// nonuniform scale. Fixtures vary translation, scale and orientation independently; omitting
    /// either construction term breaks the bound on a transformed point.
    #[test]
    fn construction_radius_encloses_rotated_scaled_and_offset_points() {
        for translation in [Vec3::ZERO, Vec3::new(8.0, -3.0, 4.0)] {
            for scale in [Vec3::ONE, Vec3::new(-3.0, 0.5, 2.0)] {
                for angle in [0.0, 0.7, 2.1] {
                    let transform = Transform {
                        translation,
                        scale,
                        rotation: Quat::from_rotation_y(angle) * Quat::from_rotation_x(angle),
                    };
                    for point in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(3.0, -4.0, 2.0)] {
                        assert!(
                            transform.transform_point(point).length()
                                <= transformed_radius(point.length(), &transform) + 1e-5
                        );
                    }
                }
            }
        }
    }

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
        spawn_proxy_parts(
            &mut commands,
            node,
            &proxy_hulls("proxy", &primitive),
            crate::substances::contact_friction(0.5),
        );
        world.flush();

        let parents: Vec<Entity> = world
            .query::<(&Collider, &ChildOf)>()
            .iter(&world)
            .map(|(_, child_of)| child_of.parent())
            .collect();
        assert_eq!(parents, vec![node, node]);
    }

    /// THE SPAWN STANCE COVERS THE GROUND CONTACT. Under any spawn turn, the stance a tank spawns
    /// under reaches past every road-contact cylinder of its running gear and down to its rest
    /// belt's ground face, so the spawn rule's footprint samples all the ground the tank stands on
    /// and its lowest point starts clear of it. A stance of the hull's proxies alone, of track
    /// shapes cut short of the rest belt, or of another pose than the spawn's, fails here.
    #[test]
    fn the_spawn_stance_covers_the_running_gears_ground_contact() {
        let (blueprint, template) = shipped_construction();
        let track = &blueprint.spec.track;
        let params = track.suspension.params();
        let geom = crate::track::rig_geom::RigGeom::build(
            &blueprint,
            &crate::assets::asset_root().join(super::super::spawn::TANK_SIM_GLB_PATH),
            track.sprocket.teeth,
            track.link_count,
            &params,
        );
        // Resolution: f32 rounding of root-frame coordinates a few metres out.
        const ROUNDING_M: f32 = 1e-4;
        // FIXTURE DATA: spawn turns about the vertical.
        for yaw in [0.0_f32, 0.6, 2.5] {
            let stance = template.stance(Quat::from_rotation_y(yaw));
            assert!(
                stance.depth_m >= geom.hull_rest_y - ROUNDING_M,
                "turn {yaw}: the stance hangs {} m, short of the rest belt's {} m",
                stance.depth_m,
                geom.hull_rest_y
            );
            for side in crate::track::side::Side::ALL {
                for (centre, tread) in geom
                    .road_wheels
                    .get(side)
                    .iter()
                    .zip(geom.wheel_treads.get(side))
                {
                    for &(start, end) in &tread.spans {
                        // The cylinder's farthest rim from the vertical axis: its farther face, at
                        // the axle's height, fore or aft of the centre.
                        let lateral = (centre.x + start).abs().max((centre.x + end).abs());
                        let reach = lateral.hypot(centre.z.abs() + tread.radius);
                        assert!(
                            stance.reach_m >= reach - ROUNDING_M,
                            "turn {yaw}: a road wheel at {centre} reaches {reach} m, past the \
                             stance's {} m",
                            stance.reach_m
                        );
                    }
                }
            }
        }
    }
}
