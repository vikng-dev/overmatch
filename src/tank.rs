//! The tank: its rig (structural markers bound by node name), the kinematic `Servo` motor
//! for the turret/gun, and the asset-load binding. The tank declares *structure*; features
//! (aim, shooting) attach their own behavior to these markers reactively.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::{
    AngularInertia, ColliderConstructor, ColliderConstructorHierarchy, CollisionLayers, LayerMask,
    Mass, NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, RigidBody,
};
use bevy::asset::LoadState;
use bevy::gltf::GltfMaterialName;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use serde::Deserialize;

use crate::Layer;
use crate::ballistics::{ArmorVolume, BallisticVolume, ComponentHealth, ComponentVolume};
use crate::damage::{
    Ammo, Crewman, Requirement, TankCapabilities, TankVolumes, VolumeFacets, VolumeOf, evaluate,
    part_qualities,
};
use crate::sight::SightMode;
use crate::spec::{RecoilSpec, TankSpec, TankSpecHandle, Trigger, ViewKind};
use crate::state::{AppState, GameplaySet};

// --- Rig markers. Name = the structural contract between the model and the code. ---

#[derive(Component)]
pub struct Turret;

#[derive(Component)]
pub struct Gun;

#[derive(Component)]
pub struct Hull;

/// Marks the vehicle's root entity — the dynamic rigid body (chassis). Suspension/drive forces
/// are applied here; debug x-ray walks its descendants. Deliberately LOCAL, never replicated:
/// its `On<Add, Tank>` observers must fire alongside the rig bundle, not at replication-receive
/// time (see `net::protocol::NetTank` for the wire-side identity marker and the bind-window
/// regression that rule comes from).
#[derive(Component)]
pub struct Tank;

/// Marks the one tank the player is currently commanding. Exactly one tank carries this at a time;
/// the swap input ([`swap_controlled_tank`]) moves it. The *control* systems (drive input, aiming,
/// cameras, shooting, gunner sight) scope to this marker so they act on the player's tank alone;
/// everything tank-agnostic (suspension support, ballistics, damage) ignores it and runs for every
/// tank. `Controlled` answers *which* tank; [`Rig`] answers *where its parts are*.
#[derive(Component)]
pub struct Controlled;

/// Resolved handles to a tank's rig nodes, captured once when the rig binds ([`on_tank_ready`]).
/// Lets a control system reach *this* tank's specific gun/turret/muzzle by entity (`rig.gun`)
/// instead of `query.single()`, which silently assumed a single tank in the world. Lives on the
/// root, so it shares the tank's lifetime — the handles can't dangle (the parts despawn with the
/// root they're parented to). Captured from the same descendant walk that already enforces the rig
/// contract, so every field is guaranteed present by the time `Rig` is inserted.
#[derive(Component)]
pub struct Rig {
    pub hull: Entity,
    pub turret: Entity,
    pub gun: Entity,
    pub muzzle: Entity,
}

/// Per-view runtime config bound from the spec's `views` map: the camera FOV and the gating
/// requirement. Keyed by [`ViewKind`] in [`TankViews`] on the tank root; the camera reads `fov`,
/// the sight systems gate on `requires` (the per-view successor to the old `GunnerSight`/
/// `CommanderView` capabilities).
pub struct ViewConfig {
    pub fov: f32,
    pub requires: Requirement,
}

/// The controlled-tank views (camera anchors), bound from the spec. The camera and sight systems
/// look up the active [`ViewKind`] here for its FOV and gating requirement.
#[derive(Component)]
pub struct TankViews(pub HashMap<ViewKind, ViewConfig>);

#[derive(Component)]
pub struct Muzzle;

/// The recoiling barrel node (child of `Gun`, parent of `Muzzle`).
#[derive(Component)]
pub struct GunBarrel;

/// A bound weapon's runtime config, attached by the binder to its muzzle entity from the spec's
/// `weapons` map. The shooting systems read it (ballistics for the shell, `reload` for the cooldown,
/// `recoil` to kick the `barrel`). Replaces the hardcoded `shooting.rs` consts; `barrel` is the
/// resolved recoil node (`None` for a barrel-less weapon like a coax).
#[derive(Component)]
pub struct Weapon {
    /// The weapon's logical name (the `weapons` map key, e.g. `MainGun`) — what the HUD shows, as
    /// opposed to the muzzle node's name (`Main_Gun_Muzzle`).
    pub name: String,
    pub speed: f32,
    pub caliber: f32,
    pub mass: f32,
    pub reload: f32,
    pub recoil: Option<RecoilSpec>,
    pub barrel: Option<Entity>,
    /// Fire / load gates (design §7b), evaluated against the controlled tank by the shooting
    /// systems — the per-weapon successors to the old global `Fire`/`Load` capabilities.
    pub fire: Requirement,
    pub load: Requirement,
    /// Which fire input drives this weapon (LMB = `Primary`, Spacebar = `Secondary`).
    pub trigger: Trigger,
}

/// Which track a roadwheel drives (for differential thrust). Left wheels sit at −X, right at +X.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrackSide {
    Left,
    Right,
}

/// A load-bearing roadwheel — a suspension/drive contact station, tagged with its track side;
/// the sprocket and idler are excluded. Its suspension ray is cast fresh each tick by
/// `apply_suspension` (see there for why it is not a `RayCaster` component).
#[derive(Component)]
pub struct Roadwheel {
    pub side: TrackSide,
}

/// The authored centre-of-mass: an Empty (`Center_Of_Mass`) placed in the model. `driving` reads
/// its position and sets the body's centre of mass from it — the model owns the COM.
#[derive(Component)]
pub struct CenterOfMassAnchor;

