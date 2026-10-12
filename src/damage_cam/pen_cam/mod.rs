//! The pen cam demo: the in-battle flow of a hit, end to end, played over the recorded scenario
//! records in `assets/dev/pen_cam/`. The shooter's sight shows the struck tank, the round flies,
//! the world shows the impact, and 0.2 s later the corner opens top-right with one frame per
//! verdict — the see-through hull for a penetration, the plate slice for a non-pen, the outside
//! view for a ricochet or for damage — and closes on its own. Specification:
//! `.agents/scratch/damage-cam-2026-10-08/demo/ux-spec.md` (§3–§5) under the owner record
//! `…/product/in-battle/owner-direction-2026-10-10.md` (four verdicts).
//!
//! Run it: `cargo run --bin pen_cam_demo` (optionally a scenario label, `pen_cam_demo s4`, and
//! `--once` to stop after it). Keys: `X` dismiss, hold `C` to hold, hold `V` for the own-tank
//! glass, `P` cycles the corner setting (all / through only / off); demo-only `1`–`9`, `0`, `-`
//! pick a scenario, `A` toggles auto-advance, `R` replays, `Space` pauses, `←`/`→` scrub, `[`/`]`
//! change speed, mouse drag orbits the corner while paused, `F` expands the corner, `O` switches
//! the slice between orthographic and perspective, `G` cycles glass variants, `D` toggles the dose
//! scale, `L` toggles each spall cone's axis line, `M` toggles reduce motion.
//!
//! A view over the record: nothing here marches a shell or reads the sim. Every position comes
//! from the record (hull-local, the stage's frame), every timing from [`timeline`], and the
//! record's few derived quantities are named in [`plan`].
//!
//! Capture: with `OVERMATCH_CAPTURE` set the clock steps a fixed 1/30 s per frame, so
//! `dev_capture`'s frame times are presentation times; virtual time stays paused until both tank
//! views have spawned, so a frame time never lands on a loading screen.

mod field;
mod hud;
mod killcam;
mod material;
mod plan;
mod stage;
mod timeline;
mod view;
mod world;

use std::time::Duration;

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use self::plan::Plan;
use self::timeline::{CornerKind, Turn};
use super::shapes::TankAsset;
use super::{PenetrationReport, ShotVerdict};

/// The records, relative to the asset root.
const RECORD_DIR: &str = "dev/pen_cam";

/// One scenario of the demo: the records it fires, each at its offset (s) from the first.
pub struct Entry {
    pub label: &'static str,
    pub shots: &'static [(&'static str, f32)],
    /// The struck player's side: the own-tank glass, not the shooter's corner.
    pub victim: bool,
}

/// The playlist, in key order (`1`–`9`, `0`, `-`). Fixture data: which recorded shots the demo
/// plays and how they are paired.
pub const PLAYLIST: &[Entry] = &[
    Entry {
        label: "S1 through, spall cone into the crew",
        shots: &[("s1_through_crew", 0.0)],
        victim: false,
    },
    Entry {
        label: "S2 stopped in the gun mask",
        shots: &[("s2_stopped_mantlet", 0.0)],
        victim: false,
    },
    Entry {
        label: "S3 ricochet off the hull side",
        shots: &[("s3_ricochet", 0.0)],
        victim: false,
    },
    Entry {
        label: "S4 through a road wheel, stopped in the hull",
        shots: &[("s4_spaced_then_stopped", 0.0)],
        victim: false,
    },
    Entry {
        label: "S5 oblique through, the cone axis leans",
        shots: &[("s5_oblique_through", 0.0)],
        victim: false,
    },
    Entry {
        label: "S6 overpenetration through empty space",
        shots: &[("s6_overpen", 0.0)],
        victim: false,
    },
    Entry {
        label: "S7 into the engine, the engine shadows",
        shots: &[("s7_engine_shadow", 0.0)],
        victim: false,
    },
    Entry {
        label: "S8 two records 0.8 s apart",
        shots: &[("s8a_mg_ricochet", 0.0), ("s8b_mg_stopped", 0.8)],
        victim: false,
    },
    Entry {
        label: "S9 own tank hit, one box lost (victim glass)",
        shots: &[("s9_one_box_lost", 0.0)],
        victim: true,
    },
    Entry {
        label: "S10 main gun barrel struck",
        shots: &[("s10_gun_barrel_struck", 0.0)],
        victim: false,
    },
    Entry {
        label: "S11 bow MG barrel struck",
        shots: &[("s11_mg_barrel_struck", 0.0)],
        victim: false,
    },
    Entry {
        label: "K1 own tank knocked out, ammo cooks off (kill cam)",
        shots: &[("s5_oblique_through", 0.0)],
        victim: true,
    },
];

