//! The scripted session and the shared `SPIKE_*` environment parser.
//! Subject: the game's scripted inputs and damage-model instruments, game state only.
//!
//! A scripted session has one input: the RON file `OVERMATCH_HARNESS=<script>.ron` names, one
//! [`Script`], whose struct docs are the grammar's one home. On a client it selects the scripted
//! client (`net::client::run`: GPU-less unless `OVERMATCH_WINDOW` names a window,
//! `crate::run_window`), which plays the script against the server and exits once it has; the
//! dedicated server reads the same variable for [`Script::spawn_pose`]. `scripts/harness/idle.ron`
//! is a worked script, checked by a test.
//!
//! The other levers stay variables. Flags accept `1/true/on/yes` or `0/false/off/no/empty`,
//! case-insensitively. Text levers treat empty, zero, false and off as disabled. Numbers preserve
//! zero; malformed numbers, unknown flag words and non-UTF-8 values refuse startup.
//!
//! | Variable / flag | Kind and default | Effect |
//! |---|---|---|
//! | `OVERMATCH_BOT` | count; off, bare = `1` | Server: that many ownerless circling, firing bot tanks. |
//! | `OVERMATCH_HARNESS` | path; off | The scripted session ([`Script`]). |
//! | `OVERMATCH_SERVER_BIND` | IPv4; every interface | Server: bind the UDP socket to this address only. |
//! | `OVERMATCH_SERVER_STATS_S` | positive seconds; unset is off | Server: one `server_stats {json}` line on stdout per period (`net::server_stats`). |
//! | `SPIKE_BURST_MS` | milliseconds; `0` | Periodic receive stall: every burst-window packet delivers at the window's end. |
//! | `SPIKE_BURST_PERIOD_TICKS` | ticks; `0` | Burst recurrence period; both burst values set activates the seeded conditioner. |
//! | `SPIKE_COST_TRACE` | path; off | Role-qualified fixed-tick cost JSONL. |
//! | `SPIKE_WORKLOAD_READY` | path; off | Cost runner start file; scripts and cost warmup wait for every input slot. |
//! | `SPIKE_COST_WARMUP` | ticks; `384` | Played ticks skipped before recording, from combat release. |
//! | `SPIKE_FRAME_COST` | path; off | Role-qualified raw per-frame wall-clock JSONL (client). |
//! | `SPIKE_JITTER_MS` | milliseconds; `0` | Receive-conditioner jitter. |
//! | `SPIKE_JITTER_SEED` | `u64`; off | Make active receive jitter reproducible by packet index. |
//! | `SPIKE_LATENCY_MS` | milliseconds; `0` | Receive-conditioner latency. |
//! | `SPIKE_RENDER_COST` | path; off | Role-qualified per-render-pass CPU/GPU-time JSONL (client). |
//! | `SPIKE_SHOT_TRACE` | path; off | Role-qualified shot-lifecycle JSONL. |
//! | `SPIKE_TRACE` | path; off | Role-qualified jitter/divergence JSONL. |
//! | `SPIKE_TRACE_SIM_FIELDS` | flag; off | Add raw sim fields to trace tick rows. |
//!
//! Seeded jitter is active only when latency and jitter are both nonzero. For example,
//! `SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_JITTER_SEED=1` gives every received packet the
//! same deterministic delay offset on every run with the same packet sequence. The seed controls
//! only those offsets: it is not simulation state and does not enter messages, replication, or the
//! protocol fingerprint. Without `SPIKE_JITTER_SEED`, the client keeps Lightyear's stock unseeded
//! receive conditioner.
//!
//! The burst pair (`SPIKE_BURST_MS=60 SPIKE_BURST_PERIOD_TICKS=33` reproduces the measured field
//! link: 60 ms stalls every ~33 ticks, which a ping EWMA reads as ~3 ms jitter) also activates the
//! seeded conditioner on its own — latency, jitter, and seed all default to zero, so a pure-burst
//! run needs no other variables.

use core::time::Duration;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bevy::prelude::*;
use lightyear::core::time::Instant;
use lightyear::link::RecvPayload;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Link, LocalTimeline, LocalTimelineSync};
use serde::Deserialize;

use super::battle::BattleState;
use crate::command::{AimIntent, TankCommand};

/// The variable that names the script file.
const VARIABLE: &str = "OVERMATCH_HARNESS";

/// A scripted session, read from the file [`VARIABLE`] names. Every field is optional; a missing
/// file, a RON or serde error, an unknown field or a value outside its field's domain fails the
/// boot naming the file and the field (ADR-0011). Optional fields may be written bare
/// (`ticks: 256`) or as `Some(…)`. The client ignores [`Script::spawn_pose`]; the server reads
/// nothing else.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Script {
    /// Run length in released combat ticks; tick zero is the input slot's first released tick.
    /// The client exits once it has played them.
    /// Default 600, or 1280 on the [`Drive::Straight`] course.
    pub(crate) ticks: Option<u32>,
    /// What the tank does ([`Drive`]). Default [`Drive::Arc`].
    pub(crate) drive: Drive,
    /// Where every servo aims ([`Aim`]), from tick 0.
    pub(crate) aim: Aim,
    /// The dialed range for the aim solution's superelevation, metres; finite and positive.
    /// Default 800. A close armour target wants a small one, so the rounds shoot flat into the
    /// plate.
    pub(crate) range_m: f32,
    /// The tick of the one primary click of [`Drive::Arc`] and [`Drive::Straight`], and the first
    /// click of a [`Drive::Fire`] primary. Default 300, mid-drive.
    pub(crate) fire_tick: u32,
    /// Ticks between a [`Drive::Fire`] primary's clicks; zero clicks none. Default 256 (DERIVED:
    /// four seconds at the fixed 64 Hz).
    pub(crate) fire_interval: u32,
    /// The server's spawned tank starts here instead of its lane spawn ([`SpawnPose`]).
    pub(crate) spawn_pose: Option<SpawnPose>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            ticks: None,
            drive: Drive::Arc,
            aim: Aim::default(),
            range_m: 800.0,
            fire_tick: 300,
            fire_interval: 256,
            spawn_pose: None,
        }
    }
}

