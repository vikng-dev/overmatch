//! The tank: its rig (structural markers bound by node name), the kinematic `Servo` motor
//! for the turret/gun, and the sim-skeleton spawner. The tank declares *structure*; features
//! (aim, shooting) attach their own behavior to these markers reactively.
//!
//! **Sim/view split, phase 1** (design `sim-view-split-and-tank-bake.md` §8 step 1): the sim
//! body — servo frames, wheel stations, collision hulls, armor volumes, `Rig`/`TankSim` — is
//! spawned *synchronously* from the extracted [`crate::bake::TankGeometry`], never from the
//! instantiated glb scene. The scene is a **view**: it attaches whenever it loads and only
//! renders ([`bind_tank_view`]). This is what makes the tier-2 rule structural: every
//! rollback-registered component is constructible at spawn, from data, so there is no bind
//! window for netcode to care about.

use std::collections::{HashMap, HashSet};

use avian3d::physics_transform::ApplyPosToTransform;
use avian3d::prelude::{
    AngularInertia, CenterOfMass, Collider, CollisionLayers, LayerMask, Mass, NoAutoAngularInertia,
    NoAutoCenterOfMass, NoAutoMass, RigidBody, TrimeshFlags,
};
use bevy::asset::LoadState;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use serde::Deserialize;

use crate::Layer;
use crate::bake::{ExtractedTankGeometry, TankGeometry};
use crate::ballistics::{ArmorVolume, BallisticVolume, ComponentHealth, ComponentVolume};
use crate::damage::{
    Ammo, Crewman, Requirement, TankCapabilities, TankVolumes, VolumeFacets, VolumeOf, evaluate,
    part_qualities,
};
use crate::shooting::RecoilParams;
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
/// its `On<Add, Tank>` observers must fire alongside the local spawn bundle, not at
/// replication-receive time (see `net::protocol::NetTank` for the wire-side identity marker and
/// the measured NaN regression that rule comes from).
#[derive(Component)]
pub struct Tank;

/// Marks the one tank the player is currently commanding. Exactly one tank carries this at a time;
/// the swap input ([`swap_controlled_tank`]) moves it. The *control* systems (drive input, aiming,
/// cameras, shooting, gunner sight) scope to this marker so they act on the player's tank alone;
/// everything tank-agnostic (suspension support, ballistics, damage) ignores it and runs for every
/// tank. `Controlled` answers *which* tank; [`Rig`] answers *where its parts are*.
#[derive(Component)]
pub struct Controlled;

/// Resolved handles to a tank's rig nodes, assigned when the sim skeleton spawns
/// ([`spawn_tank_sim`]). Lets a control system reach *this* tank's specific gun/turret/muzzle by
/// entity (`rig.gun`) instead of `query.single()`, which silently assumed a single tank in the
/// world. Lives on the root, so it shares the tank's lifetime — the handles can't dangle (the
/// parts despawn with the root they're parented to). The spawner's contract check guarantees
/// every field is present by the time `Rig` is inserted.
#[derive(Component)]
pub struct Rig {
    pub hull: Entity,
    pub turret: Entity,
    pub gun: Entity,
    pub muzzle: Entity,
}