const ENTRY_KEYS: [KeyCode; 12] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
    KeyCode::Digit0,
    KeyCode::Minus,
    KeyCode::Equal,
];

/// Seconds of quiet sight picture before the first shot.
const LEAD_IN_S: f32 = 1.2;
/// Seconds after the last corner closes before the run ends.
const TAIL_S: f32 = 0.6;
/// Auto-advance waits this long between runs.
const GAP_S: f32 = 2.0;
/// Scrub step (s) and the speed ladder.
const SCRUB_S: f32 = 0.1;
const SPEEDS: [f32; 5] = [0.1, 0.25, 0.5, 1.0, 2.0];
/// The capture clock's fixed step.
const CAPTURE_STEP: Duration = Duration::from_nanos(33_333_333);

/// Every record the playlist names, loaded once. A malformed record is a broken fixture.
#[derive(Resource)]
pub struct Library {
    pub records: Vec<Record>,
    /// The struck vehicle's tank file, shared by every record.
    pub vehicle: String,
}

pub struct Record {
    pub stem: &'static str,
    pub report: PenetrationReport,
    pub plan: Plan,
    /// Whether the record knocks the struck tank out, by the game's rule ([`killcam::knockout`]).
    pub knockout: Option<crate::damage::KnockoutReason>,
    /// Who fired it, as the victim's copy names them.
    pub killer: Option<killcam::Killer>,
}

impl Library {
    fn load() -> Self {
        let dir = crate::assets::asset_root().join(RECORD_DIR);
        let mut reports: Vec<(&'static str, PenetrationReport)> = Vec::new();
        for entry in PLAYLIST {
            for (stem, _) in entry.shots {
                if reports.iter().any(|(known, _)| known == stem) {
                    continue;
                }
                let report = PenetrationReport::load(&dir.join(format!("{stem}.ron")))
                    .unwrap_or_else(|err| panic!("pen cam record: {err}"));
                reports.push((stem, report));
            }
        }
        let vehicle = reports
            .iter()
            .find_map(|(_, report)| report.vehicle.clone())
            .expect("a recorded scenario names its vehicle");
        let asset = TankAsset::load(&vehicle).unwrap_or_else(|err| panic!("pen cam tank: {err}"));
        let records = reports
            .into_iter()
            .map(|(stem, report)| {
                assert!(
                    report.vehicle.as_deref() == Some(vehicle.as_str()),
                    "{stem}: the demo stages one vehicle, {vehicle}"
                );
                let plan = Plan::new(&report, &asset.shapes(&report.servo_deg));
                let knockout = killcam::knockout(&report);
                let killer = report.shooter.as_ref().map(|shooter| {
                    killcam::Killer::of(shooter, &report.round)
                        .unwrap_or_else(|err| panic!("{stem}: the shooter: {err}"))
                });
                Record {
                    stem,
                    report,
                    plan,
                    knockout,
                    killer,
                }
            })
            .collect();
        Self { records, vehicle }
    }

    pub fn index(&self, stem: &str) -> usize {
        self.records
            .iter()
            .position(|r| r.stem == stem)
            .expect("the playlist's records are loaded")
    }
}

/// The corner setting a player picks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CornerSetting {
    All,
    ThroughOnly,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DoseScale {
    /// (a): absolute, log-mapped reference millimetres, one scale for every box.
    Absolute,
    /// (b): the dose over the box's count.
    Relative,
}

/// The demo's switches.
#[derive(Resource, Clone, PartialEq)]
pub struct Settings {
    pub corner: CornerSetting,
    pub reduce_motion: bool,
    pub dose: DoseScale,
    /// 0: the specified rim glass, 1: a stronger rim, 2: flat alpha, for the look test.
    pub glass: u8,
    pub slice_ortho: bool,
    pub expanded: bool,
    pub axis_line: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            corner: CornerSetting::All,
            reduce_motion: false,
            dose: DoseScale::Absolute,
            glass: 0,
            slice_ortho: true,
            expanded: false,
            axis_line: false,
        }
    }
}

