//! Offline driving scenarios and scripted inputs to the game's damage model, game state only.

use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;

use super::model::{Controlled, Tank};
use super::spawn::{
    PendingTankAssets, TankContent, TankPresentation, TankSimSource, load_tank_assets,
    spawn_complete_tank,
};
use crate::sight::SightMode;
use crate::state::{AppState, GameplaySet};
use crate::{CombatantId, CombatantLife};

pub fn sp_spawn_plugin(app: &mut App) {
    app.add_systems(Startup, load_tank_assets).add_systems(
        Update,
        spawn_tank_when_loaded.run_if(in_state(AppState::Loading)),
    );
    if target_scenario().is_some() {
        app.add_systems(Update, aim_camera_at_target);
    }
}

/// Refusals of the operator's offline scenario, reported before any scenario body is spawned.
#[derive(Debug)]
enum ScenarioFault {
    Start(String),
    Target(String),
    ConflictingDemo,
    MissingVolume(&'static str),
}

impl std::fmt::Display for ScenarioFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start(value) => write!(f, "OVERMATCH_START={value:?}: want finite <x>,<z>[,<heading degrees>] or one of {:?}", START_SPOTS.map(|(name, ..)| name)),
            Self::Target(value) => write!(f, "OVERMATCH_TARGET={value:?}: want ahead, fires or kills"),
            Self::ConflictingDemo => f.write_str("OVERMATCH_TARGET conflicts with the capture plan's HullMarks demo; select ahead or unset the lever"),
            Self::MissingVolume(role) => write!(f, "OVERMATCH_TARGET requires a declared {role} volume; select a compatible tank or another scenario"),
        }
    }
}

/// The game's damage model supplies the functional membership; node names only address it.
#[derive(Clone, Copy)]
enum AimRole {
    Engine,
    Ammunition,
}

impl AimRole {
    fn matches(self, component: &crate::spec::ComponentSpec) -> bool {
        match self {
            Self::Engine => {
                component.function.map(|function| function.role)
                    == Some(crate::damage::FunctionRole::Engine)
            }
            Self::Ammunition => component.ammo,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Engine => "Engine",
            Self::Ammunition => "ammunition",
        }
    }
}

/// In the game's damage model, aim at each selected module's construction bounds in its current
/// sim-part pose. The bake certifies the primitive buffers; the spawn builds the complete part map.
fn aim_centres(
    blueprint: &crate::bake::TankBlueprint,
    parts: &super::SimParts,
    poses: &Query<&GlobalTransform>,
    role: AimRole,
) -> Vec<Vec3> {
    let mut volumes: Vec<_> = blueprint
        .spec
        .volumes
        .iter()
        .filter(|(_, volume)| {
            volume
                .component()
                .is_some_and(|component| role.matches(component))
        })
        .collect();
    volumes.sort_by(|a, b| a.0.cmp(b.0));
    volumes
        .into_iter()
        .map(|(name, _)| {
            let index = blueprint
                .geometry
                .by_name
                .get(name)
                .expect("the bake resolves every declared component volume");
            let node = &blueprint.geometry.nodes[*index];
            let (lo, hi) = node
                .primitives
                .iter()
                .flat_map(|primitive| &primitive.positions)
                .map(|position| Vec3::from(*position))
                .fold(
                    (Vec3::INFINITY, Vec3::NEG_INFINITY),
                    |(lo, hi), position| (lo.min(position), hi.max(position)),
                );
            assert!(
                lo.is_finite() && hi.is_finite(),
                "the bake certifies finite nonempty component primitive buffers"
            );
            let entity = *parts
                .0
                .get(name)
                .expect("spawn builds every declared volume into SimParts");
            poses
                .get(entity)
                .expect("a spawned sim part keeps its GlobalTransform with its tank")
                .transform_point((lo + hi) * 0.5)
        })
        .collect()
}

fn require_aim_role(spec: &crate::spec::TankSpec, role: AimRole) -> Result<(), ScenarioFault> {
    if spec.volumes.values().any(|volume| {
        volume
            .component()
            .is_some_and(|component| role.matches(component))
    }) {
        Ok(())
    } else {
        Err(ScenarioFault::MissingVolume(role.name()))
    }
}

/// Named start spots on the shipped map, fixture data for the owner's hand-driving of the track
/// prints (cosmetic terrain marks): `(name, x, z, heading°)`, each a stretch where one ground type's
/// pack dominates the painted mask for 20–30 m along the heading (read off `terrain_masks.png`).
const START_SPOTS: [(&str, f32, f32, f32); 3] = [
    ("mud", 599.9, -599.9, 195.0),
    ("sand", 302.5, 542.7, 315.0),
    ("base", 70.0, -20.0, 90.0),
];