/// What the scripted tank does. One enum, so two workloads cannot both be asked for.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub(crate) enum Drive {
    /// The default, exercising the real sim: 2 s idle (the rig binds, the hull settles onto the
    /// belt), 4 s at full throttle with steer 0.3 (belt forces and skid steer), then coast to rest;
    /// one primary click at `fire_tick` (fire, recoil, reload).
    #[default]
    Arc,
    /// Dead straight at full throttle for ~15 s: from spawn it crosses the speed bump (z≈−70) and
    /// the washboard (z≈−82…−90) on the spawn axis, which the arc never leaves the flat pad for.
    /// One primary click at `fire_tick`.
    Straight,
    /// Never drive, never fire: the tank sits on whatever contact it spawned onto (with
    /// `spawn_pose`, a known resting contact), with no drive input perturbing it.
    Idle,
    /// Stationary fire, so the only difference from [`Drive::Idle`] is the fire itself: hold the
    /// secondary trigger from a ~3 s warmup to the end, and click the primary every
    /// `fire_interval` from `fire_tick`. At least one of the two; the aim is a point.
    Fire {
        #[serde(default)]
        primary: bool,
        #[serde(default)]
        secondary: bool,
    },
}

/// Where the scripted servos aim, in hull-local metres.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) enum Aim {
    /// Chase this point, finite. Default `(200, 0, -800)`: downrange into open ground (the common
    /// spray case); at a stationary target's hull the rounds strike armour instead.
    Point((f32, f32, f32)),
    /// Sweep around the tank at ~1.3 rad/s, recommitted every tick, as a player scanning with the
    /// mouse does. Not with [`Drive::Fire`].
    Sweep,
}

impl Default for Aim {
    fn default() -> Self {
        Self::Point((200.0, 0.0, -800.0))
    }
}

/// A spawn pose checked and normalized at the script edge. Downstream spawns hold the parsed
/// position and unit rotation; they never normalize operator input again.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(try_from = "AuthoredSpawnPose")]
pub(crate) struct SpawnPose {
    pos: Vec3,
    rot: Quat,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoredSpawnPose {
    pos: (f32, f32, f32),
    rot: (f32, f32, f32, f32),
}

impl TryFrom<AuthoredSpawnPose> for SpawnPose {
    type Error = ScriptError;

    fn try_from(raw: AuthoredSpawnPose) -> Result<Self, Self::Error> {
        let pos = Vec3::from(raw.pos);
        let rot = Vec4::from(raw.rot);
        if !pos.is_finite() || !rot.is_finite() || rot.abs().max_element() == 0.0 {
            return Err(ScriptError::Invalid {
                field: "spawn_pose",
                problem: "use a finite position and a finite nonzero rotation",
            });
        }
        // Scaling before normalization avoids squared-length overflow and underflow, including
        // finite hand-entered quaternions with very large or very small magnitudes.
        let unit = (rot / rot.abs().max_element()).normalize();
        Ok(Self {
            pos,
            rot: Quat::from_xyzw(unit.x, unit.y, unit.z, unit.w),
        })
    }
}

#[derive(Debug)]
pub(crate) enum ScriptError {
    Syntax(ron::error::SpannedError),
    Invalid {
        field: &'static str,
        problem: &'static str,
    },
}
impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(error) => write!(f, "{error}"),
            Self::Invalid { field, problem } => write!(f, "{field}: {problem}"),
        }
    }
}
impl std::error::Error for ScriptError {}

#[derive(Debug)]
enum ScriptLoadError {
    Read(std::io::Error),
    Parse(ScriptError),
}
impl std::fmt::Display for ScriptLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => write!(f, "{error}"),
            Self::Parse(error) => write!(f, "{error}"),
        }
    }
}
impl std::error::Error for ScriptLoadError {}

impl Script {
    /// The script `text` holds, validated; the error names the field.
    fn parse(text: &str) -> Result<Self, ScriptError> {
        let script: Self = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(ScriptError::Syntax)?;
        script.validate()?;
        Ok(script)
    }

    fn load(path: &Path) -> Result<Self, ScriptLoadError> {
        let text = std::fs::read_to_string(path).map_err(ScriptLoadError::Read)?;
        Self::parse(&text).map_err(ScriptLoadError::Parse)
    }

    fn validate(&self) -> Result<(), ScriptError> {
        if let Drive::Fire {
            primary: false,
            secondary: false,
        } = self.drive
        {
            return Err(ScriptError::Invalid {
                field: "drive",
                problem: "a Fire workload fires the primary, the secondary or both",
            });
        }
        match self.aim {
            Aim::Point(point)
                if crate::command::ParsedAimIntent::try_from(AimIntent::HullLocal(Vec3::from(
                    point,
                )))
                .is_err() =>
            {
                return Err(ScriptError::Invalid {
                    field: "aim",
                    problem: "use a finite point inside the derived AIM_LIMIT envelope",
                });
            }
            Aim::Sweep if matches!(self.drive, Drive::Fire { .. }) => {
                return Err(ScriptError::Invalid {
                    field: "aim",
                    problem: "a Fire workload needs a point",
                });
            }
            _ => {}
        }
        if !(self.range_m.is_finite() && self.range_m > 0.0) {
            return Err(ScriptError::Invalid {
                field: "range_m",
                problem: "use a finite positive range",
            });
        }
        Ok(())
    }
}

/// This process's script, read once from the file [`VARIABLE`] names; `None` when it is unset.
///
/// An unreadable or invalid script refuses startup with its path and typed cause.
pub(crate) fn script() -> Option<&'static Script> {
    static SCRIPT: OnceLock<Option<Script>> = OnceLock::new();
    SCRIPT
        .get_or_init(|| {
            let path = PathBuf::from(std::env::var_os(VARIABLE)?);
            let script = Script::load(&path).unwrap_or_else(|error| {
                refuse_start(format!("{VARIABLE}={}: {error}", path.display()))
            });
            Some(script)
        })
        .as_ref()
}