/// One shot of a run, in run seconds and world positions.
#[derive(Clone, Debug)]
pub struct ShotRun {
    /// Index into [`Library::records`].
    pub record: usize,
    pub fire: f32,
    pub impact: f32,
    pub arrival: f32,
    /// Where the round starts its flight toward the impact (stage frame).
    pub muzzle: Vec3,
}

/// The run of one playlist entry: its shots and the corner's turns.
#[derive(Resource)]
pub struct Run {
    pub entry: usize,
    pub t: f32,
    pub speed: usize,
    pub paused: bool,
    pub auto: bool,
    pub once: bool,
    pub shots: Vec<ShotRun>,
    /// Turns index [`Self::shots`].
    pub turns: Vec<Turn>,
    pub end: f32,
    /// Seconds the hold key has frozen the current turn, and the turn it froze.
    pub held: f32,
    pub held_turn: Option<usize>,
    /// The corner time at which the player dismissed (or released a hold on) a turn.
    pub dismissed: Option<(usize, f32)>,
    /// Where the auto-advance gap started.
    pub finished_at: Option<f32>,
    /// While `V` is held: run seconds it went down.
    pub own_glass_since: Option<f32>,
    /// Mouse orbit applied to the corner camera while paused (yaw, pitch; rad).
    pub orbit: Vec2,
    /// The shot that knocks the player's own tank out, and the kill cam's beats; the corner stays
    /// shut on such a run.
    pub kill: Option<(usize, killcam::Beats)>,
}

impl Run {
    fn new(entry: usize, library: &Library, settings: &Settings, once: bool) -> Self {
        let mut run = Self {
            entry,
            t: 0.0,
            speed: 3,
            paused: false,
            auto: !once,
            once,
            shots: Vec::new(),
            turns: Vec::new(),
            end: 0.0,
            held: 0.0,
            held_turn: None,
            dismissed: None,
            finished_at: None,
            own_glass_since: None,
            orbit: Vec2::ZERO,
            kill: None,
        };
        run.schedule(library, settings);
        run
    }

    /// Lay out the entry's shots and the corner's turns under `settings`.
    fn schedule(&mut self, library: &Library, settings: &Settings) {
        let entry = &PLAYLIST[self.entry];
        self.shots = entry
            .shots
            .iter()
            .map(|(stem, offset)| {
                let record = library.index(stem);
                let report = &library.records[record].report;
                let fire = LEAD_IN_S + offset;
                let speed = report.projectile.speed_mps.max(1.0);
                let impact = fire + world::SHOOTER_DISTANCE_M / speed;
                // The approach flies the record's own first leg, origin to impact, extended back.
                let incoming = (Vec3::from(report.impact.point) - Vec3::from(report.origin))
                    .normalize_or(Vec3::NEG_Z);
                ShotRun {
                    record,
                    fire,
                    impact,
                    arrival: impact + timeline::OPEN_LATENCY_S,
                    muzzle: Vec3::from(report.impact.point) - incoming * world::SHOOTER_DISTANCE_M,
                }
            })
            .collect();
        self.kill = entry
            .victim
            .then(|| {
                self.shots.iter().enumerate().find_map(|(index, shot)| {
                    library.records[shot.record].knockout.map(|_| {
                        (
                            index,
                            killcam::Beats {
                                impact: shot.impact,
                            },
                        )
                    })
                })
            })
            .flatten();
        let shown: Vec<usize> = (0..self.shots.len())
            .filter(|_| self.kill.is_none())
            .filter(|&i| {
                let verdict = library.records[self.shots[i].record].report.impact.verdict;
                match settings.corner {
                    CornerSetting::All => true,
                    CornerSetting::ThroughOnly => verdict == ShotVerdict::Penetration,
                    CornerSetting::Off => false,
                }
            })
            .collect();
        let arrivals: Vec<f32> = shown.iter().map(|&i| self.shots[i].arrival).collect();
        self.turns = timeline::turns(&arrivals)
            .into_iter()
            .map(|turn| Turn {
                record: shown[turn.record],
                start: turn.start,
            })
            .collect();
        let last_impact = self.shots.iter().map(|s| s.impact).fold(0.0, f32::max);
        let corner_end = self
            .turns
            .last()
            .map(|turn| turn.start + self.kind_of(turn.record, library).duration())
            .unwrap_or(0.0);
        self.end = (last_impact + 1.0).max(corner_end) + TAIL_S;
        if let Some((_, beats)) = self.kill {
            self.end = beats.end();
        }
    }