/// The dev start lever, `OVERMATCH_START=<x>,<z>[,<heading°>]` or `OVERMATCH_START=<spot>` (one of
/// [`START_SPOTS`]): a normal, drivable offline game whose player tank spawns there, heading
/// `heading°` from `+Z` toward `+X`, rather than at the duel. World XZ and radians out. Unset is
/// every normal run; a dev lever on the offline path only, like [`target_scenario`].
///
/// The owner sets this operator lever for hand-driving terrain-print checks at named spots.
/// Refuses startup when the variable is set to anything [`parse_start`] refuses: a capture or a measurement that
/// asked for a spot must never frame the duel as if it were that spot.
pub(crate) fn start_spot() -> Option<(Vec2, f32)> {
    let spec = crate::env_value("OVERMATCH_START").unwrap_or_else(crate::refuse_start)?;
    let (at, heading) = parse_start(&spec)
        .ok_or_else(|| ScenarioFault::Start(spec.clone()))
        .unwrap_or_else(crate::refuse_start);
    info!("OVERMATCH_START: the player's tank spawns at {at} heading {heading}°");
    Some((at, heading.to_radians()))
}

/// The spot `spec` names, world XZ and heading in degrees: a [`START_SPOTS`] name (any case), or
/// two or three finite comma-separated numbers; `None` for anything else.
fn parse_start(spec: &str) -> Option<(Vec2, f32)> {
    let spec = spec.trim();
    if let Some(&(_, x, z, heading)) = START_SPOTS
        .iter()
        .find(|(name, ..)| name.eq_ignore_ascii_case(spec))
    {
        return Some((Vec2::new(x, z), heading));
    }
    let parts: Vec<f32> = spec
        .split(',')
        .map(|part| {
            part.trim()
                .parse()
                .ok()
                .filter(|value: &f32| value.is_finite())
        })
        .collect::<Option<_>>()?;
    match parts[..] {
        [x, z] => Some((Vec2::new(x, z), 0.0)),
        [x, z, heading] => Some((Vec2::new(x, z), heading)),
        _ => None,
    }
}

/// The offline target scenario, `OVERMATCH_TARGET=ahead|fires|kills`: a dev lever on the offline
/// path only, for captures of hits in the game's damage model. Every scenario parks the duel on the
/// FAR pad ([`PROBE_FAR_DUEL_XZ`], both points measured flat) with the other tank 17 m down the
/// player's own −Z and the camera turned to look level down the line ([`aim_camera_at_target`]).
/// Unset is every normal run; any other value fails the boot naming it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TargetScenario {
    /// The player's tank faces the target, which stands broadside, so the aim point is on it and
    /// `SPIKE_AUTO_FIRE`'s windows hit it with no input (the pen cam's corner).
    Ahead,
    /// Both broadside, and the target fires into the player's flank on [`TARGET_FIRE_AT_S`] (the
    /// pen cam's victim glass); see [`offline_target_fires_plugin`].
    Fires,
    /// Both broadside, and the target fires until the player's tank is knocked out (the kill cam);
    /// see [`offline_target_fires_plugin`].
    Kills,
}

/// The scenario `OVERMATCH_TARGET` names, or [`TargetScenario::Ahead`] under a capture plan's
/// hull-marks demo, whose strikes need the target parked ahead (`hull_marks::demo`), so the plan
/// is that capture's one input; `None` when neither asks.
///
/// `scripts/perf/run-pencam-cost.sh` sets this lever. Unknown or conflicting selections refuse startup.
pub(crate) fn target_scenario() -> Option<TargetScenario> {
    let named = crate::env_value("OVERMATCH_TARGET")
        .unwrap_or_else(crate::refuse_start)
        .map(|spec| {
            TargetScenario::named(&spec)
                .ok_or_else(|| ScenarioFault::Target(spec.clone()))
                .unwrap_or_else(crate::refuse_start)
        });
    let demo = crate::dev_capture::plan().and_then(|plan| plan.scene.demo)
        == Some(crate::dev_capture::plan::Demo::HullMarks);
    if !demo {
        return named;
    }
    if named.is_some_and(|scenario| scenario != TargetScenario::Ahead) {
        return crate::refuse_start(ScenarioFault::ConflictingDemo);
    }
    Some(TargetScenario::Ahead)
}

impl TargetScenario {
    fn named(spec: &str) -> Option<Self> {
        match spec.trim() {
            "ahead" => Some(Self::Ahead),
            "fires" => Some(Self::Fires),
            "kills" => Some(Self::Kills),
            _ => None,
        }
    }
}

/// How long the target-ahead camera keeps re-aiming after the tanks spawn, s (TUNED: the duel
/// settles on its suspension well inside it).
const TARGET_AIM_S: f32 = 5.0;

/// Turn the player's camera to look just above the target-ahead placement's other tank's origin, every
/// frame for [`TARGET_AIM_S`] after the tanks spawn. The orbit camera reads its yaw and pitch back
/// off the rotation each frame and writes only the translation (from the rotation), so the look is
/// re-aimed from wherever the orbit put the camera; afterwards it holds until the mouse moves.
fn aim_camera_at_target(
    time: Res<Time>,
    mut since: Local<Option<f32>>,
    mut camera: Query<&mut Transform, With<crate::view::PlayerView>>,
    player: Query<(), (With<Tank>, With<Controlled>)>,
    target: Query<&GlobalTransform, (With<Tank>, Without<Controlled>)>,
) {
    // Before spawn there is no possession or target, and the camera may not be declared yet.
    if super::controlled_one(player.single()).is_none() {
        return;
    }
    let Some(target) = target.iter().next() else {
        return;
    };
    let mut transform = match camera.single_mut() {
        Ok(camera) => camera,
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => return,
        Err(error @ bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("the offline root declares one PlayerView: {error}")
        }
    };
    let since = *since.get_or_insert(time.elapsed_secs());
    if time.elapsed_secs() - since > TARGET_AIM_S {
        return;
    }
    let aim = target.translation() + Vec3::Y * TARGET_AIM_RISE_M;
    transform.rotation = transform.looking_at(aim, Vec3::Y).rotation;
}