/// The scripted client's state: a fixed-tick counter playing its [`Script`], then a clean exit once
/// the script has played out ([`simulate_watchdog`]).
#[derive(Resource)]
pub(crate) struct SimulateInput {
    pub(crate) ticks: u32,
    /// Script length — exit after this many ticks.
    pub(crate) total: u32,
    drive: Drive,
    aim: Aim,
    range: f32,
    fire_tick: u32,
    fire_interval: u32,
    clock: ScriptClock,
    start: crate::cost::WorkloadStart,
}

/// The launch phases in order, each a state the script really passes through. Waiting is bounded
/// from launch; execution is bounded from its first released input.
#[derive(Debug)]
enum ScriptClock {
    /// No input slot yet: not connected, or the own tank not replicated.
    AwaitingSlot { deadline: Duration },
    /// The slot is held; wait for synchronized combat and any runner-owned start signal.
    AwaitingRelease { deadline: Duration },
    Released {
        at: Duration,
        first_fire_logged: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum ScriptTimeout {
    Release,
    Completion,
}

// ASSUMED startup and scheduling slack, shared by both wall-clock bounds.
const WATCHDOG_SLACK: Duration = Duration::from_secs(20);

impl SimulateInput {
    /// Called after `LogPlugin` has built, so the script it logs reaches the run's log.
    pub(crate) fn new(script: &Script) -> Self {
        info!("harness: {script:?}");
        let course = script.drive == Drive::Straight;
        Self {
            ticks: 0,
            total: script.ticks.unwrap_or(if course { 1280 } else { 600 }),
            drive: script.drive,
            aim: script.aim,
            range: script.range_m,
            fire_tick: script.fire_tick,
            fire_interval: script.fire_interval,
            start: crate::cost::WorkloadStart::from_env(),
            clock: ScriptClock::AwaitingSlot {
                deadline: WATCHDOG_SLACK,
            },
        }
    }

    /// The input slot arrived. Its launch deadline is unchanged and logged once.
    fn hold_slot(&mut self, now: Duration) {
        if let ScriptClock::AwaitingSlot { deadline } = self.clock {
            self.clock = ScriptClock::AwaitingRelease { deadline };
            info!("harness: input slot app_s={:.3}", now.as_secs_f64());
        }
    }

    /// Combat tick zero: the first input the synchronized timeline files.
    fn release(&mut self, now: Duration) -> bool {
        match self.clock {
            ScriptClock::AwaitingSlot { .. } => {
                unreachable!("the input slot is held before its script is released")
            }
            ScriptClock::AwaitingRelease { .. } => {
                self.clock = ScriptClock::Released {
                    at: now,
                    first_fire_logged: false,
                };
                true
            }
            ScriptClock::Released { .. } => false,
        }
    }

    fn timeout(&self, now: Duration, tick: Duration) -> Option<ScriptTimeout> {
        match self.clock {
            ScriptClock::AwaitingSlot { deadline } | ScriptClock::AwaitingRelease { deadline }
                if now > deadline =>
            {
                Some(ScriptTimeout::Release)
            }
            // DERIVED release bound: script length × fixed timestep + scheduling slack. It keeps
            // running if a later hold or a lost input slot prevents the script from completing.
            ScriptClock::Released { at, .. } if now > at + tick * self.total + WATCHDOG_SLACK => {
                Some(ScriptTimeout::Completion)
            }
            _ => None,
        }
    }

    /// The scripted secondary trigger's held window: from the warmup boundary to the end of the
    /// run.
    fn holding_secondary(t: u32) -> bool {
        // DERIVED: 192 combat ticks / 64 Hz = 3 s for settling and aim before the held trigger.
        // The first secondary input is the following combat tick.
        const FIRE_WARMUP: u32 = 192;
        t > FIRE_WARMUP
    }

    /// Whether the scripted primary trigger is clicked on tick `t`: every `fire_interval` ticks
    /// from `fire_tick`, never when the interval is zero.
    fn clicking_primary(&self, t: u32) -> bool {
        self.fire_interval != 0
            && t >= self.fire_tick
            && (t - self.fire_tick).is_multiple_of(self.fire_interval)
    }

    /// The point every servo chases on tick `t`.
    fn aim_at(&self, t: u32) -> AimIntent {
        AimIntent::HullLocal(match self.aim {
            Aim::Point(point) => Vec3::from(point),
            Aim::Sweep => {
                let theta = 0.02 * t as f32;
                Vec3::new(800.0 * theta.sin(), 0.0, -800.0 * theta.cos())
            }
        })
    }
}

/// Release evidence: `app_s` retains the watchdog's wall clock; `frame_s` reads the same
/// `Time<Real>::elapsed_secs_f64` as frame rows and surface marks, excluding initialization
/// before the first update. Frame runners must use `frame_s` as their window origin.
fn release_stamp(client_tick: u32, now: Duration, time: &Time<Real>) -> String {
    format!(
        "harness: released script_tick=0 client_tick={client_tick} app_s={:.3} frame_s={:.3}",
        now.as_secs_f64(),
        time.elapsed_secs_f64()
    )
}

/// Headless simulate mode: write the scripted `TankCommand` into the lightyear `ActionState` slot
/// each tick. Whole-state snapshot per tick, no devices.
pub(crate) fn buffer_input(
    mut sim: ResMut<SimulateInput>,
    time: Res<Time<Real>>,
    timeline: Res<LocalTimeline>,
    sync: Res<LocalTimelineSync>,
    battles: Query<&BattleState>,
    mut slots: Query<&mut ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    use bevy::ecs::query::QuerySingleError;
    let mut state = match slots.single_mut() {
        Ok(state) => state,
        // Before tank replication (or between lives), there is no local input slot.
        Err(QuerySingleError::NoEntities(_)) => return,
        Err(QuerySingleError::MultipleEntities(_)) => {
            panic!("a scripted client controls at most one input slot")
        }
    };
    let now = time.startup().elapsed();
    sim.hold_slot(now);
    let battle = match battles.single() {
        Ok(battle) => battle,
        // The tank and Battle snapshots may arrive separately. Wait for the release fact.
        Err(QuerySingleError::NoEntities(_)) => {
            state.0 = TankCommand::default();
            return;
        }
        Err(QuerySingleError::MultipleEntities(_)) => {
            panic!("a peer holds at most one replicated Battle")
        }
    };
    // The same predicate as `battle::hold_commands`; no local countdown releases a command.
    // Lightyear files a slot's command only from a synchronized timeline (`buffer_action_state`
    // reads `SyncedLocalTimeline`), so a tick spent before sync would never reach the wire:
    // combat tick zero waits for both.
    if !battle.is_combat() || !sync.is_synced() || !sim.start.ready() {
        state.0 = TankCommand::default();
        return;
    }
    if sim.release(now) {
        info!("{}", release_stamp(timeline.tick().0, now, &time));
    }
    if sim.ticks >= sim.total {
        return;
    }
    let t = sim.ticks;
    sim.write(t, &mut state.0);
    if (state.0.fire_primary || state.0.fire_secondary)
        && let ScriptClock::Released {
            first_fire_logged, ..
        } = &mut sim.clock
        && !*first_fire_logged
    {
        *first_fire_logged = true;
        info!(
            "harness: first fire input script_tick={t} client_tick={} app_s={:.3}",
            timeline.tick().0,
            now.as_secs_f64()
        );
    }
    sim.ticks += 1;
}

impl SimulateInput {
    /// Tick `t`'s scripted drive, aim and triggers into `command`.
    fn write(&self, t: u32, command: &mut TankCommand) {
        // Throttle and steer over the drive's window: the arc steers to exercise skid steer, the
        // courses run dead straight along the spawn axis.
        let (window, throttle, steer) = match self.drive {
            Drive::Arc => (128..384, 1.0, 0.3),
            Drive::Straight => (128..1088, 1.0, 0.0),
            Drive::Idle | Drive::Fire { .. } => (0..0, 0.0, 0.0),
        };
        let driving = window.contains(&t);
        command.throttle = if driving { throttle } else { 0.0 };
        command.steer = if driving { steer } else { 0.0 };
        command.aim = Some(self.aim_at(t));
        command.range = self.range;
        (command.fire_primary, command.fire_secondary) = match self.drive {
            Drive::Fire { primary, secondary } => (
                primary && self.clicking_primary(t),
                secondary && Self::holding_secondary(t),
            ),
            Drive::Arc | Drive::Straight => (t == self.fire_tick, false),
            // A recoil impulse would disturb the resting contact under study.
            Drive::Idle => (false, false),
        };
    }
}

/// Exit once the released script has played out; separately bound startup and execution.
pub(crate) fn simulate_watchdog(
    simulate: ResMut<SimulateInput>,
    time: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.startup().elapsed();
    if simulate.ticks >= simulate.total && matches!(simulate.clock, ScriptClock::Released { .. }) {
        info!(
            "client: simulation script complete, exiting ticks={} app_s={:.3}",
            simulate.ticks,
            now.as_secs_f64()
        );
        exit.write(AppExit::Success);
    } else if let Some(timeout) = simulate.timeout(now, fixed.timestep()) {
        match timeout {
            ScriptTimeout::Release => error!(
                "client: watchdog timeout — connection, input slot or combat release stalled"
            ),
            ScriptTimeout::Completion => {
                error!("client: watchdog timeout — simulation script stalled after combat release")
            }
        }
        exit.write(AppExit::error());
    }
}

/// Invalid operator input. Absence is separate from malformed input so a typo cannot select a
/// default or silently disable an instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeverError {
    NotUnicode {
        name: String,
    },
    Invalid {
        name: String,
        value: String,
        expected: &'static str,
    },
}

impl LeverError {
    pub(crate) fn invalid(name: &str, value: impl Into<String>, expected: &'static str) -> Self {
        Self::Invalid {
            name: name.into(),
            value: value.into(),
            expected,
        }
    }
}

impl std::fmt::Display for LeverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUnicode { name } => write!(f, "{name}: value must be UTF-8"),
            Self::Invalid {
                name,
                value,
                expected,
            } => write!(f, "{name}={value:?}: expected {expected}"),
        }
    }
}
impl std::error::Error for LeverError {}