/// Sweep a cooked-off turret when its tank root despawns. On cookoff the turret is detached from the
/// rig (`ChildOf` removed, remade as a free `LaunchedTurret` body — see
/// `damage::launch_turrets_on_cookoff` / `net::protocol::apply_launched_turret_pose`), so it is NOT
/// a descendant of the root: a recursive root despawn misses it, leaking one launched turret per
/// death. `Rig` lives ONLY on the tank root and is removed as the root despawns, so `On<Remove,
/// Rig>` is the "root is going away" signal on BOTH ends of the wire — the net server despawning a
/// respawning bot, and the net client recursively despawning the root when that despawn replicates.
/// The removed `Rig` is still readable inside the observer, so we reach its captured `turret` handle
/// and `try_despawn` it: a silent no-op when the turret was still an attached child (crew-loss
/// death, already swept by the recursive despawn) or is otherwise gone, so one branch covers both
/// death paths on both ends. Mounted in [`sim_plugin`], which the net client and server both pull in
/// via `SimPlugin`; harmless in single-player, where tank roots never despawn.
fn sweep_launched_turret_on_root_despawn(
    remove: On<Remove, Rig>,
    rigs: Query<&Rig>,
    mut commands: Commands,
) {
    if let Ok(rig) = rigs.get(remove.entity) {
        commands.entity(rig.turret).try_despawn();
    }
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

/// Back-link from a rig part (a servo) to its tank's root entity, set at spawn. Lets a per-tank
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

impl ServoSpec {
    /// This servo's authored travel limits converted to **radians** (the runtime unit), or `None`
    /// for a `Continuous` mount (unlimited traverse). The authoring unit is degrees; `drive_servos`
    /// clamps the live `current` (the lay) to this same window every tick. The gunner sight reuses
    /// it to keep the aim intent inside what the mount can mechanically reach, so a cursor parked
    /// past the elevation stop can't peg the reticle at the optic rim forever (the gun would
    /// saturate at its limit and the lead never close).
    pub fn travel_limits(&self) -> Option<(f32, f32)> {
        match self.travel {
            Travel::Limited { min, max } => Some((min.to_radians(), max.to_radians())),
            Travel::Continuous => None,
        }
    }
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
/// `drive_servos`; never authored, never shared. Pure per-tick state: the node's authored rest
/// pose is *config*, spawned from data as [`ServoRest`] on the servo node (it used to be lazily
/// captured in here, which is exactly the state the ConfirmedHistory-seed bug corrupted).
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
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ServoState {
    current: f32,
    /// The angle at the previous fixed tick — the render interpolation's blend-from.
    previous: f32,
    velocity: f32,
}

/// A servo node's authored rest rotation — the pose at `current = 0` — spawned from the extracted
/// geometry so the pose write can be an *absolute* rotation (`rest · R(axis, angle)`) instead of
/// accumulating deltas (no round-off). Config, not state: it never changes after spawn, so it
/// lives on the servo node, outside the rollback-registered [`TankSim`].
#[derive(Component, Clone, Copy)]
pub struct ServoRest(pub Quat);

impl ServoState {
    /// The servo's current angle (radians, parent-local) — its live mechanism position. Read by the
    /// gunner sight to clamp how far the aim intent may lead the gun (the on-screen margin).
    pub fn current(&self) -> f32 {
        self.current
    }

    /// The servo's full carried state, in a fixed field order — the canonical input to the
    /// per-tick divergence hash (`trace::hash_tank_state`). All three fields are sim truth that
    /// rolls back and replays, so all three must enter the hash; the order here IS the hashed order.
    /// The fields stay private (`drive_servos` is their only writer); this is a read-only view for
    /// the passive recorder.
    pub(crate) fn hash_fields(&self) -> [f32; 3] {
        [self.current, self.previous, self.velocity]
    }

    /// Test-only constructor: the divergence-hash unit tests (`trace.rs`) need a non-default servo
    /// to prove a servo-field flip localizes to the `hsrv` sub-hash. The fields stay private in
    /// production code — `drive_servos` remains their only writer.
    #[cfg(test)]
    pub(crate) fn test_new(current: f32, previous: f32, velocity: f32) -> Self {
        Self {
            current,
            previous,
            velocity,
        }
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
/// reload/recoil, and the wheels' brush anchors — indexed by the spawn-time [`ServoIndex`]/
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
/// Index assignment is sorted-by-name at spawn ([`spawn_tank_sim`]) — the spec's servo/weapon
/// maps are `HashMap`s, and their iteration order must never leak into indices that client and
/// server both derive.
#[derive(Component, Clone, PartialEq, Debug, Default)]
pub struct TankSim {
    pub servos: Vec<ServoState>,
    pub weapons: Vec<WeaponState>,
    /// Per-wheel brush anchor: the world point the contact "gripped" while near rest. `Some` =
    /// static friction holds the tank there; `None` = slipping (kinetic) or airborne.
    pub anchors: Vec<Option<Vec3>>,
}

/// This servo's slot in its tank's [`TankSim::servos`], assigned at spawn in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct ServoIndex(pub usize);

/// This weapon's slot in [`TankSim::weapons`] — on the muzzle AND the recoiling barrel (both
/// actuate from the same weapon state), assigned at spawn in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct WeaponIndex(pub usize);

/// This roadwheel's slot in [`TankSim::anchors`], assigned at spawn in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct WheelIndex(pub usize);

/// The sim skeleton's part table, on the tank root: node name → sim part entity, for every part
/// [`spawn_tank_sim`] spawned. The **name-keyed join between sim and view** (design §6C): the
/// view binder ([`bind_tank_view`]) resolves each instantiated glb node against this map to tag
/// it [`ViewOf`] its sim part. Phase 2's artifact part table is this map, serialized.
#[derive(Component)]
pub struct SimParts(pub HashMap<String, Entity>);

/// A glb view node's link to the sim part of the same name, inserted by [`bind_tank_view`]. The
/// name-keyed sim/view join, view→sim direction: the render writers (`interpolate_servos`)
/// resolve their sim source through it, and the sandbox's volume painter reads the sim part's
/// volume role through it.
#[derive(Component, Clone, Copy)]
pub struct ViewOf(pub Entity);

/// The sim→view back-link, inserted on each sim part by [`bind_tank_view`] when its view node
/// attaches: the part table in entity form, for consumers that start from a sim entity —
/// `sync_view_barrels` (the recoil pose copy), the cook-off view detach, and every render
/// reader that must follow the smoothed view pose (gunner camera, bore dot, HP labels).
#[derive(Component, Clone, Copy)]
pub struct ViewNode(pub Entity);

impl ViewNode {
    /// THE render-reader fallback rule, single-sourced: a sim part's render-side node is its
    /// attached view node, or the sim part itself before the scene attaches (cosmetic — the sim
    /// pose steps at tick rate, but nothing slews during the spawn pop-in). Degrades per part:
    /// a partially-instantiated scene only falls back where the join is actually missing.
    pub fn resolve(view: Option<&ViewNode>, sim: Entity) -> Entity {
        view.map_or(sim, |view| view.0)
    }
}

/// Marks a glb view node whose sim part is a servo frame — `interpolate_servos`' write set (the
/// render blend targets VIEW nodes; sim node transforms are pure tick truth since step 2).
#[derive(Component)]
pub struct ViewServo;

/// The AUTHORED local `Transform` of a child collider (collision proxy / armor volume): sim
/// structure, spawned once from extracted geometry and never legitimately rewritten at runtime —
/// the collider's world pose is *derived* (root `Position` ∘ propagated `ColliderTransform`),
/// so its local `Transform` is a constant of the rig, not state.
///
/// **ADR-0015 Layer-2 scaffolding — the collider attachment-poisoning fix.** Upstream defect
/// (lightyear_avian3d 0.28, upstream report candidate #3): `AvianReplicationMode::Position`
/// registers avian's `ApplyPosToTransform` as a REQUIRED component of `Position`/`Rotation`
/// (plugin.rs:620-623) so the Position→Transform sync also covers Interpolated roots — but child
/// colliders carry `Position`/`Rotation` too (avian collider backend required components), so the
/// blanket requirement drags them into `position_to_transform`'s write set
/// (avian3d `physics_transform/mod.rs:254-257`). That system then rewrites the child's LOCAL
/// `Transform` as its sim-world `Position` `reparented_to` the parent bone's `GlobalTransform`
/// — which is render-blended (FrameInterpolation/VisualCorrection/render-error offset) and one
/// `TransformSystems::Propagate` stale. Every frame deposits the sim-vs-render difference into
/// the local `Transform`; `propagate_collider_transforms` folds it into `ColliderTransform` next
/// tick and the collider's world pose ratchets away from the rig (measured: 2–13 cm/tick during
/// rollback storms, hull proxy 2.8 m above the root; the resulting hc=0 storms self-sustain).
/// The ADR-0014 leak class — render state leaking into sim — introduced upstream.
///
/// The fix is [`shield_authored_collider_transform`]: strip `ApplyPosToTransform` from these
/// entities, excising the write instead of undoing it. This component is the scope marker (and
/// keeps the authored value on record for tripwires/re-assertion should upstream semantics
/// change). Removal condition: upstream excludes child colliders (non-`RigidBody` entities with
/// `ColliderOf`) from the blanket `ApplyPosToTransform` requirement, or gives the sync an
/// opt-out per entity.
#[derive(Component)]
pub struct AuthoredLocalTransform(pub Transform);

/// Strip [`ApplyPosToTransform`] the moment anything inserts it on an authored child collider —
/// see [`AuthoredLocalTransform`] for the full defect. An observer, not a spawn-site `remove`,
/// so the shield is self-healing: required-component insertion happens at every `Position`/
/// `Rotation`/`Collider` (re-)insert, and any future re-insert would silently re-arm the
/// poisoning write. Runs identically on client and server (registered in [`sim_plugin`], mounted
/// by every composition root): the deposit is render-sized on the client but exists on the
/// server too (the reparent target is one `Propagate` stale even without render blending), and
/// the sim-affecting symmetry rule forbids fixing sim-visible state on one end only. In non-net
/// compositions `ApplyPosToTransform` is never required-inserted (avian's own
/// `position_to_transform` filter never matched these entities — no `RigidBody`), so this never
/// fires there.
fn shield_authored_collider_transform(
    add: On<Add, ApplyPosToTransform>,
    authored: Query<&AuthoredLocalTransform, Without<RigidBody>>,
    mut commands: Commands,
) {
    if let Ok(authored) = authored.get(add.entity) {
        // Strip the write-set membership AND re-assert the authored value: the strip lands in
        // the same command flush as the insert that armed it (no system runs in between), so
        // the re-assert is normally a no-op — it exists so that even a re-arm that somehow
        // followed a deposit leaves the attachment at its authored constant. try_* variants:
        // a same-flush despawn (spawn-then-abort, recursive parent despawn) must skip the
        // already-gone proxy, not crash the session at command application.
        commands
            .entity(add.entity)
            .try_remove::<ApplyPosToTransform>()
            .try_insert(authored.0);
    }
}

/// The mirror trigger: [`shield_authored_collider_transform`] only fires when the authored
/// marker is already present at `ApplyPosToTransform`-insertion time, so a spawn site that
/// splits the bundle (collider first, marker in a later command) would arm the poisoning write
/// with no future `Add` to heal it. Watching the marker's own insertion closes that direction —
/// the shield is order-independent: whichever of the two components lands second completes it.
fn shield_late_authored_marker(
    add: On<Add, AuthoredLocalTransform>,
    armed: Query<&AuthoredLocalTransform, (With<ApplyPosToTransform>, Without<RigidBody>)>,
    mut commands: Commands,
) {
    if let Ok(authored) = armed.get(add.entity) {
        commands
            .entity(add.entity)
            .try_remove::<ApplyPosToTransform>()
            .try_insert(authored.0);
    }
}

/// The authored attachment, stated once: the spawned local `Transform` and the
/// [`AuthoredLocalTransform`] record the shield re-asserts must never disagree — a stale copy
/// would silently move the collider to the wrong pose on a later observer re-fire (wrong
/// contact / wrong damage geometry, the exact symptom class the shield exists to prevent).
fn authored_attachment(transform: Transform) -> impl Bundle {
    (transform, AuthoredLocalTransform(transform))
}

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
    // The attachment-poisoning shield (see `AuthoredLocalTransform`): child colliders' local
    // transforms are authored constants, never derived from world state. Two observers make it
    // order-independent — whichever of marker/`ApplyPosToTransform` lands second completes it.
    app.add_observer(shield_authored_collider_transform);
    app.add_observer(shield_late_authored_marker);
    app.add_observer(sweep_launched_turret_on_root_despawn);
    app
        // The servo mechanism steps on the fixed clock (sim truth — the muzzle pose decides where
        // shells go), *after* `GameplaySet` so `drive_aim_servos` has written this tick's targets.
        // The sim node's `Transform` is pure TICK TRUTH: `restore_servo_truth` re-derives it from
        // `TankSim` at tick start (what makes rollback replays compose restored state) and
        // `drive_servos` writes the freshly-stepped angle at tick end, so every sim reader
        // (`fire`'s muzzle chain via `rig_world_pose`, avian's child-collider sync, every tick of
        // a rollback replay) sees the mechanism's state. Render smoothing lives on the VIEW tree:
        // between fixed runs `interpolate_servos` (Update) blends the last two tick angles by the
        // clock's overstep into the view nodes — smooth at any frame rate, same split Avian uses
        // for the hull, without ever touching a sim transform.
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
        )
        .add_plugins(view_attach_plugin);
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
/// front, and a tank is spawned only once both are ready — [`spawn_tank_sim`] reads the loaded
/// spec, and preloading the scene keeps the *view* pop-in to ~a frame. Since phase 1 of the
/// sim/view split the scene is presentation only: the sim body spawns synchronously from the
/// extracted geometry, so a late scene is a cosmetic wait, not a sim hazard. (The scene stays a
/// gate here because it is still the extractor's source and the shadow harness's subject; phase 2
/// drops the server's dependency on it entirely — design §8 step 3.) Kicked off once at startup
/// on every side; shared with the networking layer (`net::rig`/`net::client`/`net::server`),
/// which spawns tanks against the same dependency.
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

/// The sim spawner's data dependencies, resolved as ONE gate every spawn path shares (SP,
/// server connect-spawn, client replicated-attach, sandbox target): the extracted geometry
/// (a Startup resource wherever `bake::plugin` is mounted) + the loaded spec. `get` returns
/// `None` while anything is still pending — callers simply try again next frame — so the
/// wait-vs-spawn behavior can't drift between the paths.
#[derive(SystemParam)]
pub(crate) struct TankSimSource<'w> {
    geometry: Option<Res<'w, ExtractedTankGeometry>>,
    specs: Res<'w, Assets<TankSpec>>,
}

impl TankSimSource<'_> {
    pub(crate) fn get(&self, spec: &Handle<TankSpec>) -> Option<(&TankGeometry, &TankSpec)> {
        Some((&self.geometry.as_ref()?.0, self.specs.get(spec)?))
    }
}

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
    source: TankSimSource,
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
    let Some((geometry, spec)) = source.get(&pending.spec) else {
        return;
    };
    // Two tanks, both player-owned: `Tab` swaps which one is `Controlled`. The first spawns
    // controlled; the second sits until you swap into it (design: the antagonist/auto-aim
    // lands in Chunk 2). Both are dynamic bodies — per-tank suspension holds each up; only
    // the controlled one takes drive input.
    spawn_tank(
        &mut commands,
        &pending,
        geometry,
        spec,
        Transform::from_xyz(10.0, 2.0, 5.0).with_rotation(Quat::from_rotation_z(0.7)),
        "Tiger I (A)",
        true,
    );
    spawn_tank(
        &mut commands,
        &pending,
        geometry,
        spec,
        Transform::from_xyz(10.0, 2.0, -12.0),
        "Tiger I (B)",
        false,
    );
    commands.remove_resource::<PendingTankAssets>();
    next.set(AppState::Playing);
}

