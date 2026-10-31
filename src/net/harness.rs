//! The scripted session and the shared `SPIKE_*` environment parser.
//!
//! A scripted session has one input: the RON file `OVERMATCH_HARNESS=<script>.ron` names, one
//! [`Script`], whose struct docs are the grammar's one home. On a client it selects the scripted
//! client (`net::client::run`: GPU-less unless `OVERMATCH_WINDOW` names a window,
//! `crate::run_window`), which plays the script against the server and exits once it has; the
//! dedicated server reads the same variable for [`Script::spawn_pose`]. `scripts/harness/idle.ron`
//! is a worked script, checked by a test.
//!
//! The other levers stay variables. Boolean/value switches treat `0`, `false` (case-insensitive),
//! and the empty string as off; every other non-numeric value is on. Numeric values retain ordinary
//! parsing (including a meaningful zero) and ignore an empty string.
//!
//! | Variable / flag | Kind and default | Effect |
//! |---|---|---|
//! | `OVERMATCH_BOT` | count; off, bare = `1` | Server: that many ownerless circling, firing bot tanks. |
//! | `OVERMATCH_HARNESS` | path; off | The scripted session ([`Script`]). |
//! | `OVERMATCH_SERVER_BIND` | IPv4; every interface | Server: bind the UDP socket to this address only. |
//! | `OVERMATCH_SERVER_STATS_S` | seconds; off, bare = `10` | Server: one `server_stats {json}` line on stdout per period (`net::server_stats`). |
//! | `SPIKE_BURST_MS` | milliseconds; `0` | Periodic receive stall: every burst-window packet delivers at the window's end. |
//! | `SPIKE_BURST_PERIOD_TICKS` | ticks; `0` | Burst recurrence period; both burst values set activates the seeded conditioner. |
//! | `SPIKE_COST_TRACE` | path; off | Role-qualified fixed-tick cost JSONL. |
//! | `SPIKE_COST_WARMUP` | ticks; `384` | Cost rows skipped before recording. |
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
use lightyear::prelude::Link;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use serde::Deserialize;

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
    /// Run length in fixed ticks from the input slot; the client exits once it has played them.
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

/// The server's spawned tank pose: translation `pos` in world metres and the xyzw quaternion
/// `rot`, normalized on read so a hand-entered pose lands as a valid `Rotation`. Places the tank
/// onto a known resting contact so a beached-rest repro is deterministic.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SpawnPose {
    pub(crate) pos: (f32, f32, f32),
    pub(crate) rot: (f32, f32, f32, f32),
}

impl Script {
    /// The script `text` holds, validated; the error names the field.
    fn parse(text: &str) -> Result<Self, String> {
        let script: Self = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(|error| error.to_string())?;
        script.validate()?;
        Ok(script)
    }

    fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(&text)
    }

    fn validate(&self) -> Result<(), String> {
        if let Drive::Fire {
            primary: false,
            secondary: false,
        } = self.drive
        {
            return Err("drive: a Fire workload fires the primary, the secondary or both".into());
        }
        match self.aim {
            Aim::Point(point) if !Vec3::from(point).is_finite() => {
                return Err(format!("aim: {point:?} is not a finite point"));
            }
            Aim::Sweep if matches!(self.drive, Drive::Fire { .. }) => {
                return Err("aim: a Fire workload aims at a point, not a sweep".into());
            }
            _ => {}
        }
        if !(self.range_m.is_finite() && self.range_m > 0.0) {
            return Err(format!("range_m: {} is not a positive range", self.range_m));
        }
        if let Some(SpawnPose { pos, rot }) = self.spawn_pose {
            let rot = Vec4::from(rot);
            if !(Vec3::from(pos).is_finite() && rot.is_finite() && rot.length() > 0.0) {
                return Err(format!(
                    "spawn_pose: {pos:?} {rot:?} is not a finite position and rotation"
                ));
            }
        }
        Ok(())
    }
}

