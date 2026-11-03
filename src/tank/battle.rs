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
//! override for one scene. Each summary line counts strikes on declared tank volumes and knocked-out tanks, so a
//! run that broke the rule says so.
//!
//! # Repeatable
//!
//! [`Plan::new`] is a pure function of the seed ([`SEED`]) and the layout: the lanes, every shot's time and aim and every burst. Its [`Plan::digest`] is
//! logged at arm time, so two runs' logs show they ran one script. The script runs on the virtual
//! clock from [`SETTLE_S`] after the spawn; shots are released on that clock, whatever the physics
//! did, and each lane is a fixed route the tank steers back onto, so paths repeat within the
//! physics' own spread.
//!
//! # Evidence
//!
//! `battle: armed …` at the spawn, then one `battle: window <a>..<b> s real: full|short …` line per
//! [`WINDOW_S`] of `Time<Real>` (the frame stream's clock): `full` when every tank moved at least
//! [`MOVED_MIN_M`] and fired at least one round in that window, no armour strike named a battle
//! tank's declared volume, nothing was knocked out, and no tank overran or strayed
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
use crate::ballistics::{FireShell, Impact, ImpactSurface, ShellStrike};
use crate::command::{AimIntent, TankCommand};
use crate::damage::TankKnockedOut;
use crate::map::{MapManifest, Prototype};
use crate::spec::Trigger;
use crate::state::GameplaySet;
use crate::terrain_grid::HeightGrid;
use crate::trees::TreeState;
use crate::view::PlayerView;

/// Is this process running the battle scene? A dev lever, `OVERMATCH_BATTLE`, read by the
/// scenario's spawn (which lays out this scene instead of the duel) and by [`offline_plugin`].
pub(crate) fn enabled() -> bool {
    crate::env_flag("OVERMATCH_BATTLE", false).unwrap_or_else(crate::refuse_start)
}

/// The script's seed (ASSUMED: any value; the issue's number).
const SEED: u64 = 153;

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

// Scripted-driver tastes stay inline because this offline scenario has no data owner;
// the network Battle mode's data owns match rules, not this driver.
/// Taste: scripted steering response, degrees of heading error for full steer.
const STEER_FULL_DEG: f32 = 20.0;
/// Taste: scripted approach straightness, metres past the lane's front to steer toward.
const LOOKAHEAD_M: f32 = 10.0;
/// Taste: scripted shuttle cadence, seconds before an unfinished leg reverses anyway.
const LEG_TIMEOUT_S: f32 = 25.0;
/// DERIVED stopping advance (m, signed along the lane): full-brake stopping distance plus
/// travel during the command slew. The estimate holds speed during that transition; it is a
/// controller lookahead, and the evidence still checks actual lane overruns.
fn stopping_advance(speed: f32, deceleration: f32, fixed_dt: f32) -> f32 {
    if speed == 0.0 {
        return 0.0;
    }
    if deceleration <= 0.0 {
        // These brakes cannot arrest the downslope load: reverse intent immediately.
        return f32::INFINITY.copysign(speed);
    }
    // DERIVED: one neutral snap tick, then one full stroke in the opposite direction.
    // The script authors raw commands, so the player's tap window is not part of this path.
    let reversal_secs = fixed_dt + 1.0 / crate::track::drive::drive_feel().rise_per_second.0;
    speed * speed.abs() / (2.0 * deceleration) + speed * reversal_secs
}

// ---- The evidence ----