/// Back-link from a rig part (a servo) to its tank's root entity, set at bind. Lets a per-tank
/// system that runs over *all* tanks' parts (`drive_servos`) resolve the owning tank's
/// `TankVolumes` to evaluate that part's gate — without walking the hierarchy each frame.
#[derive(Component)]
pub struct TankRoot(pub Entity);

/// Travel limits for a [`ServoSpec`], in **degrees** (the authoring unit).
#[derive(Clone, Copy, Deserialize)]
pub enum Travel {
    Limited { min: f32, max: f32 },
    Continuous,
}

/// Which aiming degree of freedom a servo actuates — control semantics *and* the rotation axis in
/// one. `Yaw` rotates about local +Y (traverse), `Pitch` about local +X (elevation); the axis is
/// derived here rather than re-declared, since for any cardinal mount the role determines it (a
/// yaw is *by definition* about the vertical). A canted mount would add a `Custom(Dir3)` variant.
#[derive(Component, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ServoRole {
    Yaw,
    Pitch,
}

impl ServoRole {
    fn axis(self) -> Vec3 {
        match self {
            ServoRole::Yaw => Vec3::Y,
            ServoRole::Pitch => Vec3::X,
        }
    }
}

// A 1-DOF kinematic rotational motor (trapezoidal motion profile), split three ways so each
// concern has one owner: per-variant config, the commanded intent, and the live mechanism state.
// `drive_servos` is the behaviour; it reads spec + command and drives state + the transform.

/// Servo config: rotation axis, speed/accel limits, travel range. Per-variant data authored in the
/// tank's `.tank.ron` spec sheet (ADR-0010) and applied to the bound servo node. Angles are in
/// **degrees** — the human-facing authoring unit; `drive_servos` converts to radians (the
/// computed/runtime unit shared with `ServoCommand` and `ServoState`).
#[derive(Component, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServoSpec {
    /// Which aim DOF this servo drives — its handle for role-based binding (no fixed node name), and
    /// the source of its rotation axis (Yaw→Y, Pitch→X; see [`ServoRole`]).
    role: ServoRole,
    /// Max slew speed, degrees/second.
    max_speed: f32,
    /// Slew acceleration, degrees/second².
    accel: f32,
    travel: Travel,
    /// The slew gate (design §7b): what must be intact/crewed for this mount to traverse — e.g. the
    /// gunner (and, later, a traverse motor). `drive_servos` scales the slew by its effectiveness,
    /// so a dead operator freezes the mount and a damaged motor (future) just slows it. Empty =
    /// always free. This is the per-servo successor to the old global `Traverse` capability.
    #[serde(default)]
    pub(crate) requires: Requirement,
}

/// The commanded angle (parent-local) a servo slews toward — the *intent*, written by aiming
/// (and, later, the ROADMAP Phase-2 controls layer). Position-mode for now; a velocity-mode
/// command is a future variant (NOTES.md). Kept separate from state: different writer, different
/// lifecycle.
#[derive(Component, Default)]
pub struct ServoCommand {
    pub target: f32,
}

/// A servo's live mechanism state — current angle and angular velocity of the slew. Owned by
/// `drive_servos`; never authored, never shared.
///
/// `rest` is the node's authored pose at `current = 0`, captured once so the pose write can be an
/// *absolute* rotation (`rest · R(axis, angle)`) instead of accumulating deltas (no round-off).
///
/// **Fixed-clock split:** `drive_servos` steps the mechanism (`previous` → `current`) on the fixed
/// clock — servo pose is sim truth (the muzzle it carries decides where shells go), so the server
/// can replay it deterministically. The node's `Transform` is *render*: `interpolate_servos`
/// blends `previous → current` by the fixed clock's overstep each frame, so the turret is smooth
/// at any frame rate — matching how Avian interpolates the hull (ADR-0004's sim-in-fixed bet,
/// now covering the mechanisms too).
///
/// An element of [`TankSim::servos`], NOT a component — a tank's carried mechanism state lives
/// root-resident (see [`TankSim`] for why).
#[derive(Clone, PartialEq, Debug)]
pub struct ServoState {
    current: f32,
    /// The angle at the previous fixed tick — the render interpolation's blend-from.
    previous: f32,
    velocity: f32,
    rest: Quat,
    captured: bool,
}

impl Default for ServoState {
    fn default() -> Self {
        Self {
            current: 0.0,
            previous: 0.0,
            velocity: 0.0,
            rest: Quat::IDENTITY,
            captured: false,
        }
    }
}

impl ServoState {
    /// The servo's current angle (radians, parent-local) — its live mechanism position. Read by the
    /// gunner sight to clamp how far the aim intent may lead the gun (the on-screen margin).
    pub fn current(&self) -> f32 {
        self.current
    }
}

/// One weapon's carried sim state — an element of [`TankSim::weapons`]. `reload_remaining` gates
/// firing (0 = loaded); the recoil pair is the barrel's 1-DOF damped spring along the bore, which
/// is muzzle pose and therefore sim truth (a shell fired mid-recoil leaves from the recoiled
/// muzzle, replay included — previously the recoil state lived unregistered on the barrel, a
/// small determinism gap).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WeaponState {
    pub reload_remaining: f32,
    pub recoil_offset: f32,
    pub recoil_velocity: f32,
}

