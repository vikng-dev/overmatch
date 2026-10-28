//! The pen cam demo: the in-battle flow of a hit, end to end, played over the recorded scenario
//! records in `assets/dev/pen_cam/`. The shooter's sight shows the struck tank, the round flies,
//! the world shows the impact, and the corner opens and closes on its own: the presentation
//! ([`super::present`]) driven from a run of the playlist.
//!
//! Run it: `cargo run --bin pen_cam_demo` (optionally a scenario label, `pen_cam_demo s4`, and
//! `--once` to stop after it). Keys, the game's: `Esc` dismiss, hold `C` to hold; `P` cycles the
//! corner setting (all / through only / off); demo-only `1`–`9`, `0`, `-`, `=`, `Backspace` pick
//! a scenario (`=` is K1, the victim kill cam, where `R` respawns from 0.5 s after the hit and a
//! left-drag looks while the camera is free; the burst scenarios past the keys play by their tag,
//! `pen_cam_demo s13`, or auto-advance), `A` toggles auto-advance, `R` replays, `Space`
//! pauses, `←`/`→` scrub, `[`/`]` change speed, mouse drag orbits the corner while paused, `F`
//! expands the corner, `O` switches the slice between orthographic and perspective, `L` toggles
//! each field's axis line and `M` toggles reduce motion; captures set the expanded corner, the
//! slice's lens and a frame for every shooter record through [`Settings::from_env`].
//!
//! Capture: with `OVERMATCH_CAPTURE` set the clock steps a fixed 1/30 s per frame, so
//! `dev_capture`'s frame times are presentation times; virtual time stays paused until both tank
//! views have spawned, so a frame time never lands on a loading screen.

use std::sync::Arc;
use std::time::Duration;

use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::plan::Plan;
use super::timeline::{self, CornerKind, Turn};
use super::{Corner, Exhibit, Exhibits, Knobs, Showing, Surface};
use super::{hud, killcam, look, stage, view, world};
use crate::damage_cam::shapes::TankAsset;
use crate::damage_cam::{PenetrationReport, ShotVerdict};

/// The records, relative to the asset root.
const RECORD_DIR: &str = "dev/pen_cam";

/// One scenario of the demo: the records it fires, each at its offset (s) from the first.
pub struct Entry {
    pub label: &'static str,
    pub shots: &'static [(&'static str, f32)],
    /// The struck player's side: the own-tank glass, not the shooter's corner.
    pub victim: bool,
}

/// The playlist, in key order (`1`–`9`, `0`, `-`, `=`, `Backspace`; the entries past the keys by
/// their tag or auto-advance). Fixture data: which recorded shots the demo plays and how they are
/// paired.
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
        label: "S9 own tank hit, breech and loader out (victim glass)",
        shots: &[("s9_breech_and_loader", 0.0)],
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
    Entry {
        label: "S12 sub-calibre on S1's line, the core through, a weaker cone",
        shots: &[("s12_apcr_through_crew", 0.0)],
        victim: false,
    },
    Entry {
        label: "S13 burst round, ring: a band about the line, the nose piece flies on",
        shots: &[("s13_ring_through_crew", 0.0)],
        victim: false,
    },
    Entry {
        label: "S16 burst at rest in the hull side, the plate stops its field",
        shots: &[("s16_restart_at_rest", 0.0)],
        victim: false,
    },
];

pub(super) const ENTRY_KEYS: [KeyCode; 13] = [
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
    KeyCode::Backspace,
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
    pub plan: Arc<Plan>,
    /// Whether the record knocks the struck tank out, by the game's rule ([`killcam::knockout`]).
    pub knockout: Option<crate::damage::KnockoutReason>,
    /// Who fired it, as the victim's copy names them.
    pub killer: killcam::Killer,
}

impl Library {
    pub(super) fn load() -> Self {
        Self::of(Self::reports())
    }

    /// The playlist's records as they stand on disk, each once, by stem.
    pub(super) fn reports() -> Vec<(&'static str, PenetrationReport)> {
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
        reports
    }