/// The spawn core every tank shares, whatever world it lives in: the Tiger scene as the **view**
/// (drives [`bind_tank_view`] when it instantiates — preloaded, so within ~a frame), the spec
/// handle, and the [`Tank`] marker. The sim body itself comes from [`spawn_tank_sim`], which
/// every spawn path calls in the same command batch — the tank is sim-complete before this
/// bundle's flush ends. SP ([`spawn_tank`]) adds a world pose + `RigidBody::Dynamic`; the
/// networked paths (`net::rig::net_tank_rig`) add the wire identity marker.
pub(crate) fn tank_rig(assets: &PendingTankAssets) -> impl Bundle {
    (
        WorldAssetRoot(assets.scene.clone()),
        TankSpecHandle(assets.spec.clone()),
        Tank,
    )
}

/// Spawn one Tiger at `transform`: the full sim skeleton synchronously from the extracted
/// geometry ([`spawn_tank_sim`]), the glb scene as its view. `controlled` seeds the player's
/// starting tank with the [`Controlled`] marker. The hull is a dynamic rigid body — Avian owns
/// its Transform (ADR-0005) — and is collider-complete in this very command batch.
fn spawn_tank(
    commands: &mut Commands,
    assets: &PendingTankAssets,
    geometry: &TankGeometry,
    spec: &TankSpec,
    transform: Transform,
    name: &str,
    controlled: bool,
) {
    let mut tank = commands.spawn((
        tank_rig(assets),
        transform,
        Name::new(name.to_string()),
        RigidBody::Dynamic,
    ));
    tank.observe(bind_tank_view);
    if controlled {
        tank.insert(Controlled);
    }
    let root = tank.id();
    spawn_tank_sim(commands, root, geometry, spec);
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

/// Walk up the extracted node tree from `index` (inclusive) and return the first node satisfying
/// `pred` — how the spawner resolves the yaw servo above the gunner's pitch node. Topology is
/// data: the runtime used to walk entity ancestors at bind for this.
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

/// Build a tank's ENTIRE sim body on `root`, synchronously, from the extracted geometry + the
/// already-loaded spec — servo frames, wheel stations, collision hulls, armor volumes,
/// `Rig`/`TankSim` — with no asset in the loop (sim/view split phase 1, design §8 step 1). Every
/// spawn path calls this in the same command batch as the root bundle, so the tank is
/// sim-complete (colliders included) the moment its first flush ends: SP at scenario spawn, the
/// server at connect-spawn, the client the moment the replicated root lands.
///
/// The skeleton mirrors the glb node tree — same names, same local transforms (bit-exact, step-0
/// shadow-proven), same topology — but only the chains the sim actually reads; visual-only nodes
/// stay out. The glb scene instantiates later as pure view ([`bind_tank_view`]).
///
/// This also enforces the rig contract (ADR-0002's "name = the contract"): every node the spec
/// declares must exist in the extracted geometry. Missing structure is an authoring bug — fatal
/// like a bad spec sheet (ADR-0010) — so we panic with the list of what's absent. Phase 2 turns
/// this same check into a build error in the offline tank compiler.
pub(crate) fn spawn_tank_sim(
    commands: &mut Commands,
    root: Entity,
    geometry: &TankGeometry,
    spec: &TankSpec,
) {
    // --- Resolve every spec-declared node against the extracted geometry. Sorted where the
    // entry's position becomes an index both wire ends derive (`ServoIndex`/`WeaponIndex`): a
    // HashMap's iteration order must never decide those.
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
    // Sorted for spawn-order determinism (no wire-shared index derives from volumes, but entity
    // and collider creation order must not vary run-to-run or across the wire on a whim of
    // HashMap iteration).
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

    // Roadwheels and collision proxies come pre-classified from the extractor by naming convention
    // (design §8 step 3: the runtime never parses node names for sim meaning). `roadwheels` is
    // already name-sorted — that order is each wheel's `WheelIndex` slot into `TankSim::anchors`,
    // an index both wire ends derive.
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

    // The full contract: declared nodes, structural singletons, the main-bore chain, ≥1 collider
    // (else the body is massless → NaN) and ≥1 roadwheel per side (else a track has no
    // support/thrust — per-side presence, not a fixed count).
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

    // --- Spawn the skeleton: every needed node plus its ancestor chain, so `rig_world_pose`
    // composes the exact transforms the scene walk used to provide (bit-equal by the step-0
    // shadow proof). Extraction order guarantees parents precede children.
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

    // --- Servos: the spec entry's bundle + its role (the aim pass drives *every* servo by role)
    // + the authored rest rotation from data (`ServoRest` — the lazy first-tick capture this
    // replaces is what the ConfirmedHistory bug corrupted).
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

    // --- Weapons: tag muzzle (+ optional recoiling barrel) and attach the `Weapon` config the
    // shooting systems read. The barrel actuates from the same weapon slot (recoil state); its
    // spring config (`RecoilParams`) is built here from the authored rest translation — data, not
    // a bind-time transform capture.
    for (slot, ((weapon_name, weapon), (muzzle_index, barrel_index))) in
        weapon_entries.iter().zip(&weapon_nodes).enumerate()
    {
        let muzzle = entity_at(muzzle_index.expect("contract checked"));
        let barrel = barrel_index.map(&entity_at);
        commands.entity(muzzle).insert((
            Muzzle,
            TankRoot(root),
            WeaponIndex(slot),
            Weapon {
                name: (*weapon_name).clone(),
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
        // A volume with no captured mesh is invisible to the penetration march — shells would
        // silently pass through it forever. The extractor only captures buffers under the
        // `*_Ballistic` naming rule, so a differently-suffixed declared volume must die HERE,
        // loudly, not at first shot (the golden test pins this for the shipped asset; this is
        // the runtime backstop for the next one).
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
            // Pre-check the cheap failure mode by name: `trimesh_with_config` panics on an empty
            // triangle list (an unindexed export — `read_indices` came back empty) without saying
            // which node broke.
            assert!(
                !triangles.is_empty(),
                "ballistic volume `{name}` has an unindexed or triangle-less mesh primitive"
            );
            commands.spawn((
                ChildOf(entity),
                // Authored attachment, shielded from lightyear_avian's Position→Transform sync
                // (see `AuthoredLocalTransform`): a poisoned armor volume is wrong damage
                // geometry, same class as a poisoned collision proxy.
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
                // Authored attachment, shielded from lightyear_avian's Position→Transform sync —
                // the probe-confirmed poisoning ratchet (see `AuthoredLocalTransform`).
                authored_attachment(Transform::IDENTITY),
                collider,
                CollisionLayers::new([Layer::Vehicle], LayerMask::ALL),
            ));
        }
    }

    // --- Wheels: suspension/drive contact stations, slotted in name-sorted order. The suspension
    // ray is cast by `apply_suspension` itself each tick (`SpatialQuery`, tick-truth wheel pose).
    for (slot, &(index, side)) in wheel_nodes.iter().enumerate() {
        commands
            .entity(entity_at(index))
            .insert((Roadwheel { side }, WheelIndex(slot)));
    }

    // --- Structural markers.
    let hull = entity_at(hull_index.expect("contract checked"));
    let gun = entity_at(gunner_pitch.expect("contract checked"));
    let turret = entity_at(turret_index.expect("contract checked"));
    let muzzle = entity_at(primary_muzzle_index.expect("contract checked"));
    commands.entity(hull).insert(Hull);
    commands.entity(gun).insert(Gun);
    commands.entity(turret).insert(Turret);

    // --- The root: hull-level per-variant data + the assembled handles/state. Mass properties
    // are AUTHORED, never derived from the abstract collision proxy (ADR-0011): `NoAuto*` makes
    // the proxies contribute zero mass. The centre of mass is the authored `Center_Of_Mass`
    // empty's root-relative position, straight from data (the runtime used to invert the root's
    // `GlobalTransform` for this — a lazy read the split retires).
    let (ex, ey, ez) = spec.inertia_extents;
    let parts: HashMap<String, Entity> = entities
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| entity.map(|e| (geometry.nodes[index].name.clone(), e)))
        .collect();
    commands.entity(root).insert((
        spec.drivetrain.clone(),
        spec.suspension.clone(),
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
        // loaded; servo rests are spawned config, not captured state).
        TankSim {
            servos: vec![ServoState::default(); spec.servos.len()],
            weapons: vec![WeaponState::default(); spec.weapons.len()],
            anchors: vec![None; wheel_nodes.len()],
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

/// The view binder: when the tank's glb scene instantiates, join its named nodes against the sim
/// skeleton's part table ([`SimParts`]) — presentation attaching onto an already-complete sim.
/// Nothing here constructs sim state; the scene is free to arrive seconds late (a visual pop-in,
/// not a bind window). Observed per spawn path via `.observe(…)`, like the binder it replaces.
///
/// All render-side (design §6C):
///   - hide the authored physics geometry (collision proxies, ballistic volumes — their sim
///     colliders are built from data; the glb copies are just meshes);
///   - tag every glb node that has a same-named sim part with [`ViewOf`] (+ [`ViewServo`] where
///     the part is a servo frame — `interpolate_servos`' write set) and back-link the sim part
///     with [`ViewNode`] — the join every render reader resolves through ([`ViewNode::resolve`]);
///   - seed each moving view node (servo, barrel) at its sim part's CURRENT pose, so a scene
///     attaching mid-slew never shows the authored rest pose, not even for the one frame before
///     the render writers first run;
///   - if a sim part already detached (cook-off fired during the scene load), attach its view
///     subtree to the free body now — the `Add<LaunchedTurret>` observer fired before this join
///     existed and never re-fires.
pub fn bind_tank_view(
    ready: On<WorldInstanceReady>,
    roots: Query<&SimParts>,
    children: Query<&Children>,
    names: Query<&Name>,
    meshes: Query<(), With<Mesh3d>>,
    servos: Query<(), With<ServoSpec>>,
    barrels: Query<(), With<GunBarrel>>,
    launched: Query<(), With<crate::damage::LaunchedTurret>>,
    transforms: Query<&Transform>,
    mut commands: Commands,
) {
    let Ok(parts) = roots.get(ready.entity) else {
        return;
    };
    // The root's descendants hold TWO same-named trees: the sim skeleton and the instantiated
    // scene. This walk's subject is the SCENE — skip the sim parts, or every skeleton node gets
    // a self-referential `ViewOf` (which would corrupt the cook-off detach) and the hide rule
    // stamps `Visibility` onto bare skeleton nodes (B0004 warning per node, measured 48/run).
    let skeleton: HashSet<Entity> = parts.0.values().copied().collect();
    for entity in children.iter_descendants(ready.entity) {
        if skeleton.contains(&entity) {
            continue;
        }
        let Ok(name) = names.get(entity) else {
            continue;
        };
        if name.as_str().ends_with("_Collider") || name.as_str().ends_with("_Ballistic") {
            commands.entity(entity).insert(Visibility::Hidden);
        }
        // Primitive leaves (`Mesh3d`) are render geometry, not part-named nodes — a mesh sharing
        // a part's name (Blender mesh data often shares its object's name) must not be joined.
        // `Mesh3d` presence is the reliable discriminator, NOT `GltfMaterialName` (absent on
        // unnamed-material primitives — the step-0 shadow lesson).
        if meshes.contains(entity) {
            continue;
        }
        let Some(&sim) = parts.0.get(name.as_str()) else {
            continue;
        };
        // Already launched: same attach `detach_view_on_turret_launch` performs, done here
        // because that observer fired (and no-oped) before the scene existed. No `ViewOf` — the
        // subtree rides the free body whole; its child parts below still join normally.
        if launched.contains(sim) {
            commands.entity(sim).insert(Visibility::default());
            commands
                .entity(entity)
                .insert((ChildOf(sim), Transform::IDENTITY));
            continue;
        }
        commands.entity(entity).insert(ViewOf(sim));
        commands.entity(sim).insert(ViewNode(entity));
        // Runtime-written parts start at the sim's current pose (tick truth), not the authored
        // rest the glb shipped — a scene attaching mid-slew must never flash the rest pose.
        if (servos.contains(sim) || barrels.contains(sim))
            && let Ok(&pose) = transforms.get(sim)
        {
            commands.entity(entity).insert(pose);
        }
        if servos.contains(sim) {
            commands.entity(entity).insert(ViewServo);
        }
    }
}

/// Copy each recoiling barrel's tick-truth transform onto its view node. The recoil spring steps
/// on the fixed clock (`apply_recoil` writes the SIM barrel — the muzzle chain `fire` composes
/// must carry the offset), so the view copy renders at fixed rate too — exactly the pre-split
/// look (barrel recoil was never overstep-blended). A local-space copy is exact: the view node
/// sits in a parent chain identical to its sim part's.
fn sync_view_barrels(
    barrels: Query<(&Transform, &ViewNode), With<GunBarrel>>,
    mut views: Query<&mut Transform, Without<GunBarrel>>,
) {
    // A launched turret's subtree keeps its barrel link: both trees ride the same free body, so
    // the local copy stays exact there too.
    for (source, view) in &barrels {
        if let Ok(mut dest) = views.get_mut(view.0) {
            dest.set_if_neq(*source);
        }
    }
}

/// The view half of the cook-off detach (design §6C): when the sim decides the turret comes off
/// (`damage::launch_turrets_on_cookoff` strips its `ChildOf` and makes it a free rigid body), the
/// view turret subtree reparents under that free sim body with an identity offset and follows it
/// whole. Its `ViewOf`/`ViewServo` come off — the launched sim turret has no servo components
/// left, so nothing would (or should) keep writing the view node's local transform.
fn detach_view_on_turret_launch(
    add: On<Add, crate::damage::LaunchedTurret>,
    views: Query<&ViewNode>,
    mut commands: Commands,
) {
    let Ok(view) = views.get(add.entity) else {
        return;
    };
    // The free sim body becomes the view subtree's new visibility root — without its own
    // `Visibility` the reparented view node's inheritance chain breaks (B0004).
    commands.entity(add.entity).insert(Visibility::default());
    commands
        .entity(view.0)
        .insert((ChildOf(add.entity), Transform::IDENTITY))
        .remove::<(ViewOf, ViewServo)>();
}

/// The render-side view-attach systems every tank-spawning composition mounts exactly once:
/// [`sim_plugin`] pulls it in for the game and the net bins; the armor sandbox (which runs no
/// servo sim) mounts it directly. `interpolate_servos` is NOT here — it needs the `Playing`
/// gate and the fixed-clock state only sim compositions have. `sync_view_barrels` runs ungated
/// deliberately: the sandbox has no `AppState`, and outside gameplay the copy is a no-op
/// (`set_if_neq` over a handful of entities).
pub fn view_attach_plugin(app: &mut App) {
    app.add_observer(detach_view_on_turret_launch);
    app.add_systems(Update, sync_view_barrels);
}

/// The servo's absolute pose rotation: rest · R(axis, angle) — shared by all three writers
/// (truth restore, mechanism step, render blend), so there is exactly one formula.
fn servo_rotation(spec: &ServoSpec, rest: &ServoRest, angle: f32) -> Quat {
    rest.0 * Quat::from_axis_angle(spec.role.axis(), angle)
}

/// Top of each fixed tick: re-assert every sim servo node's `Transform` from the root-resident
/// state, so sim readers inside this tick (`rig_world_pose` chains, the child-collider sync) see
/// tick state derived from `TankSim` — during a rollback replay this is what re-derives the node
/// transforms from the RESTORED state each replayed tick (without it, the first replayed `fire`
/// would compose the abandoned timeline's muzzle pose). On a normal tick it re-writes the value
/// `drive_servos` left there — since step 2 nothing render-side touches sim transforms anymore
/// (`interpolate_servos` writes the view tree), so this is the invariant's cheap enforcement, a
/// quat multiply per servo, rather than an undo of the render lerp.
fn restore_servo_truth(
    mut q: Query<(
        &mut Transform,
        &ServoSpec,
        &ServoRest,
        &ServoIndex,
        &TankRoot,
    )>,
    sims: Query<&TankSim>,
) {
    for (mut transform, spec, rest, slot, root) in &mut q {
        let Some(state) = sims.get(root.0).ok().and_then(|sim| sim.servos.get(slot.0)) else {
            continue;
        };
        transform.rotation = servo_rotation(spec, rest, state.current);
    }
}

fn drive_servos(
    mut q: Query<(
        &mut Transform,
        &ServoSpec,
        &ServoRest,
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
    for (mut transform, spec, rest, command, slot, root) in &mut q {
        let Ok(mut sim) = sims.get_mut(root.0) else {
            continue;
        };
        let Some(state) = sim.servos.get_mut(slot.0) else {
            continue;
        };
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
        // child-collider sync and any later-in-tick reader consume. Render smoothing happens on
        // the VIEW tree (`interpolate_servos`); this transform never carries a blended pose.
        transform.rotation = servo_rotation(spec, rest, state.current);
    }
}

/// The render half of the fixed-clock servo split: blend last tick's angle to this tick's by the
/// fixed clock's overstep and write the **view** node's `Transform` — smooth mechanism motion at
/// any frame rate, exactly how Avian renders the hull between physics ticks. Along the shortest
/// arc, so a continuous mount's ±π wrap doesn't spin the long way round.
///
/// Writes VIEW nodes only (design §6C): the sim servo node's `Transform` is pure tick truth,
/// written by `drive_servos`/`restore_servo_truth` alone, so no sim reader can ever see a
/// render-blended pose. The view node resolves its sim source through [`ViewOf`]; a launched
/// turret's view node loses `ViewServo` at detach and drops out of this write set.
fn interpolate_servos(
    time: Res<Time<Fixed>>,
    mut views: Query<(&mut Transform, &ViewOf), With<ViewServo>>,
    servos: Query<(&ServoSpec, &ServoRest, &ServoIndex, &TankRoot)>,
    sims: Query<&TankSim>,
) {
    let alpha = time.overstep_fraction();
    for (mut transform, view_of) in &mut views {
        let Ok((spec, rest, slot, root)) = servos.get(view_of.0) else {
            continue;
        };
        let Some(state) = sims.get(root.0).ok().and_then(|sim| sim.servos.get(slot.0)) else {
            continue;
        };
        let angle = state.previous + shortest_angle(state.current - state.previous) * alpha;
        // Guarded write: a settled mount must not re-dirty the view transform every frame.
        let rotation = servo_rotation(spec, rest, angle);
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

/// Wrap an angle difference into [-PI, PI] for shortest-path rotation.
pub(crate) fn shortest_angle(diff: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (diff + PI).rem_euclid(TAU) - PI
}