/// ALL of a tank's non-physics carried sim state, root-resident: servo mechanisms, weapon
/// reload/recoil, and the wheels' brush anchors — indexed by the bind-time [`ServoIndex`]/
/// [`WeaponIndex`]/[`WheelIndex`] on the corresponding rig child.
///
/// Root-resident is the load-bearing choice, not a convenience: under prediction, rollback
/// restores state per ENTITY, and the tank root is the one replicated/predicted entity — state on
/// the root gets history and replays with a plain `local_rollback::<TankSim>()`, while state on
/// glb-bound children needed the whole `DeterministicPredicted` decoration machinery (history
/// attach, pose-history stripping, despawn-grace windows — the step-7/8 hazard cluster, deleted
/// with this move). Children keep config plus an index; their transforms are DERIVED from this
/// state every tick, replays included.
///
/// Index assignment is sorted-by-name at bind (`on_tank_ready`) — the spec's servo/weapon maps
/// are `HashMap`s, and their iteration order must never leak into indices that client and server
/// both derive.
#[derive(Component, Clone, PartialEq, Debug, Default)]
pub struct TankSim {
    pub servos: Vec<ServoState>,
    pub weapons: Vec<WeaponState>,
    /// Per-wheel brush anchor: the world point the contact "gripped" while near rest. `Some` =
    /// static friction holds the tank there; `None` = slipping (kinetic) or airborne.
    pub anchors: Vec<Option<Vec3>>,
}

/// This servo's slot in its tank's [`TankSim::servos`], assigned at bind in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct ServoIndex(pub usize);

/// This weapon's slot in [`TankSim::weapons`] — on the muzzle AND the recoiling barrel (both
/// actuate from the same weapon state), assigned at bind in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct WeaponIndex(pub usize);

/// This roadwheel's slot in [`TankSim::anchors`], assigned at bind in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct WheelIndex(pub usize);

/// Tank spawning, the spec→rig binder, and the servo mechanism — authority-side.
/// The single-player *scenario*: load the Tiger spec up front, spawn the two-tank duel setup once
/// it's ready (entering `Playing`), first tank `Controlled`. Split from [`sim_plugin`] because
/// spawning is per-configuration — the networked server spawns per connected client instead — while
/// the mechanisms below are the sim wherever tanks come from.
pub fn sp_spawn_plugin(app: &mut App) {
    app.add_systems(Startup, load_tank_assets).add_systems(
        Update,
        spawn_tank_when_loaded.run_if(in_state(AppState::Loading)),
    );
}

pub fn sim_plugin(app: &mut App) {
    app
        // The servo mechanism steps on the fixed clock (sim truth — the muzzle pose decides where
        // shells go), *after* `GameplaySet` so `drive_aim_servos` has written this tick's targets.
        // The node `Transform` is double-clocked: inside the fixed loop it is TICK TRUTH —
        // `restore_servo_truth` re-asserts it at tick start (undoing the render lerp) and
        // `drive_servos` writes the freshly-stepped angle at tick end, so every sim reader
        // (`fire`'s muzzle chain via `rig_world_pose`, avian's child-collider sync, every tick of
        // a rollback replay) sees the mechanism's state, not the smoothed picture. Between fixed
        // runs, `interpolate_servos` (Update) blends the last two tick angles by the clock's
        // overstep for rendering — smooth at any frame rate, same split Avian uses for the hull.
        .add_systems(
            FixedUpdate,
            restore_servo_truth
                .run_if(in_state(AppState::Playing))
                .before(GameplaySet),
        )
        .add_systems(
            FixedUpdate,
            drive_servos
                .run_if(in_state(AppState::Playing))
                .after(GameplaySet),
        )
        .add_systems(
            Update,
            interpolate_servos.run_if(in_state(AppState::Playing)),
        );
}

/// The Tab possession swap — client-side: which tank *this seat* controls is not the sim's
/// business (under MP the server maps client→tank; the swap becomes a host/debug tool).
pub fn client_plugin(app: &mut App) {
    // Runs before `GameplaySet` so the control systems this frame already see the new
    // `Controlled` tank.
    app.add_systems(
        Update,
        swap_controlled_tank
            .run_if(in_state(AppState::Playing))
            .before(GameplaySet),
    );
}

/// The tank's load dependencies (ADR-0011): the spec sheet AND the glb scene, both kicked off up
/// front, and a tank is spawned only once both are ready — no spec-less window, and the scene
/// instantiates within ~a frame of spawn instead of after a multi-second glb load. That window
/// matters most under netcode: a replicated-but-unbound tank is prediction-visible the whole time
/// (the bind-window hazard class `net::rig` guards). Kicked off once at startup on every side
/// (sandbox.rs's `load_target`/`PendingTarget` pattern) — `on_tank_ready` requires the spec
/// already loaded (asserts on it). Shared with the networking layer
/// (`net::rig`/`net::client`/`net::server`), which spawns tanks against the same dependency.
#[derive(Resource)]
pub(crate) struct PendingTankAssets {
    pub spec: Handle<TankSpec>,
    pub scene: Handle<bevy::world_serialization::WorldAsset>,
}

impl PendingTankAssets {
    /// Both load dependencies resolved — the one gate every spawn path shares.
    pub(crate) fn loaded(&self, asset_server: &AssetServer) -> bool {
        matches!(asset_server.load_state(&self.spec), LoadState::Loaded)
            && matches!(asset_server.load_state(&self.scene), LoadState::Loaded)
    }
}

/// The tank model's asset path — shared with `bake`'s extractor, which parses the same file as
/// data (one path, two readers, guaranteed to agree on the source bytes).
pub(crate) const TIGER_GLB_PATH: &str = "tiger_1/tiger_1.glb";

pub(crate) fn load_tank_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(PendingTankAssets {
        spec: asset_server.load("tiger_1/tiger_1.tank.ron"),
        scene: asset_server.load(GltfAssetLabel::Scene(0).from_asset(TIGER_GLB_PATH)),
    });
}