    /// The library of `reports`, staged on the one vehicle they name.
    pub(super) fn of(reports: Vec<(&'static str, PenetrationReport)>) -> Self {
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
                let unresolved = report.unresolved();
                assert!(
                    report.synthetic || unresolved.is_empty(),
                    "{stem}: {unresolved:?}"
                );
                let plan = Arc::new(Plan::new(&report, &asset.shapes(&report.servo_deg)));
                let knockout = killcam::knockout(&report);
                let killer = killcam::Killer::of(report.shooter.as_ref(), &report.round);
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

    /// Every record as the stage draws it, by the library's index.
    pub(super) fn exhibits(&self) -> Exhibits {
        Exhibits(
            self.records
                .iter()
                .map(|record| Exhibit {
                    plan: record.plan.clone(),
                    servo_deg: record.report.servo_deg.clone(),
                })
                .collect(),
        )
    }
}

/// The corner setting a player picks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CornerSetting {
    All,
    ThroughOnly,
    Off,
}

/// The demo's switches.
#[derive(Resource, Clone, PartialEq)]
pub struct Settings {
    pub corner: CornerSetting,
    pub reduce_motion: bool,
    pub slice_ortho: bool,
    pub expanded: bool,
    pub axis_line: bool,
    /// The frame every shooter record shows in, in place of its verdict's: the bench's way to see
    /// one record in another frame. `None` keeps each record's own.
    pub frame: Option<CornerKind>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            corner: CornerSetting::All,
            reduce_motion: false,
            slice_ortho: true,
            expanded: false,
            axis_line: false,
            frame: None,
        }
    }
}

impl Settings {
    /// The defaults with the capture environment applied: `PEN_CAM_EXPANDED=1` expands the corner,
    /// `PEN_CAM_SLICE_ORTHO=0` draws the slice in perspective, and `PEN_CAM_FRAME` (`glass`,
    /// `slice`, `ricochet` or `damaged`) shows every shooter record in that frame.
    fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            expanded: std::env::var("PEN_CAM_EXPANDED").is_ok_and(|value| value == "1"),
            slice_ortho: std::env::var("PEN_CAM_SLICE_ORTHO")
                .map_or(defaults.slice_ortho, |value| value != "0"),
            frame: std::env::var("PEN_CAM_FRAME")
                .ok()
                .and_then(|value| match value.as_str() {
                    "glass" => Some(CornerKind::Glass),
                    "slice" => Some(CornerKind::Slice),
                    "ricochet" => Some(CornerKind::Ricochet),
                    "damaged" => Some(CornerKind::Damaged),
                    _ => None,
                }),
            ..defaults
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
    /// Mouse orbit applied to the corner camera while paused (yaw, pitch; rad).
    pub orbit: Vec2,
    /// The shot that knocks the player's own tank out, and the kill cam's beats; the corner stays
    /// shut on such a run.
    pub kill: Option<(usize, killcam::Beats)>,
    /// Run seconds at which the player pressed respawn on a kill run: the death panel reads
    /// [`killcam::RESPAWNING`] for [`killcam::RESPAWN_S`], then the run starts a fresh life.
    pub respawned_at: Option<f32>,
    /// [`Settings::frame`] as the run was laid out under it.
    pub frame: Option<CornerKind>,
}