    pub fn kind_of(&self, shot: usize, library: &Library) -> CornerKind {
        if PLAYLIST[self.entry].victim {
            CornerKind::Victim
        } else {
            library.records[self.shots[shot].record].plan.kind
        }
    }

    fn reset_interaction(&mut self) {
        self.held = 0.0;
        self.held_turn = None;
        self.dismissed = None;
        self.finished_at = None;
    }
}

/// The corner as it stands this frame.
#[derive(Resource, Default, Clone, PartialEq)]
pub struct Corner {
    /// The record on show, if the corner is open.
    pub showing: Option<Showing>,
    /// The turn before it while the crossfade runs: its opacity.
    pub fading: Option<f32>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Showing {
    /// Index into [`Library::records`].
    pub record: usize,
    /// Which turn of the run: a new turn rebuilds the stage even for the same record.
    pub turn: usize,
    pub kind: CornerKind,
    /// Corner seconds since the turn opened, holds excluded.
    pub c: f32,
    pub opacity: f32,
    pub scale: f32,
    /// Fraction of the run still to play, for the progress hairline.
    pub remaining: f32,
    /// Where the record is drawn.
    pub surface: Surface,
}

/// Where a [`Showing`] is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surface {
    /// The corner inset.
    Corner,
    /// On the battlefield, over the struck tank itself: the kill cam's lesson.
    World,
}

/// Set once both tank views have spawned and been dressed.
#[derive(Resource, Default)]
pub struct Ready(pub bool);

pub fn demo_plugin(app: &mut App) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let once = args.iter().any(|a| a == "--once");
    let start = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .find_map(|a| {
            PLAYLIST.iter().position(|entry| {
                entry
                    .label
                    .split_whitespace()
                    .next()
                    .is_some_and(|tag| tag.eq_ignore_ascii_case(a))
            })
        })
        .unwrap_or(0);
    let library = Library::load();
    let settings = Settings::default();
    let run = Run::new(start, &library, &settings, once);
    if std::env::var_os("OVERMATCH_CAPTURE").is_some() {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(CAPTURE_STEP));
    }
    app.add_plugins((
        crate::quit::plugin,
        crate::spec::plugin,
        crate::render_policy::plugin,
        crate::ui_font::plugin,
        MaterialPlugin::<material::GlowMaterial>::default(),
        MaterialPlugin::<material::SliceMaterial>::default(),
    ))
    .insert_resource(ClearColor(world::SKY))
    .insert_resource(library)
    .insert_resource(settings)
    .insert_resource(run)
    .init_resource::<Corner>()
    .init_resource::<Ready>()
    .add_systems(Startup, (hold_clock, view::spawn, world::spawn))
    .add_systems(
        Startup,
        (stage::spawn, hud::spawn, killcam::spawn).after(view::spawn),
    )
    .add_systems(
        Update,
        (
            view::prepare,
            release_clock,
            controls,
            killcam::input,
            advance,
            corner_now,
            view::pose,
            stage::rebuild,
            stage::animate,
            world::animate,
            killcam::camera,
            hud::layout,
            hud::animate,
            killcam::hud,
        )
            .chain(),
    );
    stage::plugin(app);
}

/// Virtual time waits for the views; `release_clock` starts it.
fn hold_clock(mut time: ResMut<Time<Virtual>>) {
    time.pause();
}

fn release_clock(ready: Res<Ready>, mut time: ResMut<Time<Virtual>>, mut released: Local<bool>) {
    if ready.0 && !*released {
        time.unpause();
        *released = true;
    }
}

fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    library: Res<Library>,
    mut settings: ResMut<Settings>,
    mut run: ResMut<Run>,
    corner: Res<Corner>,
) {
    for (index, key) in ENTRY_KEYS.iter().enumerate() {
        if keys.just_pressed(*key) && index < PLAYLIST.len() {
            let once = run.once;
            *run = Run::new(index, &library, &settings, once);
            run.auto = false;
        }
    }
    if let Some((_, beats)) = run.kill
        && keys.just_pressed(KeyCode::KeyR)
    {
        // Respawn, the game's binding: the demo's stand-in ends the run.
        if beats.respawn_live(run.t) {
            run.t = run.end;
        }
    } else if keys.just_pressed(KeyCode::KeyR) {
        run.t = 0.0;
        run.reset_interaction();
    }
    if keys.just_pressed(KeyCode::KeyA) {
        run.auto = !run.auto;
    }
    if keys.just_pressed(KeyCode::Space) {
        run.paused = !run.paused;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        run.t = (run.t - SCRUB_S).max(0.0);
        run.reset_interaction();
    }
    if keys.just_pressed(KeyCode::ArrowRight) {
        run.t += SCRUB_S;
        run.reset_interaction();
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        run.speed = run.speed.saturating_sub(1);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        run.speed = (run.speed + 1).min(SPEEDS.len() - 1);
    }
    if run.paused && mouse.pressed(MouseButton::Left) {
        run.orbit += Vec2::new(-motion.delta.x, -motion.delta.y) * 0.005;
    }
    let mut changed = false;
    if keys.just_pressed(KeyCode::KeyP) {
        settings.corner = match settings.corner {
            CornerSetting::All => CornerSetting::ThroughOnly,
            CornerSetting::ThroughOnly => CornerSetting::Off,
            CornerSetting::Off => CornerSetting::All,
        };
        changed = true;
    }
    if keys.just_pressed(KeyCode::KeyM) {
        settings.reduce_motion = !settings.reduce_motion;
    }
    if keys.just_pressed(KeyCode::KeyD) {
        settings.dose = match settings.dose {
            DoseScale::Absolute => DoseScale::Relative,
            DoseScale::Relative => DoseScale::Absolute,
        };
    }
    if keys.just_pressed(KeyCode::KeyG) {
        settings.glass = (settings.glass + 1) % 3;
    }
    if keys.just_pressed(KeyCode::KeyO) {
        settings.slice_ortho = !settings.slice_ortho;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        settings.expanded = !settings.expanded;
    }
    if keys.just_pressed(KeyCode::KeyL) {
        settings.axis_line = !settings.axis_line;
    }
    if changed {
        run.schedule(&library, &settings);
    }
    if keys.pressed(KeyCode::KeyV) {
        let t = run.t;
        run.own_glass_since.get_or_insert(t);
    } else {
        run.own_glass_since = None;
    }
    // Live controls act on the turn on show.
    let Some(showing) = corner.showing else {
        return;
    };
    let open = run.dismissed.is_none_or(|(turn, _)| turn != showing.turn);
    if keys.just_pressed(KeyCode::KeyX) && open {
        run.dismissed = Some((showing.turn, showing.c));
    }
    if keys.just_released(KeyCode::KeyC) && run.held_turn == Some(showing.turn) {
        run.dismissed = Some((showing.turn, showing.c));
    }
}

/// Advance the run clock and the hold; at the run's end, wait out the gap and advance.
fn advance(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    library: Res<Library>,
    settings: Res<Settings>,
    mut run: ResMut<Run>,
    corner: Res<Corner>,
) {
    if run.paused {
        return;
    }
    let dt = time.delta_secs() * SPEEDS[run.speed];
    if let Some(showing) = corner.showing
        && keys.pressed(KeyCode::KeyC)
        && showing.c >= showing.kind.hold_from()
        && run.dismissed.is_none_or(|(turn, _)| turn != showing.turn)
    {
        if run.held_turn != Some(showing.turn) {
            run.held_turn = Some(showing.turn);
        }
        run.held += dt;
    }
    let extra = run.held;
    if run.t < run.end + extra {
        run.t += dt;
        return;
    }
    if run.once {
        return;
    }
    let now = run.t;
    let finished = *run.finished_at.get_or_insert(now);
    run.t += dt;
    if run.auto && run.t - finished >= GAP_S {
        let next = (run.entry + 1) % PLAYLIST.len();
        let once = run.once;
        *run = Run::new(next, &library, &settings, once);
    }
}

