//! The tank: its rig (structural markers bound by node name), the kinematic `Servo` motor
//! for the turret/gun, and the asset-load binding. The tank declares *structure*; features
//! (aim, shooting) attach their own behavior to these markers reactively.

use std::collections::HashSet;

use avian3d::prelude::{
    AngularInertia, ColliderConstructor, ColliderConstructorHierarchy, CollisionLayers, LayerMask,
    Mass, NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, RayCaster, RigidBody,
    SpatialQueryFilter,
};
use bevy::asset::LoadState;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use serde::Deserialize;

use crate::Layer;
use crate::ballistics::{ArmorVolume, BallisticVolume, ComponentHealth, ComponentVolume};
use crate::damage::{Ammo, Crewman, TankCapabilities, VolumeOf};
use crate::sight::SightMode;
use crate::spec::{TankSpec, TankSpecHandle};
use crate::state::{AppState, GameplaySet};

// --- Rig markers. Name = the structural contract between the model and the code. ---

#[derive(Component)]
pub struct Turret;

#[derive(Component)]
pub struct Gun;

#[derive(Component)]
pub struct Hull;

/// Marks the vehicle's root entity — the dynamic rigid body (chassis). Suspension/drive forces
/// are applied here; debug x-ray walks its descendants.
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
    pub barrel: Entity,
}

#[derive(Component)]
pub struct Muzzle;

/// The recoiling barrel node (child of `Gun`, parent of `Muzzle`).
#[derive(Component)]
pub struct GunBarrel;

/// Which track a roadwheel drives (for differential thrust). Left wheels sit at −X, right at +X.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrackSide {
    Left,
    Right,
}

/// A load-bearing roadwheel — a suspension/drive contact station, tagged with its track side.
/// Carries a downward [`RayCaster`] (the suspension ray); the sprocket and idler are excluded.
#[derive(Component)]
pub struct Roadwheel {
    pub side: TrackSide,
}

/// The authored centre-of-mass: an Empty (`Center_Of_Mass`) placed in the model. `driving` reads
/// its position and sets the body's centre of mass from it — the model owns the COM.
#[derive(Component)]
pub struct CenterOfMassAnchor;

/// The local axis a servo rotates about. Cardinal-only — tank servos yaw/pitch about a hull axis;
/// a canted mount would add a `Custom(Dir3)` variant. Resolved to a vector in `drive_servos`.
#[derive(Clone, Copy, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn to_vec3(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
}