/// One summary line per this much `Time<Real>`, s.
const WINDOW_S: f32 = 10.0;
/// A tank counts as moving in a window when its path in it is at least this long, m (ASSUMED).
const MOVED_MIN_M: f32 = 5.0;
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
    let plan = Plan::new(SEED);
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
                Some(Prototype::Tree { .. })
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
        .add_observer(count_armour_strike)
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
    (time, fixed_time): (Res<Time<Virtual>>, Res<Time<Fixed>>),
    plan: Option<Res<Plan>>,
    grid: Option<Res<HeightGrid>>,
    geometry: Res<crate::track::rig_geom::RigGeom>,
    gear: Res<crate::track::sim::TrackGear>,
    mut runs: ResMut<Runs>,
    mut tanks: Query<(
        &BattleSlot,
        &mut TankCommand,
        &Position,
        &Rotation,
        &LinearVelocity,
        &crate::track::sim::TrackGripEffect,
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
    for (slot, mut command, position, rotation, velocity, grip) in &mut tanks {
        let script = plan
            .tanks
            .get(slot.0)
            .expect("spawn assigns each BattleSlot from this Plan");
        let run = runs
            .tanks
            .get_mut(slot.0)
            .expect("Runs is resized to this Plan before driving");
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
        // Reverse using braking capability and the declared contact cylinders' lane projection.
        let speed = Vec2::new(velocity.0.x, velocity.0.z).dot(script.lane.axis());
        let lane_axis = script.lane.axis();
        let hull_axis = rotation.0.inverse() * Vec3::new(lane_axis.x, 0.0, lane_axis.y);
        let (low, high) = geometry.contact_extent(
            Dir3::new(hull_axis)
                .expect("a unit lane direction rotated into the hull remains a finite direction"),
        );
        let grade = grid.as_deref().map_or(0.0, |grid| {
            let front = here + lane_axis * high;
            let rear = here + lane_axis * low;
            (grid.height_at(front.x, front.y) - grid.height_at(rear.x, rear.y)) / (high - low)
        }); // A slab world has zero grade.
        let grade_sine = grade / (1.0 + grade * grade).sqrt();
        let stop = stopping_advance(
            speed,
            gear.braking_deceleration(
                grade_sine,
                speed.signum(),
                grip.traction_capacity_n,
                -grip
                    .rolling_force
                    .dot(Vec3::new(lane_axis.x, 0.0, lane_axis.y))
                    * speed.signum(),
            ),
            fixed_time.timestep().as_secs_f32(),
        );
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
    let mut camera = match camera.single_mut() {
        Ok(camera) => camera,
        // The player's view is not yet declared during scene startup.
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => return,
        Err(error @ bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("the offline root declares one PlayerView: {error}")
        }
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
    /// In the game's damage model, resolved faces on battle-owned volumes and other armour.
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
    // Unkeyed tool/environment emissions have no battle shooter to count.
    let Some(source) = fire.shooter else {
        return;
    };
    // Emissions by tanks outside this scripted scene are not battle evidence.
    let Ok(slot) = slots.get(source.tank) else {
        return;
    };
    let (weapon, _, _) = weapons
        .iter()
        .find(|(_, index, root)| root.0 == source.tank && index.0 == source.weapon)
        .expect("a battle tank's FireShell names one of its spawn-built weapons");
    let primary = weapon.trigger == Trigger::Primary;
    let counts = if primary {
        &mut tally.main
    } else {
        &mut tally.secondary
    };
    *counts
        .get_mut(slot.0)
        .expect("battle evidence is sized to Plan before its scheduled fire begins") += 1;
}

/// In the game's damage model, terrain impacts have no volume identity and are counted here.
/// Armour ownership comes from `ShellStrike`, the resolved face's identity-bearing event.
fn count_strike(impact: On<Impact>, mut tally: ResMut<Tally>) {
    if impact.surface == ImpactSurface::Terrain {
        tally.terrain += 1;
    }
}

/// In the game's damage model, classify each resolved armour face by its declared volume owner.
/// A nearby environment face is not a tank face; distance from the hull origin is irrelevant.
fn count_armour_strike(
    strike: On<ShellStrike>,
    owners: Query<Option<&crate::damage::VolumeOf>>,
    tanks: Query<Option<&BattleSlot>>,
    mut tally: ResMut<Tally>,
    mut logged: Local<u32>,
) {
    // Environment volumes and tanks outside this scripted scene have no battle slot.
    let owner = owners
        .get(strike.struck)
        .expect("a resolved face's volume lives through its synchronous ShellStrike observers");
    let tank = owner.and_then(|owner| {
        tanks
            .get(owner.tank())
            .expect("VolumeOf names a live relationship owner")
    });
    let Some(slot) = tank else {
        tally.elsewhere += 1;
        return;
    };
    tally.on_tanks += 1;
    if *logged < TANK_STRIKES_LOGGED {
        *logged += 1;
        warn!(
            "battle: a {:.1} mm round struck battle tank {} volume {:?} at {:.1}",
            strike.caliber * 1000.0,
            slot.0,
            strike.struck,
            strike.at
        );
    }
}

/// ASSUMED log-volume bound for game-model strikes on battle tanks.
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
        let script = plan
            .tanks
            .get(slot.0)
            .expect("spawn assigns each BattleSlot from this Plan");
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
        let plan = Plan::new(SEED);
        assert_eq!(plan, Plan::new(SEED));
        assert_eq!(plan.digest(), Plan::new(SEED).digest());
        let other = Plan::new(SEED + 1);
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
        let plan = Plan::new(SEED);
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
                Some(Prototype::Tree { .. }) => trees.push(xz),
                Some(Prototype::Building { min, max, .. }) => {
                    let reach = Vec2::new(
                        min[0].abs().max(max[0].abs()),
                        min[2].abs().max(max[2].abs()),
                    );
                    buildings.push((xz, reach.length() * instance.scale[0]));
                }
                Some(Prototype::Structure { asset }) => {
                    let (min, max) = manifest
                        .buildings
                        .get(asset)
                        .expect("the manifest loads every building it places")
                        .bounds();
                    let reach =
                        Vec2::new(min.x.abs().max(max.x.abs()), min.z.abs().max(max.z.abs()));
                    buildings.push((xz, reach.length() * instance.scale[0]));
                }
                None => {}
            }
        }
        assert!(
            !trees.is_empty() && !buildings.is_empty(),
            "the shipped map's scatter"
        );
        let plan = Plan::new(SEED);
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

