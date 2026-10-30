//! `OVERMATCH_BATTLE=1`: the battle scene, a repeatable 30-tank scene for frame-cost measurement
//! (issue #153) on the offline path. Its subject touches the game's damage model only to stay out
//! of it: every round the scene fires is aimed at the ground on a line that passes no tank, so no
//! tank is knocked out.
//!
//! # The scene
//!
//! On the shipped map, at [`ANCHOR_XZ`]: the controlled tank (the camera tank) at the north
//! forest's edge, and two rows of 14 and 15 tanks facing each other across a strip that runs away
//! from it down [`AXIS_HEADING_DEG`] and widens with depth ([`half_gap`]), so the rows stand from
//! ~70 m to ~325 m down the strip and fire at ranges that grow with depth (DERIVED; the arm line
//! logs the aims' span). The camera is the third-person camera,
//! held on the strip's axis at [`CAMERA_PITCH_DEG`] ([`hold_camera`]). Track prints and the HUD are
//! the game's defaults; the offline path has no debug card.
//!
//! Every tank shuttles on its own straight [`Lane`]: full throttle to the lane's front, full
//! reverse to its back, so its hull always faces the other row (the camera tank's faces down the
//! strip). It fires its main gun on a seeded schedule and its machine guns in seeded bursts, at
//! the aim of its next scheduled shot: a ground point in the strip on its own lane's line, part of
//! the way to the opposite row, or, for the camera tank, far down the strip ([`aims`]).
//!
//! # Driven like a player
//!
//! [`drive`] writes only [`TankCommand`]: throttle, steer, aim, range and the two triggers, after
//! the device gather, as `command::auto_fire` does. Nothing writes sim state. The script reads
//! nothing that names a vehicle: the lanes, the aims and the schedule are the same for any tank.
//!
//! # No knockouts, by construction
//!
//! A round that strikes the terrain stops there (`ballistics::Verdict::Struck`) and arms nothing,
//! so a scene in which no round can reach a hull leaves every tank's HP pools alone and all 30
//! tanks active for any window. A round aimed at the ground is not one that lands there: a hull
//! rocking under acceleration lifts its guns past a far ground aim's shallow line, and such rounds
//! fly on (MEASURED: machine-gun rounds aimed short of the opposite row struck its hulls, ~0.5 % of
//! them). So every line of fire runs on its own lane's line, which passes midway between two of the
//! opposite row's lanes (the rows are staggered by half a spacing), and the line stays clear of
//! every other lane to the map's edge, not just to the aim:
//! `no_line_of_fire_comes_near_another_lane` (the tests below) holds that every line from any
//! point of a lane toward any of its aims, turned up to the tests' `AIM_ERROR_DEG` either way,
//! passes at least half a hull's width plus [`DRIFT_M`] from every other tank's hull on its lane
//! (overruns of up to [`OVERRUN_M`] included). A round that flies long lands beyond the battle,
//! where it may strike a tree or a building; each summary line counts the trees down.
//!
//! The cost: the scene prices muzzle, flight, tracers, terrain impacts, dust, prints and audio,
//! and never a hull strike, a hull mark or the pen cam corner (which has its own transient
//! budget). Respawning was rejected because the offline path never respawns and a respawn changes
//! the population inside the window; targets that cannot be knocked out would be a damage-model
//! override for one scene. Each summary line counts strikes near a tank and knocked-out tanks, so a
//! run that broke the rule says so.
//!
//! # Repeatable
//!
//! [`Plan::new`] is a pure function of the seed (`OVERMATCH_BATTLE_SEED`, default [`DEFAULT_SEED`])
//! and the layout: the lanes, every shot's time and aim and every burst. Its [`Plan::digest`] is
//! logged at arm time, so two runs' logs show they ran one script. The script runs on the virtual
//! clock from [`SETTLE_S`] after the spawn; shots are released on that clock, whatever the physics
//! did, and each lane is a fixed route the tank steers back onto, so paths repeat within the
//! physics' own spread.
//!
//! # Evidence
//!
//! `battle: armed …` at the spawn, then one `battle: window <a>..<b> s real: full|short …` line per
//! [`WINDOW_S`] of `Time<Real>` (the frame stream's clock): `full` when every tank moved at least
//! [`MOVED_MIN_M`] and fired at least one round in that window, no armour strike landed within
//! [`STRUCK_TANK_M`] of a battle tank, nothing was knocked out, and no tank overran or strayed
//! past the bounds the clearance test assumed. `scripts/perf/run-ablation.sh` gates a condition
//! carrying `OVERMATCH_BATTLE=1` on those lines.
//!
//! Mounted by the offline composition (`run_offline`) alone, so the commands it writes never leave
//! the process.

use std::f32::consts::PI;

use avian3d::prelude::{LinearVelocity, Position, Rotation};
use bevy::prelude::*;

use super::model::{Tank, TankRoot, Weapon, WeaponIndex};
use super::spawn::{TankContent, TankPresentation};
use crate::ballistics::{FireShell, Impact, ImpactSurface};
use crate::command::{AimIntent, TankCommand};
use crate::damage::TankKnockedOut;
use crate::map::{MapManifest, Prototype};
use crate::spec::Trigger;
use crate::state::GameplaySet;
use crate::terrain_grid::HeightGrid;
use crate::trees::TreeState;
use crate::view::PlayerView;

/// Is this process running the battle scene? A dev lever of `dev_tools` builds, read by the
/// scenario's spawn (which lays out this scene instead of the duel) and by [`offline_plugin`].
pub(crate) fn enabled() -> bool {
    cfg!(feature = "dev_tools") && crate::env_flag("OVERMATCH_BATTLE", false)
}