/// Travel limits for a [`ServoSpec`], in **degrees** (the authoring unit).
#[derive(Clone, Copy, Deserialize)]
pub enum Travel {
    Limited { min: f32, max: f32 },
    Continuous,
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
    axis: Axis,
    /// Max slew speed, degrees/second.
    max_speed: f32,
    /// Slew acceleration, degrees/second².
    accel: f32,
    travel: Travel,
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
/// `rest` is the node's authored pose at `current = 0`, captured once so `drive_servos` can write
/// *absolute* rotations (`rest · R(axis, angle)`) instead of accumulating deltas — cleaner with
/// variable render-rate `dt` (no accumulating round-off).
///
/// **Why `Update`, not `FixedUpdate`:** the servos are *kinematic display* mechanisms — we drive
/// their transform ourselves; no physics depends on their pose (the hull is a body, the turret/gun
/// are plain scene nodes; only `fire` reads the muzzle, and it reads the render pose). Running them
/// at render rate makes the gun pose, the gunner camera (which bolts to it), and the mouse-driven
/// intent all share one clock → no interpolation, no aliasing. The cost is fixed-step determinism,
/// which the single-player vertical slice doesn't need; server-authoritative multiplayer will run
/// the motion profile on the server's fixed clock and have the client interpolate snapshots (a
/// different, simpler interpolation than self-sim). ADR-0004's "sim-in-fixed" bet is about *physics*.
#[derive(Component)]
pub struct ServoState {
    current: f32,
    velocity: f32,
    rest: Quat,
    captured: bool,
}

impl Default for ServoState {
    fn default() -> Self {
        Self {
            current: 0.0,
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

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, load_tank_spec)
        .add_systems(
            Update,
            spawn_tank_when_loaded.run_if(in_state(AppState::Loading)),
        )
        // `drive_servos` runs in `Update` (render rate), *after* the aim systems in `GameplaySet`
        // have written this frame's `ServoCommand.target` — so it chases the fresh target the same
        // frame, and the gun's `GlobalTransform` (computed by propagation in `PostUpdate`) is
        // current for the gunner camera and HUD reprojection that read it. No interpolation needed:
        // the pose is written fresh at render rate, same clock as the mouse-driven intent. (The
        // single-player slice trades fixed-step determinism for display simplicity; server-authority
        // will put the motion profile back on the server's fixed clock and have the client
        // interpolate snapshots. ADR-0004's "sim-in-fixed" bet is about *physics*.)
        .add_systems(
            Update,
            drive_servos
                .run_if(in_state(AppState::Playing))
                .after(GameplaySet),
        )
        // The crew/vehicle swap input. Runs before `GameplaySet` so the control systems this frame
        // already see the new `Controlled` tank.
        .add_systems(
            Update,
            swap_controlled_tank
                .run_if(in_state(AppState::Playing))
                .before(GameplaySet),
        );
}

/// The tank's spec sheet is a *load dependency* (ADR-0011): we kick off its load up front and the
/// tank scene is spawned only once it's ready, so the rig binds with its stats already in hand —
/// no spec-less window. While it loads we sit in `AppState::Loading`.
#[derive(Resource)]
struct PendingTankSpec(Handle<TankSpec>);

fn load_tank_spec(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(PendingTankSpec(
        asset_server.load("tiger_1/tiger_1.tank.ron"),
    ));
}

/// Once the spec has loaded, spawn the tank and enter `Playing`. A *failed* spec load is fatal
/// here (no fallback stats, ADR-0011); a still-loading spec just waits another frame.
fn spawn_tank_when_loaded(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pending: Option<Res<PendingTankSpec>>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(pending) = pending else {
        return;
    };
    match asset_server.load_state(&pending.0) {
        LoadState::Loaded => {
            // Two tanks, both player-owned: `Tab` swaps which one is `Controlled`. The first spawns
            // controlled; the second sits until you swap into it (design: the antagonist/auto-aim
            // lands in Chunk 2). Both are dynamic bodies — per-tank suspension holds each up; only
            // the controlled one takes drive input.
            spawn_tank(
                &mut commands,
                &asset_server,
                &pending.0,
                Transform::from_xyz(10.0, 2.0, 5.0).with_rotation(Quat::from_rotation_z(0.7)),
                "Tiger I (A)",
                true,
            );
            spawn_tank(
                &mut commands,
                &asset_server,
                &pending.0,
                Transform::from_xyz(10.0, 2.0, -12.0),
                "Tiger I (B)",
                false,
            );
            commands.remove_resource::<PendingTankSpec>();
            next.set(AppState::Playing);
        }
        LoadState::Failed(err) => {
            error!("required tank spec sheet failed to load: {err}");
            panic!("required tank spec sheet failed to load: {err}");
        }
        _ => {}
    }
}

/// Spawn one Tiger at `transform`, binding its rig via [`on_tank_ready`]. `controlled` seeds the
/// player's starting tank with the [`Controlled`] marker. The hull is a dynamic rigid body — Avian
/// owns its Transform (ADR-0005); its collider comes from the model's `*_Collider` proxy, bound in
/// `on_tank_ready`.
fn spawn_tank(
    commands: &mut Commands,
    asset_server: &AssetServer,
    spec: &Handle<TankSpec>,
    transform: Transform,
    name: &str,
    controlled: bool,
) {
    let mut tank = commands.spawn((
        WorldAssetRoot(
            asset_server.load(GltfAssetLabel::Scene(0).from_asset("tiger_1/tiger_1.glb")),
        ),
        TankSpecHandle(spec.clone()),
        transform,
        Name::new(name.to_string()),
        Tank,
        RigidBody::Dynamic,
    ));
    tank.observe(on_tank_ready);
    if controlled {
        tank.insert(Controlled);
    }
}

/// `Tab` hands control to the next tank: it moves the [`Controlled`] marker, and resets the view to
/// third-person with both tanks visible — so you never inherit the gunner optic's tank-hide on the
/// tank you just stepped out of. With two tanks this is a toggle; it generalizes to cycling N in
/// spawn order.
fn swap_controlled_tank(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    tanks: Query<Entity, With<Tank>>,
    controlled: Query<Entity, With<Controlled>>,
    mut mode: ResMut<SightMode>,
    mut visibility: Query<&mut Visibility, With<Tank>>,
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

    // Drop back to third-person and reveal both tanks: the optic hides the controlled tank's mesh,
    // so without this the tank you just left would stay invisible.
    *mode = SightMode::ThirdPerson;
    for mut v in &mut visibility {
        *v = Visibility::Inherited;
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
        // Root visibility owns the gunner-view hide: set to `Hidden`, `InheritedVisibility`
        // propagates `HIDDEN` to every descendant mesh, so the gunner optic (camera parked at the
        // gun pivot, inside the mantlet) sees no own-tank geometry — no near-plane clipping.
        Visibility::Inherited,
    ));