/// Once both tank assets have loaded, spawn the duel and enter `Playing`. A *failed* load is
/// fatal here (no fallback stats, ADR-0011); still-loading just waits another frame.
fn spawn_tank_when_loaded(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pending: Option<Res<PendingTankAssets>>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(pending) = pending else {
        return;
    };
    for handle in [pending.spec.id().untyped(), pending.scene.id().untyped()] {
        if let LoadState::Failed(err) = asset_server.load_state(handle) {
            error!("required tank asset failed to load: {err}");
            panic!("required tank asset failed to load: {err}");
        }
    }
    if !pending.loaded(&asset_server) {
        return;
    }
    // Two tanks, both player-owned: `Tab` swaps which one is `Controlled`. The first spawns
    // controlled; the second sits until you swap into it (design: the antagonist/auto-aim
    // lands in Chunk 2). Both are dynamic bodies — per-tank suspension holds each up; only
    // the controlled one takes drive input.
    spawn_tank(
        &mut commands,
        &pending,
        Transform::from_xyz(10.0, 2.0, 5.0).with_rotation(Quat::from_rotation_z(0.7)),
        "Tiger I (A)",
        true,
    );
    spawn_tank(
        &mut commands,
        &pending,
        Transform::from_xyz(10.0, 2.0, -12.0),
        "Tiger I (B)",
        false,
    );
    commands.remove_resource::<PendingTankAssets>();
    next.set(AppState::Playing);
}

/// The spawn core every tank shares, whatever world it lives in: the Tiger scene (drives
/// [`on_tank_ready`] when it lands — preloaded, so it instantiates within ~a frame), the spec
/// handle the binder reads, and the [`Tank`] marker. The three spawn paths compose it with their
/// role differences — SP ([`spawn_tank`]) adds a world pose + `RigidBody::Dynamic` from birth;
/// the networked paths (`net::rig::net_tank_rig`) add the wire identity marker and stay `Static`
/// until the rig binds.
pub(crate) fn tank_rig(assets: &PendingTankAssets) -> impl Bundle {
    (
        WorldAssetRoot(assets.scene.clone()),
        TankSpecHandle(assets.spec.clone()),
        Tank,
    )
}

/// Spawn one Tiger at `transform`, binding its rig via [`on_tank_ready`]. `controlled` seeds the
/// player's starting tank with the [`Controlled`] marker. The hull is a dynamic rigid body — Avian
/// owns its Transform (ADR-0005); its collider comes from the model's `*_Collider` proxy, bound in
/// `on_tank_ready`.
fn spawn_tank(
    commands: &mut Commands,
    assets: &PendingTankAssets,
    transform: Transform,
    name: &str,
    controlled: bool,
) {
    let mut tank = commands.spawn((
        tank_rig(assets),
        transform,
        Name::new(name.to_string()),
        // Dynamic from birth: the SP scenario's assets are fully loaded at spawn, so the rig
        // binds within ~a frame — no meaningful colliderless free-fall window to guard.
        RigidBody::Dynamic,
    ));
    tank.observe(on_tank_ready);
    if controlled {
        tank.insert(Controlled);
    }
}

/// `Tab` hands control to the next tank: it moves the [`Controlled`] marker and resets the view to
/// third-person — so you never inherit the gunner optic's tank-hide on the tank you just stepped out
/// of. The mode change re-runs `sync_optic_render_layer`, which restores both tanks' render layers.
/// With two tanks this is a toggle; it generalizes to cycling N in spawn order.
fn swap_controlled_tank(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    tanks: Query<Entity, With<Tank>>,
    controlled: Query<Entity, With<Controlled>>,
    mut mode: ResMut<SightMode>,
) {
    if !keys.just_pressed(KeyCode::Tab) {
        return;
    }
    let Ok(current) = controlled.single() else {
        return;
    };
    let all: Vec<Entity> = tanks.iter().collect();
    if all.len() < 2 {
        return;
    }
    let idx = all.iter().position(|&e| e == current).unwrap_or(0);
    let next = all[(idx + 1) % all.len()];
    if next == current {
        return;
    }
    commands.entity(current).remove::<Controlled>();
    commands.entity(next).insert(Controlled);

    // Drop back to third-person; `sync_optic_render_layer` reacts to the change and reveals both
    // tanks (the optic's render-layer hide only applies in gunner view).
    *mode = SightMode::ThirdPerson;
}

/// The track side of a roadwheel *rig empty* — `Wheel_L_<n>` / `Wheel_R_<n>` with a purely numeric
/// index, and nothing else. The numeric check is load-bearing: it excludes the wheel's typed
/// children (`Wheel_L_0_Ballistic`, `Wheel_L_0_Visual`), which also begin with `Wheel_` but are a
/// volume / render mesh, not a suspension station.
pub(crate) fn roadwheel_side(name: &str) -> Option<TrackSide> {
    for (prefix, side) in [
        ("Wheel_L_", TrackSide::Left),
        ("Wheel_R_", TrackSide::Right),
    ] {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
        {
            return Some(side);
        }
    }
    None
}

/// Tick-truth world pose of a rig node: the body root's physics `Position`/`Rotation` composed
/// down the node chain's local `Transform`s. Sim systems must use this instead of the node's
/// `GlobalTransform`, which is the *render* pose — propagated once per frame from the interpolated
/// picture, up to a frame stale against the physics state, and frozen through a rollback replay.
/// That staleness differs between a vsync-paced client and an unthrottled server, so a sim that
/// reads it diverges fastest exactly when the hull's orientation changes fastest (measured: the
/// step-8 washboard/high-speed-turn rollback stream).
///
/// The chain's local transforms are tick-truth inside the fixed loop: servo nodes are restored
/// from `ServoState` at tick start (`restore_servo_truth`) and re-written by `drive_servos`, the
/// barrel's recoil transform is stepped in `FixedUpdate`, everything else is static. Rig chains
/// are authored scale-1, so composition is rigid. `None` if `node` isn't a descendant of `root`
/// (a despawn-in-flight frame).
pub(crate) fn rig_world_pose(
    node: Entity,
    root: Entity,
    root_position: Vec3,
    root_rotation: Quat,
    parents: &Query<&ChildOf>,
    locals: &Query<&Transform>,
) -> Option<(Vec3, Quat)> {
    let mut chain = Vec::new();
    let mut entity = node;
    while entity != root {
        chain.push(entity);
        entity = parents.get(entity).ok()?.parent();
    }
    let mut position = root_position;
    let mut rotation = root_rotation;
    for &link in chain.iter().rev() {
        let local = locals.get(link).ok()?;
        position += rotation * local.translation;
        rotation *= local.rotation;
    }
    Some((position, rotation))
}