/// How far above the target's origin the target-ahead camera looks, m: fixture data, putting the
/// aim on the hull side above the running gear rather than into the wheels.
const TARGET_AIM_RISE_M: f32 = 1.2;

/// Dev-only, offline: [`TargetScenario::Fires`] turns the player's tank broadside to the parked
/// target and has the target fire its main gun into the player's flank on [`TARGET_FIRE_AT_S`],
/// for captures of the player's own tank struck (the pen cam's victim glass). From spawn the target
/// lays its gun on the player's declared Engine volume; at each scheduled time it
/// pulls the trigger once.
///
/// [`TargetScenario::Kills`] sets the same broadside, but the target lays its gun on the player's
/// ammunition stowage nearest it and fires every [`TARGET_KILL_EVERY_S`] from
/// [`TARGET_KILL_FROM_S`] until the player's tank is knocked out, for captures of the kill cam. The
/// knockout is logged with its `Time<Real>` second.
///
/// Mounted only by the offline composition (`run_offline`), as `command::offline_auto_fire_plugin`
/// is: that root has no netcode, so the commands it writes never leave the process.
pub fn offline_target_fires_plugin(app: &mut App) {
    let scenario = target_scenario();
    if scenario == Some(TargetScenario::Kills) {
        info!(
            "target kills: armed, main gun from {TARGET_KILL_FROM_S} s every {TARGET_KILL_EVERY_S} s (Time<Real>)"
        );
        app.add_systems(
            RunFixedMainLoop,
            target_kills
                .after(crate::command::gather_commands)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(GameplaySet),
        );
    } else if scenario == Some(TargetScenario::Fires) {
        info!("target fires: armed, main gun at {TARGET_FIRE_AT_S:?} s (Time<Real>)");
        app.add_systems(
            RunFixedMainLoop,
            target_fires
                .after(crate::command::gather_commands)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(GameplaySet),
        );
    }
}

/// When the target-fires scenario's target pulls its trigger, `Time<Real>` seconds (TUNED: the
/// first after its turret has traversed onto the player and the tanks have settled, the rest a
/// reload apart and more).
const TARGET_FIRE_AT_S: [f32; 3] = [25.0, 40.0, 55.0];

/// How long each scheduled trigger pull stays latched, s: shorter than any reload, so it fires one
/// round.
const TARGET_FIRE_LATCH_S: f32 = 0.5;

/// When the target-kills scenario's target first pulls its trigger, and how often after,
/// `Time<Real>` seconds (TUNED: once the turret has traversed and the tanks have settled; longer
/// than a reload, so each pull fires).
const TARGET_KILL_FROM_S: f32 = 25.0;
const TARGET_KILL_EVERY_S: f32 = 10.0;

/// The target-kills scenario: the target's gun on the centre of the player's ammunition volume
/// nearest it (the union of its colliders' bounds), a pull every [`TARGET_KILL_EVERY_S`] until the
/// player's tank is knocked out.
fn target_kills(
    time: Res<Time<Real>>,
    blueprint: Res<crate::bake::TankBlueprint>,
    player: Query<
        (&super::SimParts, Has<crate::damage::TankKnockedOut>),
        (With<Tank>, With<Controlled>),
    >,
    poses: Query<&GlobalTransform>,
    mut targets: Query<
        (&GlobalTransform, &mut crate::command::TankCommand),
        (With<Tank>, Without<Controlled>),
    >,
    mut logged: Local<bool>,
) {
    // No controlled tank before spawn or between possessions; two controlled tanks are a bug.
    let Some((parts, knocked_out)) = super::controlled_one(player.single()) else {
        return;
    };
    let t = time.elapsed_secs();
    if knocked_out {
        if !*logged {
            *logged = true;
            info!("target kills: the player's tank is knocked out at {t:.2} s (Time<Real>)");
        }
        return;
    }
    let centres = aim_centres(&blueprint, parts, &poses, AimRole::Ammunition);
    let since = t - TARGET_KILL_FROM_S;
    let pull = since >= 0.0 && since % TARGET_KILL_EVERY_S < TARGET_FIRE_LATCH_S;
    for (target, mut command) in &mut targets {
        let from = target.translation();
        let at = centres
            .iter()
            .copied()
            .min_by(|a, b| {
                a.distance_squared(from)
                    .total_cmp(&b.distance_squared(from))
            })
            .expect("scenario admission requires an ammunition volume");
        command.aim = Some(crate::command::AimIntent::World(at));
        command.fire_primary |= pull;
    }
}