/// The script's seed when `OVERMATCH_BATTLE_SEED` is unset (ASSUMED: any value; the issue's
/// number).
const DEFAULT_SEED: u64 = 153;

fn seed() -> u64 {
    crate::env_parse("OVERMATCH_BATTLE_SEED").unwrap_or(DEFAULT_SEED)
}

// ---- Layout: fixture data for the shipped map, DERIVED from its level.json and heightmap ----

/// The camera tank's lane starts here, world XZ: the south edge of the map's north forest, 50
/// trunks within 150 m of the camera, 4 of them within ±36° of the view axis (DERIVED from
/// level.json; the arm line logs both). The lanes stand on ground no steeper than 5.8° over a 6 m
/// baseline (DERIVED from the heightmap), clear of every tree and building
/// (`the_lanes_and_lines_of_fire_stand_clear_of_the_map`).
const ANCHOR_XZ: Vec2 = Vec2::new(-160.0, 470.0);
/// The strip's axis, degrees from `+Z` toward `+X` (fixture data, with [`ANCHOR_XZ`]).
const AXIS_HEADING_DEG: f32 = 165.0;
/// Rows on the strip's right and left, as seen down the axis: 1 + 14 + 15 = 30 tanks.
const ROWS: [usize; 2] = [14, 15];
/// Distance between neighbouring rows of one side, m; the left side is staggered by half of it
/// (ASSUMED: wider than a hull with room for the lines of fire, which the tests check).
const ROW_SPACING_M: f32 = 18.0;
/// The first right-side row's distance down the axis from [`ANCHOR_XZ`], m (ASSUMED).
const FIRST_ROW_M: f32 = 80.0;
/// Half the strip's width at [`FIRST_ROW_M`], m, and its growth per metre of depth: the rows
/// recede into the camera's field and their ranges spread with depth (DERIVED, the arm line logs
/// them).
const FIRST_HALF_GAP_M: f32 = 50.0;
const GAP_OPENING: f32 = 0.35;
/// Length of every lane between its back and its front, m (ASSUMED).
const LANE_M: f32 = 20.0;
/// The camera tank's aims: depths down the axis from [`ANCHOR_XZ`], m, and offsets across it, m
/// (ASSUMED: the long ranges of the mix).
const CAMERA_AIM_DEPTHS_M: [f32; 5] = [150.0, 195.0, 240.0, 285.0, 330.0];
const CAMERA_AIM_OFFSETS_M: [f32; 3] = [-6.0, 0.0, 6.0];
/// A row tank's aims, on its own lane's line: these fractions of the way from its lane's front to
/// the opposite row's fronts (ASSUMED).
const AIM_FRACTIONS: [f32; 4] = [0.25, 0.45, 0.65, 0.85];

/// The camera's pitch, degrees, held for the whole run: the far rows sit just under the horizon
/// (ASSUMED).
const CAMERA_PITCH_DEG: f32 = -10.0;

// ---- Safety bounds the layout is tested against (the tests below) ----

/// How far a tank's centre may run past either end of its lane before the reversal takes, m
/// (ASSUMED). A window whose worst overrun exceeds it is short: the tests' clearance assumed it.
const OVERRUN_M: f32 = 6.0;
/// How far a tank's centre may stray off its lane's line, m (ASSUMED). A window whose worst drift
/// exceeds it is short, likewise.
const DRIFT_M: f32 = 1.5;

// ---- The script ----

/// The script runs this long after the spawn's physics settle, s (TUNED: the tanks drop onto the
/// ground and come to rest well inside it).
const SETTLE_S: f32 = 3.0;
/// The script's length, s: past it nothing fires (ASSUMED: far beyond any measurement).
const HORIZON_S: f32 = 3600.0;
/// A tank's first main-gun shot, s after the script starts, and the gap to each next one, s
/// (ASSUMED: the gap longer than a reload, shorter than a summary window).
const FIRST_SHOT_S: (f32, f32) = (2.0, 7.0);
const SHOT_GAP_S: (f32, f32) = (5.5, 9.0);
/// A tank's first machine-gun burst, s after the script starts; each burst's length; and the
/// pause before the next, s (ASSUMED).
const FIRST_BURST_S: (f32, f32) = (4.0, 12.0);
const BURST_S: (f32, f32) = (1.0, 2.5);
const BURST_PAUSE_S: (f32, f32) = (6.0, 11.0);
/// How long a tank's aim stays on a shot after its trigger time, s: past the fixed tick that
/// consumes the edge (ASSUMED).
const AIM_HOLD_S: f32 = 0.3;

// ---- The driver ----

/// Full steer at this heading error, degrees (TUNED).
const STEER_FULL_DEG: f32 = 20.0;
/// The forward leg steers at a point this far past the lane's front, m, so the approach stays
/// straight (TUNED).
const LOOKAHEAD_M: f32 = 10.0;
/// A leg that has not reached its end in this long reverses anyway, s (ASSUMED).
const LEG_TIMEOUT_S: f32 = 25.0;
/// The deceleration a full reverse throttle is assumed to give, m/s²: a leg ends early by the
/// distance the tank needs to stop at it. Reversing only at the lane's end overran it by up to
/// 12.0 m (MEASURED), about 1 m/s² from the speeds the HUD showed (DERIVED); the bound sits under
/// that, and the worst overrun with it is 0.1 m (MEASURED, the summary lines).
const BRAKE_MPS2: f32 = 0.8;

// ---- The evidence ----