/// Startup adapters refuse outside input visibly, with a failing process status, before running.
pub(crate) fn refuse_start<T>(error: impl std::fmt::Display) -> T {
    eprintln!("overmatch: fatal: {error}");
    std::process::exit(2)
}

fn parse_env_text(
    name: &str,
    value: Option<std::ffi::OsString>,
) -> Result<Option<String>, LeverError> {
    value
        .map(|value| {
            value
                .into_string()
                .map_err(|_| LeverError::NotUnicode { name: name.into() })
        })
        .transpose()
}

/// Read text without confusing a non-UTF-8 value with an unset variable.
pub(crate) fn env_text(name: &str) -> Result<Option<String>, LeverError> {
    parse_env_text(name, std::env::var_os(name))
}

fn parse_env_flag(name: &str, value: Option<&str>, default: bool) -> Result<bool, LeverError> {
    let Some(value) = value else {
        return Ok(default);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "false" | "off" | "no" => Ok(false),
        "1" | "true" | "on" | "yes" => Ok(true),
        _ => Err(LeverError::invalid(
            name,
            value,
            "1/true/on/yes or 0/false/off/no/empty",
        )),
    }
}

/// Read a boolean operator lever using an explicit on/off vocabulary.
pub(crate) fn env_flag(name: &str, default: bool) -> Result<bool, LeverError> {
    parse_env_flag(name, env_text(name)?.as_deref(), default)
}

/// Read a path or other text lever; empty, zero, false and off disable it.
pub(crate) fn env_value(name: &str) -> Result<Option<String>, LeverError> {
    Ok(env_text(name)?.filter(|value| {
        let value = value.trim();
        !(value.is_empty()
            || value == "0"
            || value.eq_ignore_ascii_case("false")
            || value.eq_ignore_ascii_case("off"))
    }))
}

fn parse_env_number<T: std::str::FromStr>(
    name: &str,
    value: Option<&str>,
) -> Result<Option<T>, LeverError> {
    value
        .map(|value| {
            value
                .trim()
                .parse()
                .map_err(|_| LeverError::invalid(name, value, std::any::type_name::<T>()))
        })
        .transpose()
}