fn target_fires(
    time: Res<Time<Real>>,
    blueprint: Res<crate::bake::TankBlueprint>,
    player: Query<&super::SimParts, (With<Tank>, With<Controlled>)>,
    poses: Query<&GlobalTransform>,
    mut targets: Query<
        (&GlobalTransform, &mut crate::command::TankCommand),
        (With<Tank>, Without<Controlled>),
    >,
) {
    // The controlled sim body is absent before spawn; possession remains unique.
    let Some(parts) = super::controlled_one(player.single()) else {
        return;
    };
    let centres = aim_centres(&blueprint, parts, &poses, AimRole::Engine);
    let t = time.elapsed_secs();
    let pull = TARGET_FIRE_AT_S
        .iter()
        .any(|&at| (at..at + TARGET_FIRE_LATCH_S).contains(&t));
    for (target, mut command) in &mut targets {
        let from = target.translation();
        let at = centres
            .iter()
            .copied()
            .min_by(|a, b| {
                a.distance_squared(from)
                    .total_cmp(&b.distance_squared(from))
            })
            .expect("scenario admission requires an Engine volume");
        command.aim = Some(crate::command::AimIntent::World(at));
        command.fire_primary |= pull;
    }
}

/// Install the local duel's possession switch.
pub fn client_plugin(app: &mut App) {
    // Control systems in GameplaySet must see the new owner in the same frame.
    app.add_systems(
        Update,
        swap_controlled_tank
            .run_if(in_state(AppState::Playing))
            .before(GameplaySet),
    );
}

/// The local duel's two spawn POINTS — horizontal only (XZ), like every spawn definition in the
/// game. Their Y is sampled from the live surface at spawn time (`terrain_grid::spawn_pos`).
///
/// These used to be full poses with a hardcoded `y = 2.0`, which was correct for exactly one world:
/// the flat slab. Once the heightmap world landed, `--offline` spawned both Tigers ~116 m UNDER the
/// terrain (79 m after the 2560 → 1000 m re-scale) — they were never "falling through" anything,
/// they simply started inside the hill. The net server had already been fixed for this class; the
/// offline/single-player path had not, which is precisely why spawn Y is no longer authorable
/// anywhere.
pub(crate) const DUEL_SPAWN_XZ: [Vec2; 2] = [Vec2::new(10.0, 5.0), Vec2::new(10.0, -12.0)];

/// Where the duel stands under the FAR probe placement (`OVERMATCH_PROBE_FAR` — see
/// [`spawn_probe_tanks`]). Tiger A's point; Tiger B keeps its 17 m offset behind it, so the duel's
/// own geometry (and the camera's relationship to it) is the one it always has.
///
/// MEASURED off the shipped heightmap, not chosen by eye: the footprint relief under both duel
/// points is 0.22 m / 0.29 m (`terrain_grid::SPAWN_FOOTPRINT_HALF_M` square), the flattest pad on
/// the map that has a matching flat block 600 m down +Z, and the ground between the two rises
/// nowhere above the sight line — so the probe block is actually IN FRAME rather than behind a
/// crest. Flat matters for the same reason it did for the near band: parked tanks on a grade slide,
/// and a sliding scene measures contact resolution instead of the thing under test.
const PROBE_FAR_DUEL_XZ: Vec2 = Vec2::new(-410.0, -440.0);

/// Where the FAR grid's near edge sits, world z. Same units and same meaning as
/// [`PROBE_GRID_NEAR_Z`]: an absolute world coordinate, not an offset from the duel.
///
/// Paired with [`PROBE_FAR_DUEL_XZ`] so the whole grid lands 600..645 m from the controlled tank's
/// spawn — squarely inside ONE of the shoe chain's coarse bands (L3's `[542.3, 1499.6)` m as the
/// ladder currently certifies), and every probe inside the camera's 1 000 m far plane with room to
/// spare.
///
/// IT MOVED OUT 12 m ON 2026-08-12, and by the mechanism this comment says it should. The directed
/// search (ADR 0036) re-cut the ladder and L3's band opened 14 m later, at 542.3 m instead of
/// 527.9 m — which left the grid's nearest shoe 3 m inside the declared margin. The placement
/// tracks the band; the band is an output of generation.
///
/// No test asserts the band claim; a capture shows the band directly. Nothing here should be
/// re-derived by hand: the ladder is regenerated, and WHICH level owns 600..645 m is an output of
/// that regeneration.
const PROBE_FAR_NEAR_Z: f32 = 160.0;

/// Is this process running the FAR probe placement?
///
/// A dev/profiling lever on the offline path only, like [`spawn_probe_tanks`]'s own count: unset is
/// every normal run, and nothing outside the probe grid consults it.
// `scripts/perf/run-frame-sweep.sh` selects this placement.
pub(crate) fn probe_far() -> bool {
    crate::env_flag("OVERMATCH_PROBE_FAR", false).unwrap_or_else(crate::refuse_start)
}

/// The duel's two spawn points under the placement in force. `far` is [`probe_far`] — passed rather
/// than read so the terrain-clearance test can assert BOTH placements without touching the process
/// environment.
pub(crate) fn duel_spawn_xz(far: bool) -> [Vec2; 2] {
    if !far {
        return DUEL_SPAWN_XZ;
    }
    // A pure TRANSLATION of the authored duel, so the far scene differs from the near one in where
    // it stands and in nothing else.
    let shift = PROBE_FAR_DUEL_XZ - DUEL_SPAWN_XZ[0];
    DUEL_SPAWN_XZ.map(|xz| xz + shift)
}