impl Run {
    pub(super) fn new(entry: usize, library: &Library, settings: &Settings, once: bool) -> Self {
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
            orbit: Vec2::ZERO,
            kill: None,
            respawned_at: None,
            frame: settings.frame,
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
                let muzzle = staged_muzzle(report);
                let impact = fire + muzzle.distance(Vec3::from(report.impact.point)) / speed;
                ShotRun {
                    record,
                    fire,
                    impact,
                    arrival: impact + timeline::OPEN_LATENCY_S,
                    muzzle,
                }
            })
            .collect();
        self.kill = entry
            .victim
            .then(|| {
                self.shots.iter().enumerate().find_map(|(index, shot)| {
                    let record = &library.records[shot.record];
                    record.knockout.map(|_| {
                        (
                            index,
                            killcam::Beats {
                                impact: shot.impact,
                                shooter: record.report.shooter.is_some(),
                                lesson_s: killcam::lesson_length(Some(&record.plan)),
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
            .map(|turn| turn.start + self.pace_of(turn.record, library).duration)
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
            self.frame
                .unwrap_or(library.records[self.shots[shot].record].plan.kind)
        }
    }

    /// Shot `shot`'s corner times in its frame ([`stage::pace`]).
    pub fn pace_of(&self, shot: usize, library: &Library) -> timeline::Pace {
        let plan = &library.records[self.shots[shot].record].plan;
        stage::pace(plan, self.kind_of(shot, library))
    }

    /// Drop beat 3 from the kill run, whose shooter cannot be staged: the death screen follows the
    /// lesson.
    pub fn drop_killer_beat(&mut self) {
        if let Some((shot, beats)) = self.kill {
            let beats = killcam::Beats {
                shooter: false,
                ..beats
            };
            self.kill = Some((shot, beats));
            self.end = beats.end();
        }
    }

    fn reset_interaction(&mut self) {
        self.held = 0.0;
        self.held_turn = None;
        self.dismissed = None;
        self.finished_at = None;
    }
}

/// Where the round is fired from on the battlefield: the record's muzzle where the record has one,
/// else staged [`world::SHOOTER_DISTANCE_M`] back along the record's own first leg (origin to
/// impact).
pub fn staged_muzzle(report: &PenetrationReport) -> Vec3 {
    let impact = Vec3::from(report.impact.point);
    report.muzzle.map(Vec3::from).unwrap_or_else(|| {
        let incoming = (impact - Vec3::from(report.origin)).normalize_or(Vec3::NEG_Z);
        impact - incoming * world::SHOOTER_DISTANCE_M
    })
}

/// Set once both tank views have spawned and been dressed.
#[derive(Resource, Default)]
pub struct Ready(pub bool);

/// The sight's reticle.
#[derive(Component)]
pub(super) struct Reticle;

/// The demo's own labels.
#[derive(Component)]
pub(super) struct Label;

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
    let settings = Settings::from_env();
    let run = Run::new(start, &library, &settings, once);
    if std::env::var_os("OVERMATCH_CAPTURE").is_some() {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(CAPTURE_STEP));
    }
    app.add_plugins((
        crate::quit::plugin,
        crate::spec::plugin,
        crate::render_policy::plugin,
        crate::ui_font::plugin,
    ));
    super::materials(app);
    app.insert_resource(ClearColor(world::SKY))
        .insert_resource(library)
        .insert_resource(settings)
        .insert_resource(run)
        .add_systems(Startup, (hold_clock, view::spawn, world::spawn))
        .add_systems(
            Startup,
            (stage::spawn, hud::spawn, spawn_hud, killcam::spawn).after(view::spawn),
        );
    sequence(app);
}

/// The demo's frame: the presentation's resources and the one ordered chain every frame runs, from
/// the views' preparation through input, the clock, the camera's writers and the drawing. Tests
/// drive this same wiring.
pub(super) fn sequence(app: &mut App) {
    super::presentation(app);
    app.init_resource::<Ready>()
        .init_resource::<killcam::KillCam>()
        .add_systems(
            Update,
            (
                exhibit,
                view::prepare_stage,
                view::prepare,
                release_clock,
                controls,
                knobs,
                killcam::input,
                advance,
                corner_now,
                view::pose_stage,
                view::pose,
                killcam::stage,
                camera_writers(),
                hud::layout,
                look::own,
                hud::detail,
                super::present(),
                world::animate,
                labels,
                killcam::hud,
            )
                .chain(),
        );
}

/// The main camera's writers, one per run: the shooter's sight or the victim's chase view, or the
/// kill cam. Effects that read the camera (the tracer's near cull) run after them.
pub(super) fn camera_writers() -> ScheduleConfigs<ScheduleSystem> {
    (
        world::place_camera.run_if(killcam::inactive),
        killcam::place.run_if(killcam::active),
    )
        .chain()
        .into_configs()
}

/// The library's records as the presentation's exhibits, by the library's index.
fn exhibit(library: Res<Library>, mut exhibits: ResMut<Exhibits>) {
    if exhibits.0.len() != library.records.len() {
        *exhibits = library.exhibits();
    }
}

/// The presentation's knobs from the demo's switches and the paused drag.
fn knobs(settings: Res<Settings>, run: Res<Run>, mut knobs: ResMut<Knobs>) {
    let want = Knobs {
        reduce_motion: settings.reduce_motion,
        slice_ortho: settings.slice_ortho,
        expanded: settings.expanded,
        axis_line: settings.axis_line,
        orbit: run.orbit,
    };
    if *knobs != want {
        *knobs = want;
    }
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
        // Respawn, the game's binding; `advance` completes it.
        if beats.respawn_live(run.t) && run.respawned_at.is_none() {
            run.respawned_at = Some(run.t);
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
    // Live controls act on the turn on show.
    let Some(showing) = corner.showing else {
        return;
    };
    let open = run.dismissed.is_none_or(|(turn, _)| turn != showing.turn);
    if keys.just_pressed(KeyCode::Escape) && open {
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
    cam: Res<killcam::KillCam>,
) {
    if let Some(at) = run.respawned_at {
        // The demo's respawn: a fresh life of the same scenario, paused on the living tank.
        run.t += time.delta_secs();
        if run.t - at >= killcam::RESPAWN_S {
            let (entry, once) = (run.entry, run.once);
            *run = Run::new(entry, &library, &settings, once);
            run.auto = false;
            run.paused = true;
        }
        return;
    }
    if run.paused || killcam::holding(&run, &cam) {
        return;
    }
    let dt = time.delta_secs() * SPEEDS[run.speed];
    if let Some(showing) = corner.showing
        && keys.pressed(KeyCode::KeyC)
        && showing.c >= stage::pace(&library.records[showing.record].plan, showing.kind).hold_from
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
            let mut duration = run.pace_of(turn.record, &library).duration;
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
            let replacing = index > 0 && {
                let previous = &run.turns[index - 1];
                turn.start < previous.start + run.pace_of(previous.record, &library).duration
            };
            let (opacity, scale, fading) = timeline::turn_presence(c, duration, replacing);
            now.fading = fading;
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

/// The sight's reticle and the demo's labels, in Bevy's default font, ASCII only.
fn spawn_hud(mut commands: Commands) {
    // The reticle: four bars around the screen's centre.
    commands
        .spawn((
            Reticle,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn(Node {
                width: Val::Px(64.0),
                height: Val::Px(64.0),
                ..default()
            })
            .with_children(|cross| {
                let bar = |left: f32, top: f32, w: f32, h: f32| {
                    (
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(left),
                            top: Val::Px(top),
                            width: Val::Px(w),
                            height: Val::Px(h),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.05, 0.05, 0.05, 0.85)),
                    )
                };
                cross.spawn(bar(0.0, 31.0, 24.0, 2.0));
                cross.spawn(bar(40.0, 31.0, 24.0, 2.0));
                cross.spawn(bar(31.0, 40.0, 2.0, 24.0));
                cross.spawn(bar(31.0, 0.0, 2.0, 18.0));
            });
        });
    commands.spawn((
        Label,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(13.0),
            ..default()
        },
        TextColor(Color::srgba(0.95, 0.96, 0.98, 0.85)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(14.0),
            bottom: Val::Px(12.0),
            ..default()
        },
    ));
}

/// What `R` does on this run, as the legend names it: on a kill run it respawns, the game's
/// binding, and a drag looks while the camera is free.
pub(super) fn legend_r(kill: bool) -> &'static str {
    if kill {
        "R respawn  drag look"
    } else {
        "R replay"
    }
}

/// The reticle on the shooter's runs, and the labels.
fn labels(
    run: Res<Run>,
    library: Res<Library>,
    settings: Res<Settings>,
    mut reticle: Query<&mut Visibility, With<Reticle>>,
    mut label: Query<&mut Text, With<Label>>,
) {
    let entry = &PLAYLIST[run.entry];
    if let Ok(mut visibility) = reticle.single_mut() {
        visibility.set_if_neq(if entry.victim {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        });
    }
    let Ok(mut text) = label.single_mut() else {
        return;
    };
    let verdicts: Vec<String> = run
        .shots
        .iter()
        .map(|shot| format!("{:?}", library.records[shot.record].report.impact.verdict))
        .collect();
    let corner_setting = match settings.corner {
        CornerSetting::All => "all",
        CornerSetting::ThroughOnly => "through only",
        CornerSetting::Off => "off",
    };
    let line = format!(
        "{}  [{}]\nt {:.2} s  x{}{}{}  corner {}  slice {}{}\n\
         1-9 0 - = Backspace scenario  A auto  {}  Space pause  Left/Right scrub  [ ] speed  \
         Esc dismiss  C hold  P setting  F expand  O ortho  L axis  M reduce motion",
        entry.label,
        verdicts.join(", "),
        run.t,
        SPEEDS[run.speed],
        if run.paused { "  paused" } else { "" },
        if run.auto { "  auto" } else { "" },
        corner_setting,
        if settings.slice_ortho {
            "ortho"
        } else {
            "perspective"
        },
        if settings.reduce_motion {
            "  reduce motion"
        } else {
            ""
        },
        legend_r(run.kill.is_some()),
    );
    if text.0 != line {
        text.0 = line;
    }
}