/// Parse an operator's number. Only an unset variable is absent; malformed or empty is an error.
pub(crate) fn env_parse<T: std::str::FromStr>(name: &str) -> Result<Option<T>, LeverError> {
    parse_env_number(name, env_text(name)?.as_deref())
}

/// Harness-only receive conditioner selected by `SPIKE_JITTER_SEED`.
///
/// Lightyear's stock conditioner (0.30) obtains a fresh thread RNG for every packet and exposes no
/// seeded constructor. This component therefore intercepts raw receive payloads in Lightyear's
/// `ApplyConditioner` set, exactly between UDP buffering and connection/transport consumption.
/// Its SplitMix64 output is keyed only by `(seed, packet_index)`: neither OS randomness nor the
/// delivery clock contributes to the chosen delay. [`Instant`] only anchors and polls that already
/// chosen duration, as every latency conditioner must.
#[derive(Component)]
pub(crate) struct SeededRecvConditioner {
    seed: u64,
    packet_index: u64,
    latency_ms: u64,
    jitter_ms: u64,
    burst: Option<BurstSpec>,
    pending: BTreeMap<Instant, VecDeque<RecvPayload>>,
}

/// A checked periodic receive stall. Only the parser constructs it, so the conditioner holds a
/// positive recurrence and an open interval without revalidating raw operator input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Burst {
    duration: Duration,
    period: Duration,
}

impl Burst {
    pub(crate) fn parse(
        burst_ms: u64,
        period_ticks: u32,
        tick: Duration,
    ) -> Result<Option<Self>, LeverError> {
        if burst_ms == 0 && period_ticks == 0 {
            return Ok(None);
        }
        let malformed = || {
            LeverError::invalid(
                "SPIKE_BURST_MS / SPIKE_BURST_PERIOD_TICKS",
                format!("{burst_ms}/{period_ticks}"),
                "both zero (off), or positive with burst shorter than its period",
            )
        };
        let period = tick.checked_mul(period_ticks).ok_or_else(malformed)?;
        let duration = Duration::from_millis(burst_ms);
        if duration.is_zero() || duration >= period {
            return Err(malformed());
        }
        Ok(Some(Self { duration, period }))
    }
}

/// Periodic receive stall, phase-anchored to a fixed epoch: every window `[k·period, k·period +
/// burst)` after the epoch delivers nothing, and a packet whose delay lands it inside a window is
/// pushed to that window's end — the whole window's packets arrive together, exactly the measured
/// field signature (bursts, then a batch).
struct BurstSpec {
    burst: Duration,
    period: Duration,
    epoch: Instant,
}

impl BurstSpec {
    /// Defer a ready timestamp out of the burst window it landed in, if any.
    fn defer(&self, ready_at: Instant) -> Instant {
        let phase_nanos = ready_at.duration_since(self.epoch).as_nanos() % self.period.as_nanos();
        if phase_nanos < self.burst.as_nanos() {
            ready_at + self.burst - nanos_duration(phase_nanos)
        } else {
            ready_at
        }
    }
}

/// `u128` nanos from a modulo over `Duration::as_nanos` — always narrower than one period.
fn nanos_duration(nanos: u128) -> Duration {
    Duration::from_nanos(u64::try_from(nanos).expect("a burst period is far below u64 nanos"))
}

impl SeededRecvConditioner {
    pub(crate) fn new(seed: u64, latency_ms: u64, jitter_ms: u64) -> Self {
        Self {
            seed,
            packet_index: 0,
            latency_ms,
            jitter_ms,
            burst: None,
            pending: BTreeMap::new(),
        }
    }

    /// Arm the periodic burst, anchored at `epoch` (the conditioner's install instant in
    /// production; explicit here so tests control the phase).
    pub(crate) fn with_burst(mut self, burst: Burst, epoch: Instant) -> Self {
        self.burst = Some(BurstSpec {
            burst: burst.duration,
            period: burst.period,
            epoch,
        });
        self
    }

    /// Match the pinned stock conditioner's actual distribution: an integer offset in
    /// `[-jitter_ms, jitter_ms)` is added to base latency, then a negative result is clamped to
    /// immediate delivery. For ordinary millisecond ranges, modulo bucket counts differ by at most
    /// one output over the full `u64` domain and, unlike `rand::rng`, the result is reproducible.
    fn next_delay(&mut self) -> Duration {
        let delay_ms = seeded_delay_ms(
            self.seed,
            self.packet_index,
            self.latency_ms,
            self.jitter_ms,
        );
        self.packet_index = self.packet_index.wrapping_add(1);
        Duration::from_millis(delay_ms)
    }

    fn condition_packet(&mut self, packet: RecvPayload, received_at: Instant) {
        let mut ready_at = received_at + self.next_delay();
        if let Some(burst) = &self.burst {
            ready_at = burst.defer(ready_at);
        }
        self.pending.entry(ready_at).or_default().push_back(packet);
    }

    fn pop_ready(&mut self, now: Instant) -> Option<RecvPayload> {
        let ready_at = self
            .pending
            .first_key_value()
            .map(|(ready_at, _)| *ready_at)?;
        if ready_at > now {
            return None;
        }

        let (packet, empty) = {
            let packets = self
                .pending
                .get_mut(&ready_at)
                .expect("first pending jitter timestamp must still exist");
            let packet = packets
                .pop_front()
                .expect("a pending jitter timestamp must contain a packet");
            (packet, packets.is_empty())
        };
        if empty {
            self.pending.remove(&ready_at);
        }
        Some(packet)
    }
}

/// Intercept newly received payloads after Lightyear's IO buffer step, then return only payloads
/// whose seeded delay has elapsed. Registered only when seeded jitter is active, so an unset seed
/// does not add a system or replace the stock conditioner.
pub(crate) fn apply_seeded_recv_conditioner(
    mut links: Query<(&mut Link, &mut SeededRecvConditioner)>,
) {
    let now = Instant::now();
    for (mut link, mut conditioner) in &mut links {
        for packet in link.recv.drain() {
            conditioner.condition_packet(packet, now);
        }
        while let Some(packet) = conditioner.pop_ready(now) {
            link.recv.push_raw(packet);
        }
    }
}