/// Probe `index`'s spawn point under the placement in force — the grid's one geometry, shared by
/// the spawn and by the test that checks every one of those points against the shipped terrain.
pub(crate) fn probe_spawn_xz(far: bool, index: usize) -> Vec2 {
    let (column, row) = (index % PROBE_GRID_COLUMNS, index / PROBE_GRID_COLUMNS);
    let near_z = if far {
        PROBE_FAR_NEAR_Z
    } else {
        PROBE_GRID_NEAR_Z
    };
    Vec2::new(
        // Centre the block on the duel's x so it fills the view symmetrically.
        duel_spawn_xz(far)[0].x
            + (column as f32 - (PROBE_GRID_COLUMNS as f32 - 1.0) / 2.0) * PROBE_GRID_SPACING_M,
        near_z + row as f32 * PROBE_GRID_SPACING_M,
    )
}

/// Admit the local duel after presentation preloading, then construct both roots completely. This
/// is an admission policy; assets do not initialize simulation state. Failed loads remain fatal.
#[expect(
    clippy::too_many_arguments,
    reason = "admission spans the loader, assets, sim source, terrain, map, certificate, view profile and app state"
)]
fn spawn_tank_when_loaded(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pending: Res<PendingTankAssets>,
    source: TankSimSource,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    certificate: Res<crate::geometry_lod::TankCertificate>,
    view: Option<Res<crate::view::ViewProfile>>,
    manifest: Option<Res<crate::map::MapManifest>>,
    mut next: ResMut<NextState<AppState>>,
) {
    // `loaded` is where a FAILED load aborts, for every composition at once — see it.
    if !pending.loaded(&asset_server) {
        return;
    }
    let content = source.get();
    // Both bodies simulate; only the Controlled marker selects input ownership. Each spawn point is
    // horizontal — the ground under it is sampled NOW, through the one rule every spawn path shares
    // (`terrain_grid::spawn_pos`: footprint max + clearance), so the duel lands on whatever world
    // this build actually has: heightmap, flat slab, or a re-authored map later.
    let grid = height.as_deref();
    if crate::lod_showcase::enabled() {
        // THE SHOWCASE STANDS ITS PAIRS AT THEIR OWN SWITCH DISTANCES, so it cannot be laid out
        // until the view those distances are derived from has been read (`crate::view`: there is no
        // default view to lay it out against). Waiting a frame is the whole handling — this system
        // retries until the assets land anyway.
        let Some(view) = view else {
            return;
        };
        spawn_lod_showcase(
            &mut commands,
            content,
            pending.presentation(),
            grid,
            &certificate,
            *view,
        );
        commands.remove_resource::<PendingTankAssets>();
        next.set(AppState::Playing);
        return;
    }
    if super::battle::enabled() {
        super::battle::spawn(
            &mut commands,
            content,
            pending.presentation(),
            grid,
            manifest.as_deref(),
        );
        commands.remove_resource::<PendingTankAssets>();
        next.set(AppState::Playing);
        return;
    }
    let target = target_scenario();
    match target {
        Some(TargetScenario::Fires) => require_aim_role(content.spec(), AimRole::Engine),
        Some(TargetScenario::Kills) => require_aim_role(content.spec(), AimRole::Ammunition),
        Some(TargetScenario::Ahead) | None => Ok(()),
    }
    .unwrap_or_else(crate::refuse_start);
    let far = probe_far() || target.is_some();
    let duel = duel_spawn_xz(far);
    let (a_turn, b_turn) = match target {
        // Both broadside, A's flank to B's gun ([`offline_target_fires_plugin`]).
        Some(TargetScenario::Fires | TargetScenario::Kills) => (
            Quat::from_rotation_y(FRAC_PI_2),
            Quat::from_rotation_y(FRAC_PI_2),
        ),
        // A faces its own −Z, down the line to B; B stands broadside across it.
        Some(TargetScenario::Ahead) => (Quat::IDENTITY, Quat::from_rotation_y(FRAC_PI_2)),
        None => (Quat::from_rotation_z(0.7), Quat::IDENTITY),
    };
    // The player's own spawn, moved by the dev start lever when it is set.
    let (a_at, a_turn) = match start_spot() {
        // The hull faces its own −Z: a turn of `heading + π` about Y points it along the heading.
        Some((at, heading)) => (at, Quat::from_rotation_y(heading + std::f32::consts::PI)),
        None => (duel[0], a_turn),
    };
    spawn_tank(
        &mut commands,
        content,
        pending.presentation(),
        Transform::from_translation(crate::terrain_grid::spawn_pos(grid, a_at))
            .with_rotation(a_turn),
        "Tiger I (A)",
        0,
        true,
    );
    spawn_tank(
        &mut commands,
        content,
        pending.presentation(),
        Transform::from_translation(crate::terrain_grid::spawn_pos(grid, duel[1]))
            .with_rotation(b_turn),
        "Tiger I (B)",
        1,
        false,
    );
    spawn_probe_tanks(&mut commands, content, pending.presentation(), grid, far);
    commands.remove_resource::<PendingTankAssets>();
    next.set(AppState::Playing);
}

/// Centre-to-centre spacing of the [`spawn_probe_tanks`] grid, metres. Wider than a Tiger's hull is
/// long (8.45 m over the tracks) so the extra bodies never spawn interpenetrating — a stack of
/// overlapping tanks would spend the whole capture resolving contacts and measure nothing useful.
const PROBE_GRID_SPACING_M: f32 = 11.0;