/// This process's script, read once from the file [`VARIABLE`] names; `None` when it is unset.
///
/// # Panics
///
/// When the variable names a file that is missing or holds no valid script ([`Script`]).
pub(crate) fn script() -> Option<&'static Script> {
    static SCRIPT: OnceLock<Option<Script>> = OnceLock::new();
    SCRIPT
        .get_or_init(|| {
            let path = PathBuf::from(std::env::var_os(VARIABLE)?);
            let script = Script::load(&path)
                .unwrap_or_else(|error| panic!("{VARIABLE}={}: {error}", path.display()));
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
}

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
        }
    }

    /// The scripted secondary trigger's held window: from the warmup boundary to the end of the
    /// run.
    fn holding_secondary(t: u32) -> bool {
        // ~3 s: let the connect handshake, tank spawn, and asset load settle before the burst
        // starts, and comfortably inside the cost recorder's own 6 s warmup.
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

/// Headless simulate mode: write the scripted `TankCommand` into the lightyear `ActionState` slot
/// each tick. Whole-state snapshot per tick, no devices.
pub(crate) fn buffer_input(
    mut sim: ResMut<SimulateInput>,
    mut slots: Query<&mut ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    let Ok(mut state) = slots.single_mut() else {
        return;
    };
    sim.ticks += 1;
    sim.write(sim.ticks, &mut state.0);
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

/// Simulate mode: exit cleanly once the script has played out, or bail on a wall-clock timeout if
/// the connection never came up.
pub(crate) fn simulate_watchdog(
    simulate: Res<SimulateInput>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    if simulate.ticks >= simulate.total {
        info!("client: simulation script complete, exiting");
        exit.write(AppExit::Success);
    } else if time.elapsed_secs() > simulate.total as f32 / 64.0 + 20.0 {
        // The script advances at the 64 Hz tick rate, so a run that never reaches `total` in its own
        // wall-clock budget + 20 s slack is wedged (never got an input slot / stalled). Scaling the
        // bound with `total` lets the long MG-cost workloads (~40 s) finish while still fast-failing a
        // never-connected short run.
        error!("client: watchdog timeout — never got an input slot");
        exit.write(AppExit::error());
    }
}

fn value_is_enabled(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
}

fn parse_env_flag(value: Option<&str>, default: bool) -> bool {
    value.map(value_is_enabled).unwrap_or(default)
}

/// Read a `SPIKE_*` boolean using the shared off vocabulary.
pub(crate) fn env_flag(name: &str, default: bool) -> bool {
    parse_env_flag(std::env::var(name).ok().as_deref(), default)
}

/// Read a non-numeric `SPIKE_*` value, treating the shared off vocabulary as absent.
pub(crate) fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| value_is_enabled(value))
}

/// Parse a numeric `SPIKE_*` value. Numeric zero remains meaningful; empty values are ignored.
pub(crate) fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            value.parse().ok()
        }
    })
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
        let phase_nanos =
            ready_at.duration_since(self.epoch).as_nanos() % self.period.as_nanos().max(1);
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
    pub(crate) fn with_burst(mut self, burst: Duration, period: Duration, epoch: Instant) -> Self {
        self.burst = Some(BurstSpec {
            burst,
            period,
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
    let pos = Vec3::from(pos);
    let rot = Quat::from_xyzw(rot.0, rot.1, rot.2, rot.3).normalize();
    info!("server: harness spawn pose pos={pos:?} rot={rot:?}");
    Some((pos, rot))
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use lightyear::core::time::Instant;

    use bevy::math::Vec3;

    use super::{
        Aim, BurstSpec, Drive, Script, SeededRecvConditioner, SimulateInput, SpawnPose,
        parse_env_flag,
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
                    pos: (149.3, 8.75, 293.9),
                    rot: (0.0, 0.0, 0.0, 1.0),
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
            ("(range_m: 0.0)", "range_m"),
            (
                "(spawn_pose: (pos: (0.0, 0.0, 0.0), rot: (0.0, 0.0, 0.0, 0.0)))",
                "spawn_pose",
            ),
            ("(drive: Forward)", "Forward"),
        ] {
            let error = Script::parse(text).expect_err("the script is refused");
            assert!(
                error.contains(field),
                "{text}: {error:?} names no `{field}`"
            );
        }
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

    #[test]
    fn env_flag_value_parsing_truth_table() {
        for off in ["0", "false", "FALSE", "FaLsE", "", "  false  ", "   "] {
            assert!(!parse_env_flag(Some(off), true), "{off:?} must be off");
        }
        for on in ["1", "true", "yes", "no", "00", "anything"] {
            assert!(parse_env_flag(Some(on), false), "{on:?} must be on");
        }
        assert!(parse_env_flag(None, true), "missing uses the true default");
        assert!(
            !parse_env_flag(None, false),
            "missing uses the false default"
        );
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