fn seeded_delay_ms(seed: u64, packet_index: u64, latency_ms: u64, jitter_ms: u64) -> u64 {
    if jitter_ms == 0 {
        return latency_ms;
    }

    // SplitMix64's fixed odd increment gives every packet index a distinct input across the full
    // u64 cycle. This is a keyed function, not mutable entropy: packet N is determined entirely by
    // `(seed, N)` even if an earlier delivery is reordered by jitter.
    const GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut mixed = seed.wrapping_add(GAMMA.wrapping_mul(packet_index.wrapping_add(1)));
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^= mixed >> 31;

    let span = jitter_ms.saturating_mul(2);
    let bucket = mixed % span;
    let delay_ms = i128::from(latency_ms) + i128::from(bucket) - i128::from(jitter_ms);
    delay_ms.clamp(0, i128::from(u64::MAX)) as u64
}

/// The server's spawn-pose override ([`Script::spawn_pose`]), applied in `spawn_pending_tanks`;
/// `None` without a script or a pose.
pub(crate) fn spawn_pose() -> Option<(Vec3, Quat)> {
    let SpawnPose { pos, rot } = script()?.spawn_pose?;
    info!("server: harness spawn pose pos={pos:?} rot={rot:?}");
    Some((pos, rot))
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use lightyear::core::time::Instant;

    use bevy::math::Vec3;

    use super::{
        Aim, BurstSpec, Drive, Script, ScriptTimeout, SeededRecvConditioner, SimulateInput,
        SpawnPose, WATCHDOG_SLACK, parse_env_flag,
    };
    use crate::command::{AimIntent, TankCommand};

    /// EVERY FIELD OF THE GRAMMAR PARSES TO ITS OWN SLOT, bare or as `Some(…)`, and integers stand
    /// for floats: the runners write scripts by hand and from shell values (`aim: Point(($aim))`),
    /// and the client and server read each slot unchecked.
    #[test]
    fn a_full_script_parses_every_field() {
        let script = Script::parse(
            "(ticks: Some(2048), drive: Fire(primary: true, secondary: true), \
             aim: Point((8, 1.2, -2)), range_m: 12, fire_tick: 320, fire_interval: 128, \
             spawn_pose: (pos: (149.3, 8.75, 293.9), rot: (0, 0, 0, 1)))",
        )
        .expect("a valid script");
        assert_eq!(
            script,
            Script {
                ticks: Some(2048),
                drive: Drive::Fire {
                    primary: true,
                    secondary: true,
                },
                aim: Aim::Point((8.0, 1.2, -2.0)),
                range_m: 12.0,
                fire_tick: 320,
                fire_interval: 128,
                spawn_pose: Some(SpawnPose {
                    pos: Vec3::new(149.3, 8.75, 293.9),
                    rot: bevy::math::Quat::IDENTITY,
                }),
            }
        );
        let sweep = Script::parse("(drive: Idle, aim: Sweep, ticks: 256)").expect("valid");
        assert_eq!(
            (sweep.drive, sweep.aim, sweep.ticks),
            (Drive::Idle, Aim::Sweep, Some(256))
        );
    }

    /// AN EMPTY SCRIPT IS THE DEFAULT SESSION THE RUNNERS LEAN ON: the arc drive, 600 ticks, the
    /// downrange aim at 800 m, one click at tick 300, and no spawn override; the straight course
    /// runs 1280. A changed default silently changes every script that names only what it changes.
    #[test]
    fn an_empty_script_takes_the_documented_defaults() {
        let script = Script::parse("()").expect("an empty script");
        assert_eq!(script, Script::default());
        assert_eq!(script.drive, Drive::Arc);
        assert_eq!(script.aim, Aim::Point((200.0, 0.0, -800.0)));
        assert_eq!(
            (script.range_m, script.fire_tick, script.fire_interval),
            (800.0, 300, 256)
        );
        assert_eq!((script.ticks, script.spawn_pose), (None, None));
        assert_eq!(SimulateInput::new(&script).total, 600);
        let course = Script::parse("(drive: Straight)").expect("a course");
        assert_eq!(SimulateInput::new(&course).total, 1280);
    }

    /// A SCRIPT THAT CANNOT MEAN WHAT IT SAYS FAILS THE BOOT, NAMING ITS FIELD (ADR-0011): a typo,
    /// a fire workload with no trigger or a sweeping aim, a non-finite aim, a range that is not
    /// positive, a degenerate spawn rotation. Each was a silent default or an ignored variable
    /// under the levers the script replaced.
    #[test]
    fn a_script_outside_its_domain_is_refused_naming_the_field() {
        for (text, field) in [
            ("(tick: 5)", "tick"),
            ("(drive: Fire())", "drive"),
            ("(drive: Fire(secondary: true), aim: Sweep)", "aim"),
            ("(aim: Point((1.0, 1e39, 0.0)))", "aim"),
            ("(aim: Point((1.0, 1e30, 0.0)))", "aim"),
            ("(range_m: 0.0)", "range_m"),
            (
                "(spawn_pose: (pos: (0.0, 0.0, 0.0), rot: (0.0, 0.0, 0.0, 0.0)))",
                "spawn_pose",
            ),
            ("(drive: Forward)", "Forward"),
        ] {
            let error = Script::parse(text).expect_err("the script is refused");
            assert!(
                error.to_string().contains(field),
                "{text}: {error:?} names no `{field}`"
            );
        }
    }

    /// Finite scaled rotations describe the same pose at the script edge. The server relies on
    /// receiving a unit quaternion even when squaring the authored magnitude would overflow or
    /// underflow. Fixture magnitudes exercise both numeric extremes.
    #[test]
    fn spawn_rotations_are_normalized_once_without_magnitude_overflow() {
        for magnitude in ["1e30", "1e-30"] {
            let text =
                format!("(spawn_pose: (pos: (0, 0, 0), rot: (0, {magnitude}, 0, {magnitude})))");
            let pose = Script::parse(&text).unwrap().spawn_pose.unwrap();
            assert!(pose.rot.is_normalized());
            assert!((pose.rot * Vec3::NEG_Z).distance(Vec3::NEG_X) < 1e-5);
        }
    }

    /// Measurement scripts rely on combat tick zero reaching the wire: a held input slot must
    /// consume neither the script nor its first click, and neither may an unsynchronized
    /// timeline, whose ticks lightyear never files (a late joiner's first snapshot is already
    /// combat). Fixture ticks are arbitrary.
    #[test]
    fn held_ticks_advance_nothing_and_release_writes_tick_zero() {
        use super::super::battle::{BattlePhase, BattleState, LateJoin};
        use crate::sim_tick::SimTick;
        use bevy::prelude::*;
        use lightyear::prelude::LocalTimelineSync;
        use lightyear::prelude::input::native::{ActionState, InputMarker};

        let mut app = App::new();
        app.insert_resource(SimulateInput::new(
            &Script::parse("(drive: Fire(primary: true), fire_tick: 0, ticks: 2)").unwrap(),
        ))
        .insert_resource(Time::<Real>::default())
        .insert_resource(Time::<Fixed>::from_hz(64.0))
        .insert_resource(lightyear::prelude::LocalTimeline::default())
        .init_resource::<LocalTimelineSync>()
        .add_systems(FixedPreUpdate, super::buffer_input);
        let slot = app
            .world_mut()
            .spawn((
                ActionState::<TankCommand>::default(),
                InputMarker::<TankCommand>::default(),
            ))
            .id();
        app.world_mut().run_schedule(FixedPreUpdate);
        assert_eq!(app.world().resource::<SimulateInput>().ticks, 0);
        let battle = app.world_mut().spawn(BattleState::default()).id();
        for _ in 0..3 {
            app.world_mut().run_schedule(FixedPreUpdate);
            assert_eq!(app.world().resource::<SimulateInput>().ticks, 0);
            assert!(
                !app.world()
                    .get::<ActionState<TankCommand>>(slot)
                    .unwrap()
                    .0
                    .fire_primary
            );
        }
        app.world_mut()
            .get_mut::<BattleState>(battle)
            .unwrap()
            .phase = BattlePhase::Combat {
            ends_at: SimTick::new(100),
            late_join: LateJoin::Closed,
        };
        app.world_mut().run_schedule(FixedPreUpdate);
        assert_eq!(
            app.world().resource::<SimulateInput>().ticks,
            0,
            "combat on an unsynchronized timeline files nothing"
        );
        app.world_mut()
            .resource_mut::<LocalTimelineSync>()
            .set_synced(true);
        let ready =
            std::env::temp_dir().join(format!("overmatch-script-ready-{}", std::process::id()));
        assert!(!ready.exists(), "fixture start file must be absent");
        app.world_mut().resource_mut::<SimulateInput>().start =
            crate::cost::WorkloadStart::WaitingForFile(ready.clone());
        app.world_mut().run_schedule(FixedPreUpdate);
        assert_eq!(
            app.world().resource::<SimulateInput>().ticks,
            0,
            "the tool barrier must preserve tick zero after combat and sync are ready"
        );
        std::fs::write(&ready, "").unwrap();
        app.world_mut().run_schedule(FixedPreUpdate);
        std::fs::remove_file(&ready).unwrap();
        assert_eq!(app.world().resource::<SimulateInput>().ticks, 1);
        assert!(
            app.world()
                .get::<ActionState<TankCommand>>(slot)
                .unwrap()
                .0
                .fire_primary
        );
        app.world_mut().run_schedule(FixedPreUpdate);
        assert_eq!(app.world().resource::<SimulateInput>().ticks, 2);
        assert!(
            !app.world()
                .get::<ActionState<TankCommand>>(slot)
                .unwrap()
                .0
                .fire_primary
        );
    }

    /// Frame runners must open on the frame rows' clock even when window initialization delays
    /// the first update. Fixture startup and release instants model that excluded interval.
    #[test]
    fn release_stamp_uses_the_frame_clock_after_a_delayed_first_update() {
        use bevy::prelude::{Real, Time};
        let start = std::time::Instant::now();
        let mut time = Time::<Real>::new(start);
        time.update_with_instant(start + Duration::from_secs(8));
        time.update_with_instant(start + Duration::from_secs(68));
        let stamp = super::release_stamp(3900, Duration::from_secs(68), &time);
        assert!(
            stamp.contains("app_s=68.000"),
            "the watchdog retains wall time: {stamp}"
        );
        assert!(
            stamp.ends_with("frame_s=60.000"),
            "the frame window excludes startup: {stamp}"
        );
    }

    /// Runners get a finite launch bound even after the slot arrives, and a separate execution
    /// bound proportional to script length. Repeated slot sightings cannot renew either bound.
    #[test]
    fn watchdog_bounds_launch_and_execution_from_release() {
        let tick = Duration::from_secs_f64(1.0 / 64.0); // Fixture timestep.
        let mut sim = SimulateInput::new(&Script::parse("(ticks: 6400)").unwrap());
        assert_eq!(
            sim.timeout(WATCHDOG_SLACK + tick, tick),
            Some(ScriptTimeout::Release)
        );
        sim.hold_slot(Duration::from_secs(1));
        sim.hold_slot(Duration::from_secs(19));
        assert_eq!(
            sim.timeout(WATCHDOG_SLACK + tick, tick),
            Some(ScriptTimeout::Release)
        );
        let released = Duration::from_secs(2); // Fixture wall time.
        assert!(sim.release(released));
        assert!(!sim.release(released + tick));
        let deadline = released + tick * sim.total + WATCHDOG_SLACK;
        assert_eq!(sim.timeout(deadline, tick), None);
        assert_eq!(
            sim.timeout(deadline + tick, tick),
            Some(ScriptTimeout::Completion)
        );
    }

    /// EACH DRIVE PLAYS THE WORKLOAD ITS RUNNERS MEASURE: the arc drives and steers inside its
    /// window and clicks once at `fire_tick`; idle never moves or fires; a fire workload stays put,
    /// holds the secondary only past its warmup and clicks the primary every interval from
    /// `fire_tick`; the straight course never steers. The cost and pen cam runners
    /// read their deltas against these exact windows.
    #[test]
    fn each_drive_plays_its_workload() {
        let at = |text: &str, t: u32| {
            let sim = SimulateInput::new(&Script::parse(text).expect("valid"));
            let mut command = TankCommand::default();
            sim.write(t, &mut command);
            command
        };
        let arc = at("()", 200);
        assert_eq!(
            (arc.throttle, arc.steer, arc.fire_primary),
            (1.0, 0.3, false)
        );
        assert!(at("()", 300).fire_primary && !at("()", 301).fire_primary);
        assert_eq!(at("()", 400).throttle, 0.0);
        let idle = at("(drive: Idle)", 300);
        assert_eq!((idle.throttle, idle.fire_primary), (0.0, false));
        let fire = "(drive: Fire(primary: true, secondary: true), fire_tick: 320, \
                    fire_interval: 256, aim: Point((8.0, 1.2, 2.2)))";
        assert!(!at(fire, 192).fire_secondary && at(fire, 193).fire_secondary);
        assert!(at(fire, 320).fire_primary && at(fire, 576).fire_primary);
        assert!(!at(fire, 448).fire_primary);
        assert_eq!(at(fire, 200).throttle, 0.0);
        assert_eq!(
            at(fire, 200).aim,
            Some(AimIntent::HullLocal(Vec3::new(8.0, 1.2, 2.2)))
        );
        assert_eq!(at("(drive: Straight)", 1000).steer, 0.0);
    }

    /// THE WORKED SCRIPT THE MODULE DOC NAMES IS A VALID SCRIPT: it is the grammar's one example
    /// file, the one copied to write the next.
    #[test]
    fn the_example_script_parses() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/harness/idle.ron");
        let script = Script::load(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(script.drive, Drive::Idle);
    }

    /// Operator typos must fail instead of changing the workload. The pure parsers exercise the
    /// production edge without mutating this test process's environment.
    #[test]
    fn env_flag_value_parsing_truth_table() {
        for off in ["0", "false", "FALSE", "off", "no", "", "  false  ", "   "] {
            assert!(!parse_env_flag("FLAG", Some(off), true).unwrap(), "{off:?}");
        }
        for on in ["1", "true", "TRUE", "on", "yes"] {
            assert!(parse_env_flag("FLAG", Some(on), false).unwrap(), "{on:?}");
        }
        for bad in ["00", "anything", "treu"] {
            assert!(
                parse_env_flag("FLAG", Some(bad), false)
                    .unwrap_err()
                    .to_string()
                    .contains("FLAG")
            );
        }
        assert!(parse_env_flag("FLAG", None, true).unwrap());
        assert!(!parse_env_flag("FLAG", None, false).unwrap());
        assert_eq!(super::parse_env_number::<u64>("COUNT", None), Ok(None));
        assert_eq!(
            super::parse_env_number::<u64>("COUNT", Some("0")),
            Ok(Some(0))
        );
        for bad in ["", "8O", "-1", "18446744073709551616"] {
            assert!(
                super::parse_env_number::<u64>("COUNT", Some(bad))
                    .unwrap_err()
                    .to_string()
                    .contains("COUNT")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_operator_input_is_an_error_not_absence() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(
            super::parse_env_text("ADDRESS", Some(std::ffi::OsString::from_vec(vec![0xff]))),
            Err(super::LeverError::NotUnicode {
                name: "ADDRESS".into()
            })
        );
    }

    /// Partial or impossible stalls must fail at startup instead of disabling the conditioner or
    /// dividing by a zero recurrence. Fixture tick and periods are deliberately small.
    #[test]
    fn a_burst_requires_a_positive_open_interval() {
        let tick = Duration::from_millis(10);
        assert_eq!(super::Burst::parse(0, 0, tick).unwrap(), None);
        assert_eq!(
            super::Burst::parse(60, 33, tick).unwrap(),
            Some(super::Burst {
                duration: Duration::from_millis(60),
                period: Duration::from_millis(330),
            })
        );
        for (duration, period) in [(1, 0), (0, 1), (10, 1), (11, 1), (u64::MAX, 1)] {
            assert!(
                super::Burst::parse(duration, period, tick).is_err(),
                "{duration}/{period}"
            );
        }
    }

    #[test]
    fn seeded_jitter_delay_sequence_is_reproducible() {
        let mut first = SeededRecvConditioner::new(0x5eed_cafe_f00d_beef, 80, 10);
        let mut second = SeededRecvConditioner::new(0x5eed_cafe_f00d_beef, 80, 10);
        let mut different = SeededRecvConditioner::new(0x5eed_cafe_f00d_bef0, 80, 10);

        let first: Vec<_> = (0..64).map(|_| first.next_delay()).collect();
        let second: Vec<_> = (0..64).map(|_| second.next_delay()).collect();
        let different: Vec<_> = (0..64).map(|_| different.next_delay()).collect();

        assert_eq!(first, second, "the same seed must reproduce every delay");
        assert_ne!(first, different, "different seeds must change the pattern");
        assert!(
            first.iter().all(
                |delay| (Duration::from_millis(70)..Duration::from_millis(90)).contains(delay)
            ),
            "80 ms base plus stock-shaped 10 ms jitter must stay in [70, 90) ms",
        );
    }

    /// THE BURST LAW: a ready time inside `[k·period, k·period + burst)` past the epoch defers to
    /// that window's end; outside passes untouched. The second-window case reds a dropped modulo,
    /// the in-window case reds an inverted comparison, and the window-end equality reds a defer by
    /// the full burst instead of the phase remainder.
    #[test]
    fn a_burst_window_delivery_defers_to_the_window_end() {
        let epoch = Instant::now();
        let spec = BurstSpec {
            burst: Duration::from_millis(60),
            period: Duration::from_millis(500),
            epoch,
        };
        let at = |ms: u64| epoch + Duration::from_millis(ms);
        assert_eq!(spec.defer(at(10)), at(60), "first window: the window end");
        assert_eq!(spec.defer(at(60)), at(60), "the window end itself is clear");
        assert_eq!(spec.defer(at(100)), at(100), "between windows: untouched");
        assert_eq!(
            spec.defer(at(510)),
            at(560),
            "the second window defers by epoch phase, not absolute time",
        );
    }
}