/// Where the grid's near edge sits in the DEFAULT placement: down +Z, in FRONT of the third-person
/// camera. The camera spawns at `(10, 7, −7)` looking at `(10, 1, 5)` (`camera.rs`), i.e. behind
/// Tiger A looking along +Z — so +Z is the only direction that puts probe tanks in frame as well as
/// in the cascade volume. (Tiger B, at z = −12, is behind the camera and always was.) The far
/// placement keeps every one of those properties and only moves the scene: [`PROBE_FAR_NEAR_Z`].
///
/// 48 m rather than "just past the duel" because the ground between z ≈ 5 and z ≈ 30 falls at ~40°:
/// tanks parked there SLIDE (measured — the first layout drifted 6..13 m downhill during a capture,
/// which is both a moving scene and a pile of contact work). z ≈ 46..100 is the valley floor under
/// that slope, flat to a couple of metres across the whole block, and still ~65..115 m from the
/// camera — inside the 150 m cascade envelope with room to spare.
const PROBE_GRID_NEAR_Z: f32 = 48.0;

/// Columns in the block. Fixed rather than square-rooted so the grid keeps the same shape at every
/// count: six columns at 11 m is 55 m across, inside the camera's horizontal FOV at the block's
/// distance, and extra rows recede along the view axis where everything stays in frame.
const PROBE_GRID_COLUMNS: usize = 6;

/// Spawn `OVERMATCH_PROBE_TANKS` EXTRA idle Tigers in a grid in front of the duel, for render/sim
/// cost profiling at caster counts the two-tank duel cannot reach (a 15v15 frame is 30 tanks).
///
/// Dev instrument, not content: unset (or `0`) spawns nothing, which is every normal run. The count
/// is the TOTAL wanted, so `OVERMATCH_PROBE_TANKS=30` yields 30 tanks — the two duel Tigers plus 28
/// here. They are ordinary complete tanks (same body, same tracks, same `RigidBody::Dynamic`), so a
/// count sweep measures the real per-tank cost of both halves; nothing drives them.
///
/// Grid geometry is deliberately dumb — a square-ish block laid out on XZ and dropped onto the
/// surface by the one shared rule (`terrain_grid::spawn_pos`), like every other spawn point.
///
/// # `OVERMATCH_PROBE_FAR=1`: the same block, on the far side of the shoe LOD
///
/// The near block is the RIGHT default and the wrong half of one question. `track::link_view` swaps
/// every shoe down a ladder of certified reductions whose metres follow the view: at the commander
/// field (45°) at 1440p they are 6.9 / 26.8 / 49.8 / 81.6 m, so the near block's 55..99 m of
/// separation puts its belts on the two COARSEST rungs — 252 and 166 triangles a shoe against the
/// authored 1 520 — and moves across a switch as the camera does. That is a measurement of the
/// bottom of the ladder, not of the shoe pool the shadow work is sized against, and not of a fixed
/// rung. This flag is the other placement — the far block, one rung, well past the last switch —
/// and nothing else.
///
/// It moves the WHOLE offline scene, not just the grid, because the camera sits at the duel and the
/// world is finite (the map declares its side — `map::MapManifest`): with the duel
/// where it is, the furthest in-bounds probe on a 1 000 m map is ~490 m away and every shoe is
/// still the base mesh. So the duel
/// translates to
/// [`PROBE_FAR_DUEL_XZ`] and the grid to [`PROBE_FAR_NEAR_Z`] — same spacing, same columns, same
/// shape, same count, 588..633 m of separation instead of 55..99 m.
///
/// One thing the far placement inherently is NOT: a shadow-caster measurement. The cascade envelope
/// is ~150 m, so the far probes cast nothing into it. That is a property of distance, not of this
/// lever — the shadow ladder is what the near placement is for.
fn spawn_probe_tanks(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    grid: Option<&crate::terrain_grid::HeightGrid>,
    far: bool,
) {
    let Some(total) =
        crate::env_parse::<usize>("OVERMATCH_PROBE_TANKS").unwrap_or_else(crate::refuse_start)
    else {
        return;
    };
    // The frame sweep, ablation, night battery, shadow and terrain pricing scripts set this lever.
    let extra = total.saturating_sub(DUEL_SPAWN_XZ.len());
    if extra == 0 {
        return;
    }
    let anchor = duel_spawn_xz(far)[0];
    let (mut nearest, mut furthest) = (f32::INFINITY, 0.0f32);
    for index in 0..extra {
        let xz = probe_spawn_xz(far, index);
        let distance = xz.distance(anchor);
        nearest = nearest.min(distance);
        furthest = furthest.max(distance);
        spawn_tank(
            commands,
            content,
            presentation.clone(),
            Transform::from_translation(crate::terrain_grid::spawn_pos(grid, xz)),
            &format!("Probe Tiger {index}"),
            DUEL_SPAWN_XZ.len() + index,
            false,
        );
    }
    // Self-documenting capture: a frame stream is evidence for a placement, and the placement is
    // this line. Distances are to the CONTROLLED tank's spawn — the camera orbits up to 18 m behind
    // that point (`camera::orbit_camera`'s `ORBIT_FAR`), so the rendered distances are these or
    // greater, which is the direction that keeps a "beyond the LOD swap" claim conservative.
    info!(
        "offline: spawned {extra} probe tanks (OVERMATCH_PROBE_TANKS={total}) — {} placement, grid \
         x {:.0}..{:.0} z {:.0}..{:.0}, {nearest:.0}..{furthest:.0} m from the controlled tank at \
         ({:.0}, {:.0})",
        if far {
            "FAR (OVERMATCH_PROBE_FAR=1)"
        } else {
            "near"
        },
        probe_spawn_xz(far, 0).x,
        probe_spawn_xz(far, extra.min(PROBE_GRID_COLUMNS) - 1).x,
        probe_spawn_xz(far, 0).y,
        probe_spawn_xz(far, extra - 1).y,
        anchor.x,
        anchor.y,
    );
}