#[cfg(test)]
mod ownership_tests {
    use super::*;

    /// The script must anticipate less travel with stronger brakes, preserve reverse symmetry,
    /// and request reversal immediately on a grade with no finite stopping distance.
    #[test]
    fn stopping_lookahead_uses_current_speed_and_braking_capability() {
        let fixed_dt = 1.0 / 64.0; // Fixture simulation tick.
        let weak = stopping_advance(4.0, 1.0, fixed_dt);
        let strong = stopping_advance(4.0, 2.0, fixed_dt);
        assert!(strong < weak);
        assert_eq!(stopping_advance(-4.0, 1.0, fixed_dt), -weak);
        assert_eq!(stopping_advance(0.0, 0.0, fixed_dt), 0.0);
        assert_eq!(stopping_advance(1.0, -0.1, fixed_dt), f32::INFINITY);
    }

    /// The scripted driver prices live capacity and delivered rolling when choosing reversal.
    /// Dropping either input or reversing the rolling projection fails this caller witness.
    /// FIXTURE: one slab-lane pose between two capacities' stopping points. Approach speed
    /// fits the lane at any declared brake strength and command slew.
    #[cfg(feature = "bitprobe")]
    #[test]
    fn scripted_reversal_reads_live_contact_capacity() {
        use crate::track::sim::{TrackGear, TrackGripEffect};
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins((crate::bake::plugin, crate::track::sim::sim_plugin))
            .init_resource::<Time<Virtual>>()
            .init_resource::<Time<Fixed>>();
        app.world_mut().run_schedule(Startup);
        app.world_mut().run_schedule(PreUpdate);
        let plan = Plan::new(SEED);
        let lane = plan.tanks[0].lane;
        let gear = app.world().resource::<TrackGear>();
        let mass = gear.landing_mass();
        let available = 2.0 * gear.trans().brake_capacity_n;
        let capacities = [available * 0.1, available * 0.4]; // FIXTURE brake-limited shares.
        let dt = app
            .world()
            .resource::<Time<Fixed>>()
            .timestep()
            .as_secs_f32();
        let transition = dt + 1.0 / crate::track::drive::drive_feel().rise_per_second.0;
        // FIXTURE: stopping and command-transition travel each fit a quarter lane.
        let speed = 1.0_f32
            .min((capacities[0] * lane.length() / (2.0 * mass)).sqrt())
            .min(lane.length() / (4.0 * transition));
        let stops = capacities.map(|capacity| {
            stopping_advance(
                speed,
                gear.braking_deceleration(0.0, 1.0, capacity, 0.0),
                dt,
            )
        });
        let remaining = (stops[0] + stops[1]) / 2.0;
        let axis = lane.axis();
        let point = lane.front - axis * remaining;
        app.insert_resource(plan);
        for (capacity, rolling, expected_throttle) in [
            (capacities[0], 0.0, -1.0),
            (capacities[1], 0.0, 1.0),
            (capacities[0], capacities[1] - capacities[0], 1.0),
        ] {
            app.insert_resource(Runs {
                start: Some(0.0),
                tanks: Vec::new(),
            });
            let tank = app
                .world_mut()
                .spawn((
                    BattleSlot(0),
                    TankCommand::default(),
                    Position(Vec3::new(point.x, 0.0, point.y)),
                    Rotation(lane.spawn_turn()),
                    LinearVelocity(Vec3::new(axis.x, 0.0, axis.y) * speed),
                    TrackGripEffect {
                        traction_capacity_n: capacity,
                        rolling_force: -Vec3::new(axis.x, 0.0, axis.y) * rolling,
                        ..Default::default()
                    },
                ))
                .id();
            app.world_mut().run_system_once(drive).unwrap();
            assert_eq!(
                app.world().get::<TankCommand>(tank).unwrap().throttle,
                expected_throttle,
                "the caller must price the reported {capacity} N capacity"
            );
            app.world_mut().despawn(tank);
        }
    }

    /// In the game's damage model, evidence follows the struck volume's owner even when the
    /// face is far from that owner's origin; a nearby environment face stays environment.
    #[test]
    fn resolved_ownership_not_proximity_classifies_armour_strikes() {
        let mut app = App::new();
        app.init_resource::<Tally>()
            .add_observer(count_armour_strike);
        let tank = app
            .world_mut()
            .spawn((BattleSlot(0), Position(Vec3::ZERO)))
            .id();
        let owned = app.world_mut().spawn(crate::damage::VolumeOf(tank)).id();
        let environment = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(crate::hull_marks::tests::strike(
            owned,
            Vec3::X * 100.0,
            crate::ballistics::Verdict::Stopped,
        ));
        app.world_mut().trigger(crate::hull_marks::tests::strike(
            environment,
            Vec3::ZERO,
            crate::ballistics::Verdict::Stopped,
        ));
        let tally = app.world().resource::<Tally>();
        assert_eq!((tally.on_tanks, tally.elsewhere), (1, 1));
    }
}