/// One summary line per this much `Time<Real>`, s.
const WINDOW_S: f32 = 10.0;
/// A tank counts as moving in a window when its path in it is at least this long, m (ASSUMED).
const MOVED_MIN_M: f32 = 5.0;
/// An armour strike this close to a battle tank's centre is counted as a strike on it, m: a hull's
/// farthest surface from its centre, with room (ASSUMED). Farther ones are trees'.
const STRUCK_TANK_M: f32 = 6.0;

// Every window of a running script holds a main-gun shot from every tank.
const _: () = assert!(SHOT_GAP_S.1 < WINDOW_S);

/// One tank's lane, world XZ: it spawns at `back` facing `front`, drives forward to `front` and
/// reverses to `back`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lane {
    back: Vec2,
    front: Vec2,
}

impl Lane {
    fn length(&self) -> f32 {
        self.back.distance(self.front)
    }

    fn axis(&self) -> Vec2 {
        (self.front - self.back) / self.length()
    }

    /// Where `at` stands along the lane: 0 at the back, 1 at the front.
    fn progress(&self, at: Vec2) -> f32 {
        (at - self.back).dot(self.axis()) / self.length()
    }

    /// How far `at` stands off the lane's line, m.
    fn drift(&self, at: Vec2) -> f32 {
        self.axis().perp_dot(at - self.back).abs()
    }

    /// The spawn pose's turn about `+Y`: the hull faces its own `−Z`, so a turn of `heading + π`
    /// points it along the heading.
    fn spawn_turn(&self) -> Quat {
        let axis = self.axis();
        Quat::from_rotation_y(axis.x.atan2(axis.y) + PI)
    }
}

/// The strip's frame: `lateral` across the axis (positive to the right looking down it), `depth`
/// down it, both m from [`ANCHOR_XZ`].
fn at(lateral: f32, depth: f32) -> Vec2 {
    let heading = AXIS_HEADING_DEG.to_radians();
    let axis = Vec2::new(heading.sin(), heading.cos());
    let right = Vec2::new(-axis.y, axis.x);
    ANCHOR_XZ + right * lateral + axis * depth
}

/// Half the strip's width at `depth`, m.
fn half_gap(depth: f32) -> f32 {
    FIRST_HALF_GAP_M + GAP_OPENING * (depth - FIRST_ROW_M)
}

/// Every lane, the camera tank's first, each with its aims (world XZ).
fn layout() -> Vec<(Lane, Vec<Vec2>)> {
    let camera = Lane {
        back: at(0.0, 0.0),
        front: at(0.0, LANE_M),
    };
    let camera_aims = CAMERA_AIM_DEPTHS_M
        .iter()
        .flat_map(|&depth| {
            CAMERA_AIM_OFFSETS_M
                .iter()
                .map(move |&lateral| at(lateral, depth))
        })
        .collect();
    let mut lanes = vec![(camera, camera_aims)];
    for (side, rows, stagger) in [(1.0, ROWS[0], 0.0), (-1.0, ROWS[1], -0.5)] {
        for row in 0..rows {
            let depth = FIRST_ROW_M + (row as f32 + stagger) * ROW_SPACING_M;
            let gap = half_gap(depth);
            let lane = Lane {
                back: at(side * (gap + LANE_M / 2.0), depth),
                front: at(side * (gap - LANE_M / 2.0), depth),
            };
            lanes.push((lane, aims(side, depth)));
        }
    }
    lanes
}

/// A row tank's aims, on side `side` (+1 right, −1 left) at `depth`: ground points on its own
/// lane's line, [`AIM_FRACTIONS`] of the way from its front to the opposite row's fronts.
fn aims(side: f32, depth: f32) -> Vec<Vec2> {
    let front = half_gap(depth) - LANE_M / 2.0;
    AIM_FRACTIONS
        .iter()
        .map(|&fraction| at(side * front * (1.0 - 2.0 * fraction), depth))
        .collect()
}

/// SplitMix64: one stream per tank and purpose, keyed by the seed alone.
struct Draws(u64);