/// Which turn the corner shows at the run's clock, and how far into it.
fn corner_now(run: Res<Run>, library: Res<Library>, mut corner: ResMut<Corner>) {
    let mut now = Corner::default();
    if let Some((shot, beats)) = run.kill {
        // The kill cam's lesson drives the stage's clock; the corner stays shut.
        now.showing = killcam::lesson_showing(beats, run.t, run.shots[shot].record);
    } else if let Some(since) = run.own_glass_since
        && let Some(record) = own_glass_record(&library)
    {
        // The own-tank glass on demand: open while the key is down.
        let c = (run.t - since).max(0.0);
        let (opacity, scale) = timeline::presence(c.min(1.0), f32::INFINITY);
        now.showing = Some(Showing {
            record,
            turn: usize::MAX,
            kind: CornerKind::Victim,
            c: c.min(CornerKind::Victim.hold_from()),
            opacity,
            scale,
            remaining: 1.0,
            surface: Surface::Corner,
        });
    } else {
        let held = |turn: usize| {
            if run.held_turn == Some(turn) {
                run.held
            } else {
                0.0
            }
        };
        for (index, turn) in run.turns.iter().enumerate() {
            let kind = run.kind_of(turn.record, &library);
            let c = run.t - turn.start - held(index);
            let mut duration = kind.duration();
            if let Some((dismissed, at)) = run.dismissed
                && dismissed == index
            {
                duration = duration.min(at + timeline::CLOSE_S);
            }
            let next = run.turns.get(index + 1);
            let replaced = next.is_some_and(|next| run.t >= next.start);
            if c < 0.0 || c > duration || replaced {
                continue;
            }
            let previous_open = index > 0 && {
                let previous = &run.turns[index - 1];
                let previous_kind = run.kind_of(previous.record, &library);
                turn.start < previous.start + previous_kind.duration()
            };
            let (mut opacity, mut scale) = timeline::presence(c, duration);
            if previous_open && c < timeline::OPEN_S {
                // A replacement: the frame stays open and its content crossfades.
                (opacity, scale) = (1.0, 1.0);
                now.fading = Some(1.0 - timeline::progress(c, 0.0, timeline::CROSSFADE_S));
            }
            now.showing = Some(Showing {
                record: run.shots[turn.record].record,
                turn: index,
                kind,
                c,
                opacity,
                scale,
                remaining: 1.0 - (c / duration).clamp(0.0, 1.0),
                surface: Surface::Corner,
            });
        }
    }
    if *corner != now {
        *corner = now;
    }
}

/// The record the own-tank glass shows: the latest record that struck the player, which in the
/// demo is the victim scenario's.
fn own_glass_record(library: &Library) -> Option<usize> {
    PLAYLIST
        .iter()
        .filter(|entry| entry.victim)
        .flat_map(|entry| entry.shots.iter())
        .last()
        .map(|(stem, _)| library.index(stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every playlist record loads and plans, every entry's corner closes inside the 4.5 s cap,
    /// and every shot's corner opens 0.2 s after its impact unless a turn before it is still
    /// inside its minimum.
    #[test]
    fn every_entry_schedules_inside_the_cap() {
        let library = Library::load();
        let settings = Settings::default();
        for (index, entry) in PLAYLIST.iter().enumerate() {
            let run = Run::new(index, &library, &settings, true);
            if run.kill.is_some() {
                assert!(run.turns.is_empty(), "{}", entry.label);
                continue;
            }
            assert!(!run.turns.is_empty(), "{}", entry.label);
            for turn in &run.turns {
                let shot = &run.shots[turn.record];
                assert!(turn.start >= shot.arrival - 1e-6);
                assert!(run.kind_of(turn.record, &library).duration() <= 4.5);
            }
            assert!(run.end > run.turns.last().unwrap().start);
        }
    }

    /// The setting gates which verdicts open a corner.
    #[test]
    fn the_setting_gates_the_corner() {
        let library = Library::load();
        let through_only = Settings {
            corner: CornerSetting::ThroughOnly,
            ..Settings::default()
        };
        let off = Settings {
            corner: CornerSetting::Off,
            ..Settings::default()
        };
        for entry in 0..PLAYLIST.len() {
            let run = Run::new(entry, &library, &through_only, true);
            for turn in &run.turns {
                let record = &library.records[run.shots[turn.record].record];
                assert_eq!(record.report.impact.verdict, ShotVerdict::Penetration);
            }
            assert!(Run::new(entry, &library, &off, true).turns.is_empty());
        }
    }

    /// The paired entry fires its second record 0.8 s after the first, and it replaces the first
    /// on arrival.
    #[test]
    fn the_paired_entry_replaces_its_first_record() {
        let library = Library::load();
        let entry = PLAYLIST.iter().position(|e| e.shots.len() == 2).unwrap();
        let run = Run::new(entry, &library, &Settings::default(), true);
        assert_eq!(run.turns.len(), 2);
        assert!((run.turns[1].start - run.turns[0].start - 0.8).abs() < 0.05);
    }
}