/// Stand up the LOD showcase INSTEAD of the duel: the player at the map's edge, and one clamped
/// pair per switch in the shoe chain (capture-plan `LodShowcase` — see [`crate::lod_showcase`]).
///
/// It replaces the duel rather than adding to it because the whole scene is the instrument: a
/// second free Tiger parked at (10, 5) is 490 m off to one side of a range everything else is laid
/// out along, and the Tab possession swap would hand the player a probe whose belt is pinned to one
/// level.
///
/// The layout and the legend both come from `lod_showcase`, which derives them from the chain; this
/// function's whole job is turning poses into tanks and putting the legend next to them in the log.
fn spawn_lod_showcase(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    grid: Option<&crate::terrain_grid::HeightGrid>,
    certificate: &crate::geometry_lod::TankCertificate,
    view: crate::view::ViewProfile,
) {
    // The staging distances are derived at the LIVE profile, which by now has seen the real window:
    // a pair belongs where its switch actually happens in the session it is being judged in.
    let switches = crate::lod_showcase::shoe_switches(certificate, view);
    let deviations = crate::lod_showcase::shoe_deviations(certificate);
    let stage = crate::lod_showcase::Stage::new(crate::terrain_grid::world_extent(grid));
    for line in crate::lod_showcase::legend(&switches, &deviations, stage) {
        info!("{line}");
    }
    for (index, tank) in crate::lod_showcase::layout(&switches, stage)
        .into_iter()
        .enumerate()
    {
        let root = spawn_complete_tank(
            commands,
            content,
            presentation.clone(),
            (
                Transform::from_translation(crate::terrain_grid::spawn_pos(grid, tank.xz))
                    .with_rotation(tank.yaw),
                Name::new(tank.name),
                crate::eased_dynamic_body(),
                identity(index),
            ),
        );
        if tank.controlled {
            commands.entity(root).insert(Controlled);
        }
        if let Some(level) = tank.clamp {
            commands
                .entity(root)
                .insert(crate::lod_showcase::LodClamp(level));
        }
    }
}

/// The match identity of the scenario's tank `index` (from 0), in its first life. Single-player
/// never respawns a tank.
fn identity(index: usize) -> (CombatantId, CombatantLife) {
    (CombatantId(index as u64 + 1), CombatantLife(0))
}

/// Spawn one complete dynamic tank for the local scenario, its tank `index`, and return its root.
pub(super) fn spawn_tank(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    transform: Transform,
    name: &str,
    index: usize,
    controlled: bool,
) -> Entity {
    let root = spawn_complete_tank(
        commands,
        content,
        presentation,
        (
            transform,
            Name::new(name.to_string()),
            crate::eased_dynamic_body(),
            identity(index),
            // The hull marks the local authority records (`hull_marks::plugin`).
            crate::hull_marks::HullMarks::default(),
        ),
    );
    if controlled {
        commands.entity(root).insert(Controlled);
    }
    root
}

/// Move local possession to the next tank and return to third-person view.
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
    // No possession before spawn; multiple controlled tanks violate the possession contract.
    let Some(current) = super::controlled_one(controlled.single()) else {
        return;
    };
    let all: Vec<Entity> = tanks.iter().collect();
    if all.len() < 2 {
        return;
    }
    let idx = all
        .iter()
        .position(|&e| e == current)
        .expect("the controlled tank belongs to this same live Tank query");
    let next = all[(idx + 1) % all.len()];
    if next == current {
        return;
    }
    commands.entity(current).remove::<Controlled>();
    commands.entity(next).insert(Controlled);

    *mode = SightMode::ThirdPerson;
}

#[cfg(test)]
mod target_scenario_tests {
    use bevy::math::Vec2;

    use super::{START_SPOTS, TargetScenario, parse_start};