/// Walk up the model hierarchy from `start` (inclusive) and return the first ancestor that's in
/// `candidates` — used to resolve a weapon's chain (the yaw / pitch servo above its muzzle).
fn first_ancestor_in(
    mut entity: Entity,
    candidates: &HashSet<Entity>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    loop {
        if candidates.contains(&entity) {
            return Some(entity);
        }
        match parents.get(entity) {
            Ok(parent) => entity = parent.parent(),
            Err(_) => return None,
        }
    }
}

/// Walk the loaded scene and, in one pass, bind structural markers + apply the (already-loaded)
/// per-variant spec to each part — servo configs, the suspension ray, the collider's density — and
/// enforce the rig contract: every node the sim binds behaviour to must exist in the model.
/// Missing structure is an authoring bug — fatal like a bad spec sheet (ADR-0010) — so we panic
/// with the list of what's absent. This is where ADR-0002's "name = the contract" is *enforced*.
pub fn on_tank_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    children: Query<&Children>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
    primitives: Query<(), With<GltfMaterialName>>,
    handles: Query<&TankSpecHandle>,
    specs: Res<Assets<TankSpec>>,
) {
    // The spec is a load dependency of spawning (ADR-0011): the tank is spawned only once its
    // `.tank.ron` has loaded, so it's guaranteed present here. Its absence would be a bug.
    let spec = handles
        .get(ready.entity)
        .ok()
        .and_then(|handle| specs.get(&handle.0))
        .expect("tank spec must be loaded before the tank is spawned");

    // Hull-level per-variant data. Mass properties are AUTHORED, never derived from the abstract
    // collision proxy (ADR-0011): `NoAuto*` makes the proxy (and the future turret ramming collider)
    // contribute zero mass — they are collision-only. Mass is the balance figure; angular inertia is
    // a box of the authored extents at that mass (distribution only); the centre of mass is the
    // authored `Center_Of_Mass` empty, applied authoritatively by `set_center_of_mass`.
    let (ex, ey, ez) = spec.inertia_extents;
    commands.entity(ready.entity).insert((
        spec.drivetrain.clone(),
        spec.suspension.clone(),
        Mass(spec.mass),
        AngularInertia::from_shape(&Cuboid::new(ex, ey, ez), spec.mass),
        NoAutoMass,
        NoAutoAngularInertia,
        NoAutoCenterOfMass,
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
        // propagates `HIDDEN` to every descendant mesh, so the gunner optic (camera parked at the
        // gun pivot, inside the mantlet) sees no own-tank geometry — no near-plane clipping.
        Visibility::Inherited,
    ));

    // Record what the walk found, to check against the required contract afterwards.
    let mut found: HashSet<&'static str> = HashSet::new();
    let mut left_wheels = 0u32;
    let mut right_wheels = 0u32;
    let mut colliders = 0u32;
    // Roadwheel nodes collected during the walk; sorted by name after it so each wheel's
    // `WheelIndex` into `TankSim::anchors` is walk-order-independent.
    let mut wheel_nodes: Vec<(String, Entity, TrackSide)> = Vec::new();
    // Rig-node handles captured for this tank's `Rig` (built after the contract check below).
    let mut hull_node = None;
    let mut turret_node = None;
    let mut gun_node = None;
    let mut muzzle_node = None;
    // Node-name → entity index, built in the walk so the spec-driven binding below (servos) can
    // resolve a declared node by name instead of matching it inline.
    let mut index: HashMap<String, Entity> = HashMap::new();

    for entity in children.iter_descendants(ready.entity) {
        // Skip render-primitive entities (`GltfMaterialName`, named `{mesh}.{material}`): we bind
        // structure to authored nodes only, not the loader's per-material render leaves.
        if primitives.contains(entity) {
            continue;
        }
        // Most descendants are unnamed mesh nodes — skip them quietly.
        let Ok(name) = names.get(entity) else {
            continue;
        };
        let id = entity;
        index.insert(name.to_string(), id);
        let mut entity = commands.entity(entity);
        match name.as_str() {
            // Servos are bound from `spec.servos` after the walk (resolved via `index`), not here —
            // their nodes carry no fixed names, so the spec is the source of truth for which they are.
            "Hull" => {
                found.insert("Hull");
                hull_node = Some(id);
                entity.insert(Hull);
            }
            // Weapon parts (muzzle, recoiling barrel) are bound from `spec.weapons` after the walk,
            // resolved via `index` — node names live in the weapon entry, not here.
            "Center_Of_Mass" => {
                found.insert("Center_Of_Mass");
                entity.insert(CenterOfMassAnchor);
            }
            // Roadwheels (Wheel_L_0.., Wheel_R_0..): tag the track side. The suspension ray is
            // cast by `apply_suspension` itself each tick (`SpatialQuery`, tick-truth wheel pose)
            // — deliberately NOT a `RayCaster` component: its `RayHits` refresh after the step,
            // which fed rollback replays stale hits (and its position-update system was the
            // residual bind-window NaN vector).
            s if roadwheel_side(s).is_some() => {
                let side = roadwheel_side(s).expect("guard matched");
                match side {
                    TrackSide::Left => left_wheels += 1,
                    TrackSide::Right => right_wheels += 1,
                }
                // Collected, not inserted here: the wheel's `WheelIndex` into `TankSim::anchors`
                // is its position in the name-sorted list, assigned after the walk.
                wheel_nodes.push((name.to_string(), id, side));
            }
            // Collision proxies (`*_Collider`): a convex-hull collider on the Vehicle layer, hidden
            // (it's physics, not rendering — ADR-0008). Collision-only: it contributes no mass (the
            // hull authors its own, see above). The glTF loader puts the mesh on a child primitive,
            // so build over the node's descendants.
            s if s.ends_with("_Collider") => {
                colliders += 1;
                entity.insert((
                    ColliderConstructorHierarchy::new(ColliderConstructor::ConvexHullFromMesh)
                        .with_default_layers(CollisionLayers::new(
                            [Layer::Vehicle],
                            LayerMask::ALL,
                        )),
                    Visibility::Hidden,
                ));
            }
            // Ballistic volumes are bound from `spec.volumes` after the walk (resolved via `index`),
            // like servos and weapons — a node is a volume iff it's a declared key, so the spec is the
            // source of truth, not an inline name match. (An authored `*_Ballistic` node with no spec
            // entry is caught by the CI bind-contract test, so there's no runtime drift scan here.)
            _ => {}
        }
    }

    // Servos are spec-driven: each `spec.servos` entry resolves to its node via `index` and gets the
    // servo bundle + its role (the aim pass drives *every* servo by role; no chain concept). The
    // `Turret`/`Gun` markers are NOT set here — with multiple mounts of a role they'd be ambiguous;
    // they go on the primary weapon's chain below. A declared servo with no matching node is fatal.
    // Sorted by node name: the entry's position is its `ServoIndex` into `TankSim::servos`, and a
    // HashMap's iteration order must never decide an index both wire ends derive.
    let mut missing_servos: Vec<&str> = Vec::new();
    let mut yaw_servos: HashSet<Entity> = HashSet::new();
    let mut servo_entries: Vec<_> = spec.servos.iter().collect();
    servo_entries.sort_by_key(|(node, _)| node.as_str());
    for (slot, (node, servo)) in servo_entries.into_iter().enumerate() {
        let Some(&id) = index.get(node.as_str()) else {
            missing_servos.push(node.as_str());
            continue;
        };
        commands.entity(id).insert((
            servo.clone(),
            ServoCommand::default(),
            ServoIndex(slot),
            TankRoot(ready.entity),
            servo.role,
        ));
        if servo.role == ServoRole::Yaw {
            yaw_servos.insert(id);
        }
    }

    // Weapons are spec-driven: resolve each weapon's muzzle (+ optional recoiling barrel) via
    // `index`, tag the nodes, and attach the `Weapon` config the shooting systems read. One weapon
    // for now (the main gun) — the coax + hull MG join with the multi-weapon increment — so the
    // rig's `muzzle`/`barrel` are this single weapon's. A weapon node that doesn't resolve is fatal.
    // Sorted for the same reason as servos: position = `WeaponIndex` into `TankSim::weapons`.
    let mut missing_weapon_nodes: Vec<&str> = Vec::new();
    let mut weapon_entries: Vec<_> = spec.weapons.iter().collect();
    weapon_entries.sort_by_key(|(name, _)| name.as_str());
    for (slot, (weapon_name, weapon)) in weapon_entries.into_iter().enumerate() {
        let Some(&muzzle) = index.get(weapon.muzzle.as_str()) else {
            missing_weapon_nodes.push(weapon.muzzle.as_str());
            continue;
        };
        let barrel = match &weapon.barrel {
            Some(name) => match index.get(name.as_str()) {
                Some(&e) => Some(e),
                None => {
                    missing_weapon_nodes.push(name.as_str());
                    None
                }
            },
            None => None,
        };
        commands.entity(muzzle).insert((
            Muzzle,
            TankRoot(ready.entity),
            WeaponIndex(slot),
            Weapon {
                name: weapon_name.clone(),
                speed: weapon.speed,
                caliber: weapon.caliber,
                mass: weapon.mass,
                reload: weapon.reload,
                recoil: weapon.recoil.clone(),
                barrel,
                fire: weapon.fire.clone(),
                load: weapon.load.clone(),
                trigger: weapon.trigger,
            },
        ));
        if let Some(barrel) = barrel {
            // The barrel actuates from the same weapon slot (recoil state) — see `apply_recoil`.
            commands
                .entity(barrel)
                .insert((GunBarrel, WeaponIndex(slot), TankRoot(ready.entity)));
        }
        // The single `Primary` weapon supplies the rig's main bore (`Rig.muzzle`) — what the bore
        // HUD reads and LMB fires. The traverse/elevation handles come from the gunner *view* below,
        // not from the weapon; trigger never speaks to aiming.
        if weapon.trigger == Trigger::Primary {
            muzzle_node = Some(muzzle);
        }
    }

    // Ballistic volumes are spec-driven too: each `spec.volumes` entry resolves to its node via
    // `index` and gets the volume bundle (design `armor-penetration-and-damage.md` §12; composition,
    // not a `kind` enum). `material_factor` (shell-resistance per metre) every volume has; optional
    // `hp` makes it a damageable component. The `Armor_/Module_/...` name prefix is documentation
    // only, never parsed — resistance and role both come from data, so a steel barrel module resists
    // like steel yet still takes damage. Bound as a query-only trimesh collider on the `Armor` layer
    // (watertight solids may be concave — fine for a raycast, unlike the dynamic physics proxy,
    // ADR-0008) with NO collision response (`filters = NONE`), so it never perturbs the body; hidden
    // like `*_Collider` (the march raycasts it, the sandbox visualizes it). A declared volume with no
    // matching node is fatal — the spec↔model contract (the reverse is the CI bind-contract test).
    let mut missing_volume_nodes: Vec<&str> = Vec::new();
    for (name, volume) in &spec.volumes {
        let Some(&id) = index.get(name.as_str()) else {
            missing_volume_nodes.push(name.as_str());
            continue;
        };
        assert!(
            volume.hp.is_some()
                || (volume.crew.is_none() && !volume.ammo && volume.function.is_none()),
            "tank volume `{name}` declares a consequence facet but has no hp"
        );
        let mut entity = commands.entity(id);
        entity.insert((
            Visibility::Hidden,
            ColliderConstructorHierarchy::new(ColliderConstructor::TrimeshFromMesh)
                .with_default_layers(CollisionLayers::new([Layer::Armor], LayerMask::NONE)),
            BallisticVolume {
                material_factor: volume.material_factor,
            },
            VolumeOf(ready.entity),
        ));
        if let Some(crew) = volume.crew {
            // Seat role + its native occupant (topology B): `home == seat` at bind, so competence is
            // 1.0 until a backfill swap moves an occupant to a foreign seat.
            entity.insert((crew, Crewman { home: crew }));
        }
        if volume.ammo {
            entity.insert(Ammo);
        }
        if let Some(function) = volume.function {
            entity.insert(function);
        }
        match volume.hp {
            // Damageable (module/crew/ammo): an HP pool the march depletes (transit/spall/shock). The
            // consequences of HP→0 (§§7–8) are a later increment.
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

    // The gunner's chain feeds the rig's `turret`/`gun` (optic, camera, launched-turret). It's the
    // gunner view's node (its Pitch servo) + the Yaw servo above it — declared, so the binder never
    // guesses which of several yaw/pitch mounts is the main one. Tagged `Turret`/`Gun` for the
    // queries that still address the main mount specifically (the aim pass instead drives *every*
    // servo by `ServoRole`, chain-agnostic).
    if let Some(view) = spec.views.get(&ViewKind::Gunner)
        && let Some(&pitch) = index.get(view.node.as_str())
    {
        commands.entity(pitch).insert(Gun);
        gun_node = Some(pitch);
        if let Some(yaw) = first_ancestor_in(pitch, &yaw_servos, &parents) {
            commands.entity(yaw).insert(Turret);
            turret_node = Some(yaw);
        }
    }

    // Fixed-name structural singletons, plus ≥1 collider (else the body is massless → NaN) and ≥1
    // roadwheel per side (else a track has no support/thrust). A real Tiger has many wheels; the sim
    // only needs one contact station per side to be non-degenerate, so the contract is per-side
    // presence, not a fixed count. Servos and weapons are contracted separately (they're spec-
    // declared, not fixed-named).
    const REQUIRED: [&str; 2] = ["Hull", "Center_Of_Mass"];
    let mut missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|n| !found.contains(n))
        .collect();
    missing.extend(missing_servos);
    missing.extend(missing_weapon_nodes);
    missing.extend(missing_volume_nodes);
    if muzzle_node.is_none() {
        missing.push("<a Primary weapon>");
    }
    if turret_node.is_none() {
        missing.push("<a Yaw servo above the Primary weapon's muzzle>");
    }
    if gun_node.is_none() {
        missing.push("<a Pitch servo above the Primary weapon's muzzle>");
    }
    if colliders == 0 {
        missing.push("*_Collider");
    }
    if left_wheels == 0 {
        missing.push("Wheel_L*");
    }
    if right_wheels == 0 {
        missing.push("Wheel_R*");
    }
    assert!(
        missing.is_empty(),
        "tank model is missing required rig nodes: {missing:?}"
    );

    // Wheels get their slot in name-sorted order (see `wheel_nodes` above).
    wheel_nodes.sort_by(|a, b| a.0.cmp(&b.0));
    let wheel_count = wheel_nodes.len();
    for (slot, (_, id, side)) in wheel_nodes.into_iter().enumerate() {
        commands
            .entity(id)
            .insert((Roadwheel { side }, WheelIndex(slot)));
    }

    // The contract check above guarantees every rig node was found, so these unwraps can't fire.
    // Record them so control systems can address *this* tank's parts by entity (`rig.gun`). The
    // recoiling barrel isn't a rig field — it rides each `Weapon` (`weapon.barrel`).
    // `TankSim` is sized to the bound rig: every slot exists from birth (reloads start 0.0 =
    // loaded; servo rest quats are captured by `drive_servos`' first step).
    commands.entity(ready.entity).insert((
        Rig {
            hull: hull_node.unwrap(),
            turret: turret_node.unwrap(),
            gun: gun_node.unwrap(),
            muzzle: muzzle_node.unwrap(),
        },
        TankSim {
            servos: vec![ServoState::default(); spec.servos.len()],
            weapons: vec![WeaponState::default(); spec.weapons.len()],
            anchors: vec![None; wheel_count],
        },
    ));
}

/// The servo's absolute pose write: rest · R(axis, angle) — shared by the three writers (truth
/// restore, mechanism step, render blend), so there is exactly one formula.
fn write_servo_pose(transform: &mut Transform, spec: &ServoSpec, state: &ServoState, angle: f32) {
    transform.rotation = state.rest * Quat::from_axis_angle(spec.role.axis(), angle);
}

/// Top of each fixed tick: re-assert every servo node's `Transform` to the mechanism's current
/// angle, undoing `interpolate_servos`' render-time lerp — sim readers inside this tick
/// (`rig_world_pose` chains, the child-collider sync) must see tick state. Cheap: a quat multiply
/// per servo. Reads the root-resident state (`TankSim::servos`) — during a rollback replay this
/// is what re-derives the node transforms from the RESTORED state each tick.
fn restore_servo_truth(
    mut q: Query<(&mut Transform, &ServoSpec, &ServoIndex, &TankRoot)>,
    sims: Query<&TankSim>,
) {
    for (mut transform, spec, slot, root) in &mut q {
        let Some(state) = sims.get(root.0).ok().and_then(|sim| sim.servos.get(slot.0)) else {
            continue;
        };
        if state.captured {
            write_servo_pose(&mut transform, spec, state, state.current);
        }
    }
}

fn drive_servos(
    mut q: Query<(
        &mut Transform,
        &ServoSpec,
        &ServoCommand,
        &ServoIndex,
        &TankRoot,
    )>,
    mut sims: Query<&mut TankSim>,
    tanks: Query<&TankVolumes>,
    facets: Query<VolumeFacets>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut transform, spec, command, slot, root) in &mut q {
        let Ok(mut sim) = sims.get_mut(root.0) else {
            continue;
        };
        let Some(state) = sim.servos.get_mut(slot.0) else {
            continue;
        };
        // Capture the node's authored rest rotation once, so the pose write can be an *absolute*
        // rotation (`rest · R(axis, angle)`) instead of accumulating deltas (no round-off).
        if !state.captured {
            state.rest = transform.rotation;
            state.captured = true;
        }

        // This tick's blend-from for the render interpolation.
        state.previous = state.current;

        // Slew gate (design §7b): scale max speed by the requirement's effectiveness, so a dead
        // operator (or, later, a damaged traverse motor) freezes or slows this mount. 0 → no slew.
        let slew = tanks
            .get(root.0)
            .map(|tv| evaluate(&spec.requires, &part_qualities(tv, &facets)))
            .unwrap_or(0.0);

        // `ServoSpec` authors angles in degrees (the human authoring unit); the runtime — the
        // command, the state, and the slew maths below — is radians. Convert the spec's angular
        // quantities once here, at the spec→runtime boundary.
        let max_speed = spec.max_speed.to_radians() * slew;
        let accel = spec.accel.to_radians();
        let travel = match spec.travel {
            Travel::Limited { min, max } => Travel::Limited {
                min: min.to_radians(),
                max: max.to_radians(),
            },
            Travel::Continuous => Travel::Continuous,
        };

        let error = match travel {
            Travel::Limited { .. } => command.target - state.current,
            Travel::Continuous => shortest_angle(command.target - state.current),
        };

        // Land-exactly: if this step's motion would reach or overshoot the target, snap to it and
        // stop. Without this, the sqrt envelope's `v·dt` exceeds `|error|` just before arrival →
        // overshoot → sign flip → a tight limit cycle (the residual "buzz" at settle). Snapping
        // also kills the discrete-cycle hypothesis for the gunner-optic vibration.
        let step = state.velocity * dt;
        if step.abs() >= error.abs() && error.abs() > 0.0 {
            state.current += error;
            state.velocity = 0.0;
        } else {
            // Speed that still allows braking to rest exactly at the target — the sqrt velocity
            // envelope, `v = √(2a·|error|)` — capped at max_speed; slew the actual velocity toward
            // it within the accel limit. Same trapezoidal motion (accelerate, cruise, decelerate),
            // but it brakes *smoothly onto* the target.
            let target_speed = (2.0 * accel * error.abs()).sqrt().min(max_speed);
            let desired_velocity = error.signum() * target_speed;
            let dv = accel * dt;
            state.velocity += (desired_velocity - state.velocity).clamp(-dv, dv);

            state.current += state.velocity * dt;
            if let Travel::Limited { min, max } = travel {
                state.current = state.current.clamp(min, max);
            }
        }

        // Settle deadband scaled to what one step can resolve (`accel·dt²` ≈ the smallest move the
        // servo can make before braking), so it's reachable per-step rather than a fixed band that
        // may sit below the discretization floor and never trigger.
        let settle = accel * dt * dt;
        if error.abs() < settle && state.velocity.abs() < accel * dt {
            state.velocity = 0.0;
            if let Travel::Limited { min, max } = travel {
                state.current = command.target.clamp(min, max);
            }
        }

        // Publish the freshly-stepped angle as this tick's node pose — the value avian's
        // child-collider sync and any later-in-tick reader consume (`interpolate_servos`
        // re-blends it for rendering after the fixed loop).
        let angle = state.current;
        write_servo_pose(&mut transform, spec, state, angle);
    }
}

/// The render half of the fixed-clock servo split: blend last tick's angle to this tick's by the
/// fixed clock's overstep and write the node's `Transform` — smooth mechanism motion at any frame
/// rate, exactly how Avian renders the hull between physics ticks. Along the shortest arc, so a
/// continuous mount's ±π wrap doesn't spin the long way round.
fn interpolate_servos(
    time: Res<Time<Fixed>>,
    mut servos: Query<(&mut Transform, &ServoSpec, &ServoIndex, &TankRoot)>,
    sims: Query<&TankSim>,
) {
    let alpha = time.overstep_fraction();
    for (mut transform, spec, slot, root) in &mut servos {
        let Some(state) = sims.get(root.0).ok().and_then(|sim| sim.servos.get(slot.0)) else {
            continue;
        };
        if !state.captured {
            continue;
        }
        let angle = state.previous + shortest_angle(state.current - state.previous) * alpha;
        write_servo_pose(&mut transform, spec, state, angle);
    }
}

/// Wrap an angle difference into [-PI, PI] for shortest-path rotation.
pub(crate) fn shortest_angle(diff: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (diff + PI).rem_euclid(TAU) - PI
}