impl Draws {
    fn new(seed: u64, tank: usize, purpose: u64) -> Self {
        Self(
            seed ^ (tank as u64 + 1).wrapping_mul(0xa24b_aed4_963e_e407)
                ^ purpose.wrapping_mul(0x9fb2_1c65_1e98_df25),
        )
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[lo, hi)`.
    fn within(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + (hi - lo) * (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One main-gun shot: its time on the script clock, s, and its aim's index in the lane's aims.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shot {
    at: f32,
    aim: usize,
}

/// One machine-gun burst: the trigger held from `from` to `to`, s on the script clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Burst {
    from: f32,
    to: f32,
}

/// One tank's part of the script.
#[derive(Clone, Debug, PartialEq)]
struct TankScript {
    lane: Lane,
    aims: Vec<Vec2>,
    shots: Vec<Shot>,
    bursts: Vec<Burst>,
}

/// The whole script, a pure function of the seed and the layout.
#[derive(Resource, Clone, Debug, PartialEq)]
struct Plan {
    seed: u64,
    tanks: Vec<TankScript>,
}

impl Plan {
    fn new(seed: u64) -> Self {
        let tanks = layout()
            .into_iter()
            .enumerate()
            .map(|(tank, (lane, aims))| {
                let mut draws = Draws::new(seed, tank, 1);
                let mut shots = Vec::new();
                let mut next = draws.within(FIRST_SHOT_S);
                while next < HORIZON_S {
                    shots.push(Shot {
                        at: next,
                        aim: draws.below(aims.len()),
                    });
                    next += draws.within(SHOT_GAP_S);
                }
                let mut draws = Draws::new(seed, tank, 2);
                let mut bursts = Vec::new();
                let mut from = draws.within(FIRST_BURST_S);
                while from < HORIZON_S {
                    let to = from + draws.within(BURST_S);
                    bursts.push(Burst { from, to });
                    from = to + draws.within(BURST_PAUSE_S);
                }
                TankScript {
                    lane,
                    aims,
                    shots,
                    bursts,
                }
            })
            .collect();
        Self { seed, tanks }
    }

    /// FNV-1a over every number the script holds: equal digests, equal scripts.
    fn digest(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut eat = |bits: u32| {
            for byte in bits.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for tank in &self.tanks {
            let lane = tank.lane;
            for v in [lane.back, lane.front].iter().chain(&tank.aims) {
                eat(v.x.to_bits());
                eat(v.y.to_bits());
            }
            for shot in &tank.shots {
                eat(shot.at.to_bits());
                eat(shot.aim as u32);
            }
            for burst in &tank.bursts {
                eat(burst.from.to_bits());
                eat(burst.to.to_bits());
            }
        }
        hash
    }
}

/// Which of the plan's tanks this root is.
#[derive(Component, Clone, Copy, Debug)]
struct BattleSlot(usize);

/// Lay out the battle scene's 30 tanks in place of the duel, the first controlled, and publish the
/// plan. Called by the scenario's spawn when [`enabled`].
pub(super) fn spawn(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    grid: Option<&HeightGrid>,
    manifest: Option<&MapManifest>,
) {
    let plan = Plan::new(seed());
    for (index, tank) in plan.tanks.iter().enumerate() {
        let root = super::scenario::spawn_tank(
            commands,
            content,
            presentation.clone(),
            Transform::from_translation(crate::terrain_grid::spawn_pos(grid, tank.lane.back))
                .with_rotation(tank.lane.spawn_turn()),
            &format!("Battle tank {index}"),
            index,
            index == 0,
        );
        commands.entity(root).insert(BattleSlot(index));
    }
    let ranges = plan
        .tanks
        .iter()
        .flat_map(|tank| {
            let middle = tank.lane.back.lerp(tank.lane.front, 0.5);
            tank.aims.iter().map(move |aim| aim.distance(middle))
        })
        .fold((f32::INFINITY, 0.0f32), |(lo, hi), range| {
            (lo.min(range), hi.max(range))
        });
    let (eye, look) = camera_ground_pose();
    let (near, in_view) = manifest.map_or((0, 0), |manifest| trunks_near(manifest, eye, look));
    info!(
        "battle: armed — seed {}, {} tanks, script digest {:#018x}, aims {:.0}..{:.0} m from the \
         lanes, camera near ({:.0}, {:.0}) heading {AXIS_HEADING_DEG}° pitch {CAMERA_PITCH_DEG}°, \
         {near} trunks within 150 m of it ({in_view} within ±36° of the view axis; DERIVED from \
         the map's placement)",
        plan.seed,
        plan.tanks.len(),
        plan.digest(),
        ranges.0,
        ranges.1,
        eye.x,
        eye.y,
    );
    commands.insert_resource(plan);
}

/// The camera's ground position at the camera tank's spawn and its look direction, world XZ: the
/// orbit body sits `ORBIT_FAR` behind the pivot along the held view axis.
fn camera_ground_pose() -> (Vec2, Vec2) {
    let look = (at(0.0, 1.0) - at(0.0, 0.0)).normalize();
    let back = crate::camera::ORBIT_FAR * CAMERA_PITCH_DEG.to_radians().cos();
    (ANCHOR_XZ - look * back, look)
}

/// Trunks standing within 150 m of `eye`, and those of them within ±36° of `look`.
fn trunks_near(manifest: &MapManifest, eye: Vec2, look: Vec2) -> (usize, usize) {
    let half_field = 36f32.to_radians().cos();
    manifest
        .instances
        .iter()
        .filter(|instance| {
            matches!(
                manifest.prototypes.get(&instance.prototype),
                Some(Prototype::Tree { .. } | Prototype::Fir { .. })
            )
        })
        .map(|instance| Vec2::new(instance.translation[0], instance.translation[2]) - eye)
        .filter(|offset| offset.length() < 150.0)
        .fold((0, 0), |(near, in_view), offset| {
            let seen = offset.normalize_or_zero().dot(look) > half_field;
            (near + 1, in_view + usize::from(seen))
        })
}

/// The battle scene's runtime: the driver, the held camera and the evidence. Adds nothing unless
/// [`enabled`].
pub fn offline_plugin(app: &mut App) {
    if !enabled() {
        return;
    }
    info!("battle: OVERMATCH_BATTLE set — the battle scene replaces the duel");
    app.init_resource::<Runs>()
        .init_resource::<Tally>()
        .add_observer(count_round)
        .add_observer(count_strike)
        .add_systems(
            RunFixedMainLoop,
            drive
                .after(crate::command::gather_commands)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(GameplaySet),
        )
        .add_systems(Update, (hold_camera, (track_motion, report).chain()));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leg {
    Forward,
    Reverse,
}

/// One tank's progress through its part of the script.
#[derive(Clone, Copy, Debug)]
struct Run {
    leg: Leg,
    leg_since: f32,
    /// The next shot whose trigger has not been pulled.
    next_shot: usize,
    /// The shot whose aim the tank holds.
    aim_shot: usize,
    /// The first burst that has not ended.
    burst: usize,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            leg: Leg::Forward,
            leg_since: 0.0,
            next_shot: 0,
            aim_shot: 0,
            burst: 0,
        }
    }
}

/// The script clock's origin, s of `Time<Virtual>`, and every tank's [`Run`].
#[derive(Resource, Default)]
struct Runs {
    start: Option<f32>,
    tanks: Vec<Run>,
}

/// Write every battle tank's command for this frame from the script, after the device gather: the
/// leg's throttle and steer, the held shot's aim and range, the main-gun edge when a shot comes
/// due and the machine-gun level through a burst.
fn drive(
    time: Res<Time<Virtual>>,
    plan: Option<Res<Plan>>,
    grid: Option<Res<HeightGrid>>,
    mut runs: ResMut<Runs>,
    mut tanks: Query<(
        &BattleSlot,
        &mut TankCommand,
        &Position,
        &Rotation,
        &LinearVelocity,
    )>,
) {
    let Some(plan) = plan else {
        return;
    };
    if tanks.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    let start = *runs.start.get_or_insert(now + SETTLE_S);
    let t = now - start;
    runs.tanks.resize(plan.tanks.len(), Run::default());
    for (slot, mut command, position, rotation, velocity) in &mut tanks {
        let (Some(script), Some(run)) = (plan.tanks.get(slot.0), runs.tanks.get_mut(slot.0)) else {
            continue;
        };
        // Lay on the held shot's aim from the spawn on, so the first shot finds the gun laid.
        while script
            .shots
            .get(run.aim_shot)
            .is_some_and(|shot| shot.at + AIM_HOLD_S <= t)
        {
            run.aim_shot += 1;
        }
        if let Some(shot) = script.shots.get(run.aim_shot) {
            let aim = script.aims[shot.aim];
            let ground = grid
                .as_deref()
                .map_or(0.0, |grid| grid.height_at(aim.x, aim.y));
            command.aim = Some(AimIntent::World(Vec3::new(aim.x, ground, aim.y)));
            command.range = aim.distance(script.lane.back.lerp(script.lane.front, 0.5));
        }
        if t < 0.0 {
            command.throttle = 0.0;
            command.steer = 0.0;
            command.fire_secondary = false;
            continue;
        }
        if script
            .shots
            .get(run.next_shot)
            .is_some_and(|shot| shot.at <= t)
        {
            // An edge, latched until a fixed tick consumes it; a hitch that skips past several
            // shots fires one.
            command.fire_primary = true;
            while script
                .shots
                .get(run.next_shot)
                .is_some_and(|shot| shot.at <= t)
            {
                run.next_shot += 1;
            }
        }
        while script
            .bursts
            .get(run.burst)
            .is_some_and(|burst| burst.to <= t)
        {
            run.burst += 1;
        }
        command.fire_secondary = script
            .bursts
            .get(run.burst)
            .is_some_and(|burst| burst.from <= t);

        let here = Vec2::new(position.0.x, position.0.z);
        let along = script.lane.progress(here) * script.lane.length();
        // Reverse where the tank, braking at the assumed rate from its own speed, stops at the
        // lane's end.
        let speed = Vec2::new(velocity.0.x, velocity.0.z).dot(script.lane.axis());
        let stop = speed * speed.abs() / (2.0 * BRAKE_MPS2);
        let timed_out = t - run.leg_since > LEG_TIMEOUT_S;
        let next = match run.leg {
            Leg::Forward if along + stop >= script.lane.length() || timed_out => Leg::Reverse,
            Leg::Reverse if along + stop <= 0.0 || timed_out => Leg::Forward,
            leg => leg,
        };
        if next != run.leg {
            run.leg = next;
            run.leg_since = t;
        }
        (command.throttle, command.steer) = match run.leg {
            Leg::Forward => {
                let toward = script.lane.front + script.lane.axis() * LOOKAHEAD_M;
                (1.0, steer_toward(rotation.0, position.0, toward))
            }
            // Straight back down the line the forward leg laid; the next forward leg corrects.
            Leg::Reverse => (-1.0, 0.0),
        };
    }
}

/// The steer that turns a hull at `from` with `rotation` toward `toward` (world XZ): positive to
/// the right, full at [`STEER_FULL_DEG`] of error. The hull faces its own `−Z`, its right is `+X`.
fn steer_toward(rotation: Quat, from: Vec3, toward: Vec2) -> f32 {
    let local = rotation.inverse() * Vec3::new(toward.x - from.x, 0.0, toward.y - from.z);
    let error = local.x.atan2(-local.z);
    (error / STEER_FULL_DEG.to_radians()).clamp(-1.0, 1.0)
}

/// Hold the player's camera on the strip's axis at [`CAMERA_PITCH_DEG`], every frame: the orbit
/// camera places its body behind the camera tank along whatever rotation this leaves.
fn hold_camera(mut camera: Query<&mut Transform, With<PlayerView>>) {
    let Ok(mut camera) = camera.single_mut() else {
        return;
    };
    // A camera faces its own −Z: a yaw of `heading + π` looks along the heading.
    let rotation = Quat::from_euler(
        EulerRot::YXZ,
        AXIS_HEADING_DEG.to_radians() + PI,
        CAMERA_PITCH_DEG.to_radians(),
        0.0,
    );
    if camera.rotation != rotation {
        camera.rotation = rotation;
    }
}

/// The current window's counts.
#[derive(Resource, Default)]
struct Tally {
    /// The window's index on `Time<Real>`, in [`WINDOW_S`].
    window: Option<u32>,
    last: Vec<Option<Vec2>>,
    path_m: Vec<f32>,
    main: Vec<u32>,
    secondary: Vec<u32>,
    terrain: u32,
    /// Armour strikes within [`STRUCK_TANK_M`] of a battle tank, and the rest (trees).
    on_tanks: u32,
    elsewhere: u32,
    overrun_m: f32,
    drift_m: f32,
}

impl Tally {
    fn reset(&mut self, tanks: usize) {
        self.path_m = vec![0.0; tanks];
        self.main = vec![0; tanks];
        self.secondary = vec![0; tanks];
        self.last.resize(tanks, None);
        self.terrain = 0;
        self.on_tanks = 0;
        self.elsewhere = 0;
        self.overrun_m = 0.0;
        self.drift_m = 0.0;
    }
}

/// Count each round a battle tank fires, by its weapon's trigger.
fn count_round(
    fire: On<FireShell>,
    slots: Query<&BattleSlot>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot)>,
    mut tally: ResMut<Tally>,
) {
    let Some(source) = fire.shooter else {
        return;
    };
    let Ok(slot) = slots.get(source.tank) else {
        return;
    };
    let primary = weapons.iter().any(|(weapon, index, root)| {
        root.0 == source.tank && index.0 == source.weapon && weapon.trigger == Trigger::Primary
    });
    let counts = if primary {
        &mut tally.main
    } else {
        &mut tally.secondary
    };
    if let Some(count) = counts.get_mut(slot.0) {
        *count += 1;
    }
}

/// Count each strike by the surface it met, an armour strike by whether a battle tank's centre is
/// within [`STRUCK_TANK_M`] of it; name the first [`TANK_STRIKES_LOGGED`] strikes on a tank, which
/// the layout promises never happen.
fn count_strike(
    impact: On<Impact>,
    tanks: Query<(&BattleSlot, &Position)>,
    mut tally: ResMut<Tally>,
    mut logged: Local<u32>,
) {
    if impact.surface == ImpactSurface::Terrain {
        tally.terrain += 1;
        return;
    }
    let nearest = tanks
        .iter()
        .map(|(slot, position)| (slot.0, position.0.distance(impact.position)))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let Some((tank, distance)) = nearest.filter(|&(_, distance)| distance <= STRUCK_TANK_M) else {
        tally.elsewhere += 1;
        return;
    };
    tally.on_tanks += 1;
    if *logged < TANK_STRIKES_LOGGED {
        *logged += 1;
        warn!(
            "battle: a {:.1} mm round struck armour at {:.1}, {distance:.1} m from battle tank \
             {tank}",
            impact.caliber * 1000.0,
            impact.position,
        );
    }
}

/// How many strikes on a tank [`count_strike`] describes in the log.
const TANK_STRIKES_LOGGED: u32 = 20;

/// Add each battle tank's path since the last frame, and the worst overrun and drift off its lane.
fn track_motion(
    plan: Option<Res<Plan>>,
    mut tally: ResMut<Tally>,
    tanks: Query<(&BattleSlot, &Position)>,
) {
    let Some(plan) = plan else {
        return;
    };
    if tally.path_m.len() != plan.tanks.len() {
        tally.reset(plan.tanks.len());
    }
    for (slot, position) in &tanks {
        let Some(script) = plan.tanks.get(slot.0) else {
            continue;
        };
        let here = Vec2::new(position.0.x, position.0.z);
        if let Some(last) = tally.last[slot.0] {
            tally.path_m[slot.0] += last.distance(here);
        }
        tally.last[slot.0] = Some(here);
        let progress = script.lane.progress(here);
        let overrun = (progress - 1.0).max(-progress).max(0.0) * script.lane.length();
        tally.overrun_m = tally.overrun_m.max(overrun);
        tally.drift_m = tally.drift_m.max(script.lane.drift(here));
    }
}

/// At each window's end on `Time<Real>`, log what every battle tank did in it, and start the next.
fn report(
    real: Res<Time<Real>>,
    plan: Option<Res<Plan>>,
    mut tally: ResMut<Tally>,
    tanks: Query<Has<TankKnockedOut>, (With<BattleSlot>, With<Tank>)>,
    trees: Query<&TreeState>,
) {
    let Some(plan) = plan else {
        return;
    };
    let window = (real.elapsed_secs() / WINDOW_S) as u32;
    let Some(previous) = tally.window.replace(window) else {
        return;
    };
    if previous == window {
        return;
    }
    let tanks_n = plan.tanks.len();
    let knocked_out = tanks.iter().filter(|&knocked_out| knocked_out).count();
    let moved = tally.path_m.iter().filter(|&&m| m >= MOVED_MIN_M).count();
    let least = tally.path_m.iter().copied().fold(f32::INFINITY, f32::min);
    let fired = (0..tanks_n)
        .filter(|&i| tally.main[i] + tally.secondary[i] > 0)
        .count();
    let full = tanks.iter().count() == tanks_n
        && moved == tanks_n
        && fired == tanks_n
        && tally.on_tanks == 0
        && knocked_out == 0
        && tally.overrun_m <= OVERRUN_M
        && tally.drift_m <= DRIFT_M;
    let down = trees
        .iter()
        .filter(|state| matches!(state, TreeState::Down { .. }))
        .count();
    let from = previous as f32 * WINDOW_S;
    info!(
        "battle: window {from:.0}..{:.0} s real: {} — {moved}/{tanks_n} moved >= {MOVED_MIN_M} m \
         (least {least:.1} m), {fired}/{tanks_n} fired (main {}, secondary {} rounds), strikes on \
         terrain {}, on tanks {}, on other armour {}, knocked out {knocked_out}, trees down {down}, \
         worst overrun {:.1} m, worst drift {:.1} m",
        from + WINDOW_S,
        if full { "full" } else { "short" },
        tally.main.iter().sum::<u32>(),
        tally.secondary.iter().sum::<u32>(),
        tally.terrain,
        tally.on_tanks,
        tally.elsewhere,
        tally.overrun_m,
        tally.drift_m,
    );
    tally.reset(tanks_n);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Half the length and half the width of any hull the layout makes room for, m: a hull up to
    /// 10 m long and 4 m wide (ASSUMED).
    const HULL_HALF_LENGTH_M: f32 = 5.0;
    const HULL_HALF_WIDTH_M: f32 = 2.0;
    /// How far a round's line may turn off its aim in azimuth, degrees: servo lag behind a rocking
    /// hull (ASSUMED). Its error in elevation needs no bound: the line clears every lane to the
    /// map's edge.
    const AIM_ERROR_DEG: f32 = 1.0;
    /// How far a line of fire is followed, m: past the shipped map's diagonal, so a round that
    /// never meets the ground is covered too.
    const LINE_REACH_M: f32 = 2200.0;

    /// Two runs of one seed run one script — the acceptance's "same fire schedule": the lanes,
    /// every shot's time and aim and every burst are the seed's alone, so a mutant that read a
    /// clock, OS randomness or iteration order would change a digest. Another seed is another
    /// schedule on the same lanes.
    #[test]
    fn the_script_is_the_seeds_alone() {
        let plan = Plan::new(DEFAULT_SEED);
        assert_eq!(plan, Plan::new(DEFAULT_SEED));
        assert_eq!(plan.digest(), Plan::new(DEFAULT_SEED).digest());
        let other = Plan::new(DEFAULT_SEED + 1);
        assert_ne!(plan.digest(), other.digest());
        for (a, b) in plan.tanks.iter().zip(&other.tanks) {
            assert_eq!(
                (a.lane, &a.aims),
                (b.lane, &b.aims),
                "the lanes are the layout's"
            );
        }
        assert_eq!(plan.tanks.len(), 1 + ROWS[0] + ROWS[1]);
        for (index, tank) in plan.tanks.iter().enumerate() {
            assert!(!tank.aims.is_empty(), "tank {index} has nothing to aim at");
            assert!(tank.shots.iter().all(|shot| shot.aim < tank.aims.len()));
            assert!(tank.shots.windows(2).all(|pair| pair[0].at < pair[1].at));
            assert!(tank.bursts.windows(2).all(|pair| pair[0].to < pair[1].from));
        }
    }

    fn point_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        p.distance(a + ab * t)
    }

    fn segment_segment(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> f32 {
        let (r, s) = (a1 - a0, b1 - b0);
        let den = r.perp_dot(s);
        if den.abs() > 1e-9 {
            let t = (b0 - a0).perp_dot(s) / den;
            let u = (b0 - a0).perp_dot(r) / den;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                return 0.0;
            }
        }
        point_segment(a0, b0, b1)
            .min(point_segment(a1, b0, b1))
            .min(point_segment(b0, a0, a1))
            .min(point_segment(b1, a0, a1))
    }

    /// `lane` lengthened by `extra` at both ends.
    fn stretched(lane: Lane, extra: f32) -> (Vec2, Vec2) {
        let axis = lane.axis();
        (lane.back - axis * extra, lane.front + axis * extra)
    }

    /// The segment a tank's hull can cover on `lane`: its centre's overruns and half a hull.
    fn body(lane: Lane) -> (Vec2, Vec2) {
        stretched(lane, OVERRUN_M + HULL_HALF_LENGTH_M)
    }

    /// Every line a tank of `lane` can fire toward `aim`, `reach` metres long: from points along
    /// the stretch its centre covers, toward the aim turned up to [`AIM_ERROR_DEG`] either way.
    fn lines_of_fire(lane: Lane, aim: Vec2, reach: f32) -> Vec<(Vec2, Vec2)> {
        let (from, to) = stretched(lane, OVERRUN_M);
        let error = AIM_ERROR_DEG.to_radians();
        let mut lines = Vec::new();
        for step in 0..=8 {
            let p = from.lerp(to, step as f32 / 8.0);
            let toward = (aim - p).normalize();
            for k in -2..=2 {
                let turn = Vec2::from_angle(error * k as f32 / 2.0);
                lines.push((p, p + turn.rotate(toward) * reach));
            }
        }
        lines
    }

    /// No line of fire, followed past the map's edge, passes within [`HULL_HALF_WIDTH_M`] +
    /// [`DRIFT_M`] of another tank's hull on its lane — the invariant that keeps all 30 tanks
    /// active for the whole window (module doc): a round that does not land where it was aimed
    /// still never reaches a hull. Pure layout geometry; a mutant aim, stagger, row spacing or
    /// overrun bound that put one tank in another's line fails here.
    #[test]
    fn no_line_of_fire_comes_near_another_lane() {
        let plan = Plan::new(DEFAULT_SEED);
        for (index, tank) in plan.tanks.iter().enumerate() {
            for &aim in &tank.aims {
                for (start, end) in lines_of_fire(tank.lane, aim, LINE_REACH_M) {
                    for (other, them) in plan.tanks.iter().enumerate() {
                        if other == index {
                            continue;
                        }
                        let (from, to) = body(them.lane);
                        let clear = segment_segment(start, end, from, to);
                        assert!(
                            clear >= HULL_HALF_WIDTH_M + DRIFT_M,
                            "tank {index}'s fire toward {aim} passes {clear:.1} m from tank \
                             {other}'s lane",
                        );
                    }
                }
            }
        }
    }

    /// Steepest slope over a 6 m baseline at `p`, degrees.
    fn slope_deg(grid: &HeightGrid, p: Vec2) -> f32 {
        let h = |x: f32, z: f32| grid.height_at(x, z);
        let dx = (h(p.x + 3.0, p.y) - h(p.x - 3.0, p.y)) / 6.0;
        let dz = (h(p.x, p.y + 3.0) - h(p.x, p.y - 3.0)) / 6.0;
        dx.hypot(dz).atan().to_degrees()
    }

    /// The layout against the shipped map: every lane inside the world, on ground no steeper than
    /// a tank drives without stalling or sliding, clear of every tree and building, every spawn
    /// clear of the others and of the terrain, and every line of fire clear of every tree and
    /// building until it has passed every lane — so no tank is stopped by the scatter and no round
    /// that lands in the battle fells a tree, either of which would change the scene between runs.
    /// Thresholds ASSUMED.
    #[test]
    fn the_lanes_and_lines_of_fire_stand_clear_of_the_map() {
        const MAX_SLOPE_DEG: f32 = 8.0;
        const LANE_TREE_CLEAR_M: f32 = 10.0;
        const LANE_BUILDING_CLEAR_M: f32 = 10.0;
        const FIRE_TREE_CLEAR_M: f32 = 3.0;
        const EDGE_MARGIN_M: f32 = 50.0;
        let grid = crate::terrain_grid::tests::shipped_grid();
        let manifest = crate::map::tests::shipped_manifest();
        let mut trees = Vec::new();
        let mut buildings = Vec::new();
        for instance in &manifest.instances {
            let xz = Vec2::new(instance.translation[0], instance.translation[2]);
            match manifest.prototypes.get(&instance.prototype) {
                Some(Prototype::Tree { .. } | Prototype::Fir { .. }) => trees.push(xz),
                Some(Prototype::Building { min, max }) => {
                    let reach = Vec2::new(
                        min[0].abs().max(max[0].abs()),
                        min[2].abs().max(max[2].abs()),
                    );
                    buildings.push((xz, reach.length() * instance.scale[0]));
                }
                None => {}
            }
        }
        assert!(
            !trees.is_empty() && !buildings.is_empty(),
            "the shipped map's scatter"
        );
        let plan = Plan::new(DEFAULT_SEED);
        let bodies: Vec<(Vec2, Vec2)> = plan.tanks.iter().map(|tank| body(tank.lane)).collect();
        for (index, tank) in plan.tanks.iter().enumerate() {
            let (from, to) = stretched(tank.lane, OVERRUN_M);
            let samples = from.distance(to).ceil() as usize;
            for step in 0..=samples {
                let p = from.lerp(to, step as f32 / samples as f32);
                let edge = grid.half_extent() - EDGE_MARGIN_M;
                assert!(
                    p.x.abs() <= edge && p.y.abs() <= edge,
                    "tank {index}'s lane leaves the map at {p}"
                );
                let slope = slope_deg(&grid, p);
                assert!(
                    slope <= MAX_SLOPE_DEG,
                    "tank {index}'s lane climbs {slope:.1}° at {p}"
                );
            }
            for &tree in &trees {
                let clear = point_segment(tree, from, to);
                assert!(
                    clear >= LANE_TREE_CLEAR_M,
                    "tank {index}'s lane passes {clear:.1} m from a tree at {tree}"
                );
            }
            for &(building, reach) in &buildings {
                let clear = point_segment(building, from, to) - reach;
                assert!(
                    clear >= LANE_BUILDING_CLEAR_M,
                    "tank {index}'s lane passes {clear:.1} m from a building at {building}"
                );
            }
            crate::terrain_grid::tests::assert_spawn_clears_terrain(
                &grid,
                &format!("battle tank {index}"),
                tank.lane.back,
            );
            for (other, them) in plan.tanks.iter().enumerate().skip(index + 1) {
                let apart = tank.lane.back.distance(them.lane.back);
                assert!(
                    apart >= 2.0 * HULL_HALF_LENGTH_M,
                    "tanks {index} and {other} spawn {apart:.1} m apart"
                );
            }
            for &aim in &tank.aims {
                for (start, end) in lines_of_fire(tank.lane, aim, 1.0) {
                    // Followed until it has passed the farthest end of every lane.
                    let reach = bodies
                        .iter()
                        .flat_map(|&(a, b)| [a, b])
                        .map(|point| (point - start).dot(end - start))
                        .fold(0.0f32, f32::max);
                    let end = start + (end - start) * reach;
                    for &tree in &trees {
                        let clear = point_segment(tree, start, end);
                        assert!(
                            clear >= FIRE_TREE_CLEAR_M,
                            "tank {index}'s fire toward {aim} passes {clear:.1} m from a tree at {tree}"
                        );
                    }
                    for &(building, reach) in &buildings {
                        let clear = point_segment(building, start, end) - reach;
                        assert!(
                            clear >= FIRE_TREE_CLEAR_M,
                            "tank {index}'s fire toward {aim} passes {clear:.1} m from a building at {building}"
                        );
                    }
                }
            }
        }
    }

    /// The driver steers toward the side the point is on: the hull faces its own −Z and a positive
    /// steer turns it right (`command::TankCommand::steer`). A sign flip would drive every tank off
    /// its lane.
    #[test]
    fn the_driver_steers_toward_its_point() {
        let facing_z = Quat::from_rotation_y(PI);
        assert!(steer_toward(facing_z, Vec3::ZERO, Vec2::new(0.0, 10.0)).abs() < 1e-5);
        // Facing +Z, the hull's right is world −X.
        assert!(steer_toward(facing_z, Vec3::ZERO, Vec2::new(-3.0, 10.0)) > 0.0);
        assert!(steer_toward(facing_z, Vec3::ZERO, Vec2::new(3.0, 10.0)) < 0.0);
        assert_eq!(
            steer_toward(facing_z, Vec3::ZERO, Vec2::new(-10.0, 0.0)),
            1.0
        );
    }
}