    /// In the game's damage model, a scenario aim follows the selected functional volume's
    /// pose, independently of the hull origin. A hull-offset mutant would leave the second aim
    /// behind when the component and observing opponent move together.
    #[test]
    fn the_scripted_aim_follows_functional_volume_pose() {
        use super::super::*;
        use super::{AimRole, target_fires};
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins(crate::bake::plugin);
        app.world_mut().run_schedule(Startup);
        app.insert_resource(Time::<Real>::default());
        let names: Vec<_> = app
            .world()
            .resource::<crate::bake::TankBlueprint>()
            .spec
            .volumes
            .iter()
            .filter(|(_, volume)| {
                volume
                    .component()
                    .is_some_and(|component| AimRole::Engine.matches(component))
            })
            .map(|(name, _)| name.clone())
            .collect();
        let mut parts = std::collections::HashMap::new();
        let mut engines = Vec::new();
        for name in names {
            let entity = app.world_mut().spawn(GlobalTransform::IDENTITY).id();
            parts.insert(name, entity);
            engines.push(entity);
        }
        assert!(!engines.is_empty(), "the fixture declares an Engine volume");
        app.world_mut().spawn((
            Tank,
            Controlled,
            crate::tank::SimParts(parts),
            GlobalTransform::IDENTITY,
        ));
        let observer = app
            .world_mut()
            .spawn((
                Tank,
                GlobalTransform::from(Transform::from_xyz(20.0, 0.0, 0.0)),
            ))
            .id();
        let aim = |world: &World| match world
            .get::<crate::command::TankCommand>(observer)
            .unwrap()
            .aim
            .unwrap()
        {
            crate::command::AimIntent::World(at) => at,
            _ => panic!("the scripted aim is in world space"),
        };
        app.world_mut().run_system_once(target_fires).unwrap();
        let first = aim(app.world());
        let delta = Vec3::new(3.0, 1.0, -2.0); // Fixture displacement (m).
        for entity in engines {
            app.world_mut()
                .entity_mut(entity)
                .insert(GlobalTransform::from(Transform::from_translation(delta)));
        }
        app.world_mut()
            .entity_mut(observer)
            .insert(GlobalTransform::from(Transform::from_translation(
                Vec3::new(20.0, 0.0, 0.0) + delta,
            )));
        app.world_mut().run_system_once(target_fires).unwrap();
        assert!((aim(app.world()) - first - delta).length() < 1e-5);
    }

    /// `OVERMATCH_START` READS A NAMED SPOT OR TWO OR THREE FINITE NUMBERS, AND NOTHING ELSE: the
    /// forest cost runs and the house captures set it, and a typo that fell back to the duel would
    /// frame the duel as the spot they asked for, so `start_spot` refuses whatever this refuses.
    #[test]
    fn the_start_lever_reads_a_spot_or_coordinates_and_nothing_else() {
        let (name, x, z, heading) = START_SPOTS[0];
        assert_eq!(parse_start(name), Some((Vec2::new(x, z), heading)));
        assert_eq!(
            parse_start(&name.to_uppercase()),
            Some((Vec2::new(x, z), heading))
        );
        assert_eq!(parse_start("540,570"), Some((Vec2::new(540.0, 570.0), 0.0)));
        assert_eq!(
            parse_start(" -396.32, 195.23 ,229.5"),
            Some((Vec2::new(-396.32, 195.23), 229.5))
        );
        for refused in [
            "", "mudd", "540", "540;570", "1,2,3,4", "nan,0", "0,inf", "540,570,",
        ] {
            assert_eq!(parse_start(refused), None, "{refused}");
        }
    }

    /// `scripts/perf/run-pencam-cost.sh` names its three conditions' scenarios by these words; a
    /// renamed or dropped word fails each of those runs at boot.
    #[test]
    fn the_target_scenario_is_one_of_three_words() {
        assert_eq!(TargetScenario::named("ahead"), Some(TargetScenario::Ahead));
        assert_eq!(TargetScenario::named("fires"), Some(TargetScenario::Fires));
        assert_eq!(TargetScenario::named(" kills"), Some(TargetScenario::Kills));
        for refused in ["", "1", "Ahead", "fire", "ahead,fires"] {
            assert_eq!(TargetScenario::named(refused), None, "{refused}");
        }
    }
}

#[cfg(test)]
mod spawn_contract_tests {
    use std::collections::HashMap;

    use bevy::prelude::{App, ResMut, Resource, Update};

    use super::super::spawn::TankSimSource;
    use crate::bake::{TankBlueprint, TankGeometry};
    use crate::spec::TankSpec;

    #[derive(Resource, Default)]
    struct SourceProbe(bool);

    fn probe_unresolved_handle(source: TankSimSource, mut probe: ResMut<SourceProbe>) {
        // Running at all is the proof: the param validated on the blueprint and colliders alone.
        let _content = source.get();
        probe.0 = true;
    }

    #[test]
    fn sim_source_does_not_require_a_resolved_asset_handle() {
        let spec: TankSpec = crate::provenance::from_shipped_str(
            crate::tank::TANK_SPEC_PATH,
            include_str!("../../assets/tiger_1/tiger_1.tank.ron"),
        )
        .expect("the shipped Tiger spec must parse");
        let geometry = TankGeometry {
            nodes: Vec::new(),
            by_name: HashMap::new(),
            roadwheels: Vec::new(),
            collision_proxies: Vec::new(),
            ballistic_volumes: Vec::new(),
        };

        let track_colliders = crate::track::bump_stop::TrackColliders::new(
            &crate::track::rig_geom::tiger_rig(),
            &spec.track.suspension.params(),
            spec.track.friction(),
        );
        let mut app = App::new();
        app.insert_resource(TankBlueprint::new(
            crate::tank::TANK_SPEC_PATH.to_owned(),
            geometry,
            spec,
        ))
        .insert_resource(track_colliders)
        .init_resource::<SourceProbe>()
        .add_systems(Update, probe_unresolved_handle);
        app.update();

        assert!(
            app.world().resource::<SourceProbe>().0,
            "TankSimSource must read the eager blueprint without an asset-handle argument",
        );
    }
}