    // Record what the walk found, to check against the required contract afterwards.
    let mut found: HashSet<&'static str> = HashSet::new();
    let mut bound_volumes: HashSet<String> = HashSet::new();
    let mut left_wheels = 0u32;
    let mut right_wheels = 0u32;
    let mut colliders = 0u32;
    // Rig-node handles captured for this tank's `Rig` (built after the contract check below).
    let mut hull_node = None;
    let mut turret_node = None;
    let mut gun_node = None;
    let mut muzzle_node = None;
    let mut barrel_node = None;

    for entity in children.iter_descendants(ready.entity) {
        // Most descendants are unnamed mesh nodes — skip them quietly.
        let Ok(name) = names.get(entity) else {
            continue;
        };
        let id = entity;
        let mut entity = commands.entity(entity);
        match name.as_str() {
            // Servos: marker + command/state slots + the per-variant `ServoSpec` (axis, speeds,
            // travel) from the spec sheet (ADR-0010) — never authored in code.
            "Turret" => {
                found.insert("Turret");
                turret_node = Some(id);
                entity.insert((
                    Turret,
                    ServoCommand::default(),
                    ServoState::default(),
                    spec.turret.clone(),
                ));
            }
            "Gun" => {
                found.insert("Gun");
                gun_node = Some(id);
                entity.insert((
                    Gun,
                    ServoCommand::default(),
                    ServoState::default(),
                    spec.gun.clone(),
                ));
            }
            "Hull" => {
                found.insert("Hull");
                hull_node = Some(id);
                entity.insert(Hull);
            }
            "Muzzle" => {
                found.insert("Muzzle");
                muzzle_node = Some(id);
                entity.insert(Muzzle);
            }
            "Gun_Barrel" => {
                found.insert("Gun_Barrel");
                barrel_node = Some(id);
                entity.insert(GunBarrel);
            }
            "Center_Of_Mass" => {
                found.insert("Center_Of_Mass");
                entity.insert(CenterOfMassAnchor);
            }
            // Roadwheels (Wheel_L_0.., Wheel_R_0..): tag the track side + a downward suspension ray
            // sized by the variant's `ray_length`, filtered to Terrain so it skips the tank's own
            // collider. The wheel node has identity rotation, so local −Y is the hull-down axis.
            s if s.starts_with("Wheel_") => {
                let side = if s.starts_with("Wheel_L") {
                    left_wheels += 1;
                    TrackSide::Left
                } else {
                    right_wheels += 1;
                    TrackSide::Right
                };
                entity.insert((
                    Roadwheel { side },
                    RayCaster::new(Vec3::ZERO, Dir3::NEG_Y)
                        .with_max_distance(spec.suspension.ray_length)
                        .with_query_filter(SpatialQueryFilter::from_mask(Layer::Terrain)),
                ));
            }
            // Collision proxies (`*_Collider` / `*_Collision`, optionally split `_0/_1/...`): a
            // convex-hull collider on the Vehicle layer, hidden (it's physics, not rendering —
            // ADR-0008). Collision-only: it contributes no mass (the hull authors its own, see above).
            // The glTF loader puts the mesh on a child primitive, so build over the node's descendants.
            s if s.starts_with("Collider_") => {
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
            // Ballistic volumes — the spec's `volumes` map is the source of truth (design
            // `armor-penetration-and-damage.md` §12; composition, not a `kind` enum): a node is a
            // volume iff it's a key. `material_factor` (shell-resistance per metre) every volume has;
            // optional `hp` makes it a damageable component. The `Armor_/Module_/...` prefix is
            // documentation only, never parsed for behaviour — resistance and role both come from
            // data, so a steel barrel module resists like steel yet still takes damage.
            //
            // Bound as a query-only trimesh collider on the `Armor` layer (watertight solids may be
            // concave — fine for a raycast, unlike the dynamic physics proxy, ADR-0008) with NO
            // collision response (`filters = NONE`), so it never perturbs the body. Hidden, like
            // `*_Collider` — the march raycasts it and the sandbox visualizes it itself.
            s if spec.volumes.contains_key(s) => {
                let volume = &spec.volumes[s];
                bound_volumes.insert(s.to_string());
                entity.insert((
                    Visibility::Hidden,
                    ColliderConstructorHierarchy::new(ColliderConstructor::TrimeshFromMesh)
                        .with_default_layers(CollisionLayers::new([Layer::Armor], LayerMask::NONE)),
                    BallisticVolume {
                        material_factor: volume.material_factor,
                    },
                    VolumeOf(ready.entity),
                ));
                assert!(
                    volume.hp.is_some()
                        || (volume.crew.is_none() && !volume.ammo && volume.function.is_none()),
                    "tank volume `{s}` declares a consequence facet but has no hp"
                );
                if let Some(crew) = volume.crew {
                    // Seat role + its native occupant (topology B): `home == seat` at bind, so
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
                    // Damageable (module/crew/ammo): an HP pool the march depletes (transit/spall/
                    // shock). The consequences of HP→0 (§§7–8) are a later increment.
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
            // Drift lint: a `Ballistic_*` node absent from the spec's `volumes` map — likely an
            // authoring slip (forgot to declare it, or a rename diverged). Ignored, not bound.
            s if s.starts_with("Ballistic_") => {
                warn!(
                    "node `{s}` is named like a ballistic volume but has no entry in the tank \
                     spec's `volumes` map — ignoring (add it, or rename if it isn't one)"
                );
            }
            _ => {}
        }
    }

    // Required singletons, plus ≥1 collider (else the body is massless → NaN) and ≥1 roadwheel per
    // side (else a track has no support/thrust). A real Tiger has many wheels; the sim only needs
    // one contact station per side to be non-degenerate, so the contract is per-side presence, not
    // a fixed count (which varies per variant).
    const REQUIRED: [&str; 6] = [
        "Hull",
        "Turret",
        "Gun",
        "Gun_Barrel",
        "Muzzle",
        "Center_Of_Mass",
    ];
    let mut missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|n| !found.contains(n))
        .collect();
    if colliders == 0 {
        missing.push("*Collider_");
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

    // The contract check above guarantees all five rig nodes were found, so these unwraps can't
    // fire. Record them so control systems can address *this* tank's parts by entity (`rig.gun`).
    commands.entity(ready.entity).insert(Rig {
        hull: hull_node.unwrap(),
        turret: turret_node.unwrap(),
        gun: gun_node.unwrap(),
        muzzle: muzzle_node.unwrap(),
        barrel: barrel_node.unwrap(),
    });

    // The spec's `volumes` map is the model↔code contract: every declared volume must exist in the
    // model (a missing one is an authoring bug — fatal like a missing rig node).
    let missing_volumes: Vec<&String> = spec
        .volumes
        .keys()
        .filter(|name| !bound_volumes.contains(*name))
        .collect();
    assert!(
        missing_volumes.is_empty(),
        "tank spec declares ballistic volumes with no matching model node: {missing_volumes:?}"
    );
}

fn drive_servos(
    mut q: Query<(&mut Transform, &ServoSpec, &ServoCommand, &mut ServoState)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut transform, spec, command, mut state) in &mut q {
        // Capture the node's authored rest rotation once, so we can write *absolute* rotations
        // (`rest · R(axis, angle)`) instead of accumulating deltas — robust to variable render-rate
        // `dt` (no accumulating round-off).
        if !state.captured {
            state.rest = transform.rotation;
            state.captured = true;
        }

        // `ServoSpec` authors angles in degrees (the human authoring unit); the runtime — the
        // command, the state, and the slew maths below — is radians. Convert the spec's angular
        // quantities once here, at the spec→runtime boundary.
        let max_speed = spec.max_speed.to_radians();
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

        // Absolute write of the sim-truth pose. `rest` is the node's authored rotation at
        // `current = 0`; composing the axis-angle onto it gives the true mechanism pose without
        // accumulating deltas (robust to variable render-rate `dt`).
        transform.rotation = state.rest * Quat::from_axis_angle(spec.axis.to_vec3(), state.current);
    }
}

/// Wrap an angle difference into [-PI, PI] for shortest-path rotation.
pub(crate) fn shortest_angle(diff: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (diff + PI).rem_euclid(TAU) - PI
}
