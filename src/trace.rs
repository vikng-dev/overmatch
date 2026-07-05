//! The jitter-trace recorder: an env-gated, per-frame/per-tick JSONL log of the rendered vs.
//! simulated tank pose, rollback events, and correction decay — the raw material an offline Python
//! script graphs to explain MP hull jitter (worst under collision/suspension stress; SP is smooth).
//!
//! A PASSIVE observer, like [`crate::net::diagnostics`]: nothing here writes sim state. Every system
//! reads and appends a line, nothing more. The whole module is OFF unless `SPIKE_TRACE=<path>` is
//! set at startup — [`install`] opens the file only then and reports whether it armed, so the
//! recorder systems/observer are REGISTERED only in a traced run. An unset env var costs one
//! `std::env::var` lookup at plugin-build and nothing thereafter (no per-frame run conditions).
//!
//! The sink path is role-qualified so a server and client launched from one shell with the same
//! `SPIKE_TRACE` value never truncate each other's file: `SPIKE_TRACE=/tmp/t.jsonl` writes
//! `/tmp/t.client.jsonl` on the client, `/tmp/t.server.jsonl` on the server, `/tmp/t.sp.jsonl` in
//! single-player (a value with no extension gets `.<role>.jsonl` appended). The startup `info!`
//! line prints the RESOLVED path, not the raw env value.
//!
//! ## Row schema (one compact JSON object per line, `k` = kind)
//! - `meta`   — once, at Startup: `role` ("sp"|"client"|"server"), `tick_hz` (derived from the app's
//!   `Time<Fixed>` timestep, not hardcoded), `ver` (crate version). Written first (Startup runs
//!   before any Update/FixedUpdate recorder).
//! - `frame`  — per render frame, per tank root, AFTER transform propagation, so `p`/`q` are what
//!   actually renders (`GlobalTransform`): `t` wall-secs, `dt` frame delta, `os` fixed overstep,
//!   `e` entity bits, `p`/`q` world pose, `ctl` (present+true only for the controlled tank). Client
//!   extras: `tick` (predicted), `conf` (GLOBAL last-confirmed server tick or null — the whole
//!   replication stream's high-water mark), `rb`/`rbt` (cumulative rollback / rolled-back-tick
//!   counts), `cp`/`cq` (live `VisualCorrection` error translation/quat, present only while a
//!   correction decays). Per-ENTITY confirmed authority for the predicted tank root, from its
//!   `ConfirmedHistory<C>` newest present sample: `confp` [x,y,z] latest confirmed `Position`,
//!   `confv` [x,y,z] latest confirmed `LinearVelocity`, `conft` the lightyear tick that
//!   `confp`/`confv` belong to. This tick is the entity's OWN last authoritative update — it can
//!   lag the global `conf` when the tank's replicated components stop changing (or stop arriving),
//!   the key discriminator for a silent desync. All three are omitted (not null) when no confirmed
//!   sample exists yet, and absent entirely in SP / SP-composition net builds (no `ConfirmedHistory`).
//! - `tick`   — per fixed tick, per tank root: sim truth — `p`/`q`/`lv`/`av`, `gnd` grounded wheel
//!   count, `anc` planted anchor count, `loads` per-wheel spring load (N, ~0.1 N), `thr`/`str` drive
//!   intent, `hc` hull contact-pair count, `pen` max penetration depth (m). Client-only: `rp`
//!   (present+true only on a rollback-REPLAY tick — the corrected re-simulation of an
//!   already-recorded tick number; absent on original ticks and in SP/server rows, which never
//!   replay). Analysis keeps the LAST row per (tick, entity), i.e. the replayed/corrected value.
//! - `rollback` — client only, one per rollback start: `t`, `tick`, `start`, `depth` (tick−start),
//!   `cause` ("state"|"input"), `trg` the [component, magnitude] pairs whose rollback condition
//!   tripped during THIS `check_rollback` — exact per-check attribution: the slot is cleared each
//!   frame before lightyear's rollback check, so the correction-decay re-tests (which reuse the same
//!   registered conditions) can't leak in.
//!
//! Analysis aligns rows across processes on `tick` + `role`, never on `e` (entity ids differ per
//! process). Rows are line-buffered through a `BufWriter` flushed every ~1 s (`on_timer`) and on a
//! clean World drop (`BufWriter::drop`); a hard-killed process may lose the unflushed tail
//! (acceptable).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use avian3d::prelude::{AngularVelocity, Collisions, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use serde_json::{Value, json};

use crate::driving::{DriveState, Suspension};
use crate::tank::{Controlled, Tank, TankSim, WheelIndex};

#[cfg(feature = "net")]
use bevy::ecs::system::SystemParam;
#[cfg(feature = "net")]
use lightyear::core::confirmed_history::ConfirmedHistory;
#[cfg(feature = "net")]
use lightyear::prelude::{
    LocalTimeline, PredictionManager, PredictionMetrics, ReplicationCheckpointMap, Rollback,
    RollbackSystems, VisualCorrection,
};

/// The open trace sink. Present iff `SPIKE_TRACE` was set at startup — [`install`] both inserts it
/// and returns whether it did, so the recorder systems gate on that return value at registration
/// time rather than on a per-frame `resource_exists` check.
#[derive(Resource)]
struct TraceWriter {
    writer: BufWriter<File>,
    /// The composition role, carried so the Startup `write_meta` system can stamp the `meta` row
    /// (it runs after `install`, which no longer writes the row itself — see fix 6).
    role: &'static str,
}

impl TraceWriter {
    /// Append one row, best-effort. A passive observer never lets an I/O hiccup disturb the sim, so
    /// write errors are dropped (the periodic flush + the parse check surface a broken file).
    fn write(&mut self, row: &Value) {
        let _ = writeln!(self.writer, "{row}");
    }
}

// --- JSON leaf helpers -----------------------------------------------------------------------
// serde_json's `From<f64>` maps NaN/Inf to `Null` rather than emitting invalid JSON — exactly the
// safety a pose recorder needs, since a corrupt frame must still produce a parseable line.

fn num(x: f32) -> Value {
    Value::from(x as f64)
}

fn vec3(v: Vec3) -> Value {
    Value::Array(vec![num(v.x), num(v.y), num(v.z)])
}

fn quat(q: Quat) -> Value {
    Value::Array(vec![num(q.x), num(q.y), num(q.z), num(q.w)])
}

/// Insert `role` before the extension of the raw `SPIKE_TRACE` value, so concurrently-launched
/// processes sharing one value write to distinct files. `/tmp/t.jsonl` → `/tmp/t.<role>.jsonl`; a
/// value with no extension gets `.<role>.jsonl` appended (`/tmp/t` → `/tmp/t.<role>.jsonl`).
fn role_path(path: &str, role: &str) -> PathBuf {
    let p = Path::new(path);
    if let (Some(stem), Some(ext)) = (p.file_stem(), p.extension()) {
        let mut name = stem.to_os_string();
        name.push(".");
        name.push(role);
        name.push(".");
        name.push(ext);
        return match p.parent() {
            Some(parent) => parent.join(name),
            None => PathBuf::from(name),
        };
    }
    PathBuf::from(format!("{path}.{role}.jsonl"))
}

/// Open the role-qualified sink and register the shared meta/flush systems — only when `SPIKE_TRACE`
/// is set. Returns `true` iff tracing is armed, so each composition plugin registers its recorders
/// only in a traced run (dropping per-frame `resource_exists` gating). Called once per composition
/// root.
fn install(app: &mut App, role: &'static str) -> bool {
    let Ok(path) = std::env::var("SPIKE_TRACE") else {
        return false;
    };
    let resolved = role_path(&path, role);
    let file = match File::create(&resolved) {
        Ok(file) => file,
        Err(err) => {
            error!("trace: cannot open {}: {err}", resolved.display());
            return false;
        }
    };
    info!("trace: recording {role} rows to {}", resolved.display());
    app.insert_resource(TraceWriter {
        writer: BufWriter::new(file),
        role,
    });
    // The `meta` row is written from Startup (not here) so `tick_hz` can read the app's actual
    // `Time<Fixed>` timestep, which may not be configured at plugin-build time. Startup runs before
    // any Update/FixedUpdate recorder, so it stays the first row.
    app.add_systems(Startup, write_meta);
    // Flushing is best-effort tail protection: line-buffered writes reach disk every ~1 s and on a
    // clean exit (the `BufWriter` drops with the World and flushes); a killed process may lose the
    // unflushed remainder (accepted).
    app.add_systems(Last, flush_periodically.run_if(on_timer(Duration::from_secs(1))));
    // Arm the trigger-attribution slot's fast path. The slot only fills on the client (check_rollback
    // is client-only), but the flag is role-agnostic and cheap; the server never calls
    // `note_rollback_trigger`, so its slot stays empty regardless.
    #[cfg(feature = "net")]
    TRACE_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    true
}

/// Single-player: frame + tick recorders, no net extras (the cfg-gated fields simply don't exist in
/// this build). Two tanks spawn; both are recorded, told apart by `e`/`ctl` in analysis.
pub fn sp_plugin(app: &mut App) {
    if !install(app, "sp") {
        return;
    }
    app.add_systems(PostUpdate, record_frame.after(TransformSystems::Propagate));
    app.add_systems(FixedLast, record_tick);
}

/// MP client: frame (with prediction/correction extras) + tick + the rollback observer. Replay ticks
/// ARE recorded (stamped `rp` — fix 4), so analysis sees the corrected re-simulation, not the
/// abandoned misprediction. A PreUpdate system clears the trigger slot before lightyear's rollback
/// check so `trg` attribution is exact (fix 2).
#[cfg(feature = "net")]
pub fn client_plugin(app: &mut App) {
    if !install(app, "client") {
        return;
    }
    app.add_systems(PostUpdate, record_frame.after(TransformSystems::Propagate));
    app.add_systems(FixedLast, record_tick);
    // Clear last frame's accumulated triggers BEFORE `check_rollback` runs, so the slot the rollback
    // observer drains holds only this check's trips (see `clear_rollback_triggers`).
    app.add_systems(
        PreUpdate,
        clear_rollback_triggers.before(RollbackSystems::Check),
    );
    app.add_observer(record_rollback);
}

/// MP server: tick rows only (it has no `Predicted` view to render, hence no frame/rollback rows).
#[cfg(feature = "net")]
pub fn server_plugin(app: &mut App) {
    if !install(app, "server") {
        return;
    }
    app.add_systems(FixedLast, record_tick);
}

/// Write the `meta` row once, at Startup: `tick_hz` is derived from the configured `Time<Fixed>`
/// timestep here (not hardcoded), and Startup precedes every Update/FixedUpdate recorder, so this
/// stays the file's first row.
fn write_meta(mut trace: ResMut<TraceWriter>, fixed: Res<Time<Fixed>>) {
    let role = trace.role;
    let tick_hz = (1.0 / fixed.timestep().as_secs_f64()).round() as u64;
    let meta = json!({ "k": "meta", "role": role, "tick_hz": tick_hz, "ver": env!("CARGO_PKG_VERSION") });
    trace.write(&meta);
}

/// Per render frame, per tank root: the pose that actually rendered this frame. Ordered
/// `after(TransformSystems::Propagate)` so `GlobalTransform` already reflects the frame-interpolated
/// + correction-adjusted `Position`/`Rotation` (MP) or avian's render interpolation (SP).
///
/// The net extras ride cfg-gated parameters (`net`, `corr`): the whole system compiles in the
/// no-net SP build with those params (and the block that fills them) removed, so nothing from
/// lightyear leaks outside `#[cfg(feature = "net")]`.
fn record_frame(
    mut trace: ResMut<TraceWriter>,
    real: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    roots: Query<(Entity, &GlobalTransform, Has<Controlled>), With<Tank>>,
    #[cfg(feature = "net")] net: NetFrameCtx,
    #[cfg(feature = "net")] corr: Query<(
        Option<&VisualCorrection<Position>>,
        Option<&VisualCorrection<Rotation>>,
    )>,
    // The predicted root's confirmed-authority history: lightyear seeds these buffers from every
    // replication receive (`add_confirmed_to_history`) and `prepare_rollback` reads them as the
    // rollback source. `Option<&…>` per component and an unfiltered query (like `corr`) so the
    // SP-composition net build — which never registers `ConfirmedHistory` — yields `(None, None)`
    // rather than failing system-param validation.
    #[cfg(feature = "net")] conf: Query<(
        Option<&ConfirmedHistory<Position>>,
        Option<&ConfirmedHistory<LinearVelocity>>,
    )>,
) {
    for (entity, global, controlled) in &roots {
        let (_, rotation, translation) = global.to_scale_rotation_translation();
        let mut row = json!({
            "k": "frame",
            "t": real.elapsed_secs() as f64,
            "dt": real.delta_secs() as f64,
            "os": fixed.overstep_fraction() as f64,
            "e": entity.to_bits(),
            "p": vec3(translation),
            "q": quat(rotation),
        });
        let obj = row.as_object_mut().expect("json! built an object");
        if controlled {
            obj.insert("ctl".into(), Value::Bool(true));
        }
        #[cfg(feature = "net")]
        {
            // The prediction/replication resources exist together on a real MP client. On an
            // SP-composition net build (`cargo run --features net` mounts `trace::sp_plugin`, but
            // here `client_plugin` — the point is the resources may be absent) none are present, so
            // access them optionally and skip the net extras rather than panic on system-param
            // validation (fix 3): the row then has the same shape as an SP frame row.
            if let (Some(timeline), Some(checkpoints), Some(metrics)) = (
                net.timeline.as_deref(),
                net.checkpoints.as_deref(),
                net.metrics.as_deref(),
            ) {
                obj.insert("tick".into(), Value::from(u64::from(timeline.tick().0)));
                obj.insert(
                    "conf".into(),
                    checkpoints
                        .last_confirmed_tick()
                        .map_or(Value::Null, |t| Value::from(u64::from(t.0))),
                );
                obj.insert("rb".into(), Value::from(metrics.rollbacks));
                obj.insert("rbt".into(), Value::from(metrics.rollback_ticks));
            }
            // `VisualCorrection` sits on the predicted root only while an error decays — omit the
            // field entirely when at rest, so its mere presence marks a live correction. (No
            // corrections exist in the SP-composition build, so the lookup yields nothing there.)
            if let Ok((cp, cq)) = corr.get(entity) {
                if let Some(correction) = cp {
                    obj.insert("cp".into(), vec3(correction.error.0));
                }
                if let Some(correction) = cq {
                    obj.insert("cq".into(), quat(correction.error.0));
                }
            }
            // The LATEST confirmed (server-authoritative) Position/LinearVelocity this predicted
            // root has received, plus the tick it belongs to. `ConfirmedHistory::newest_present`
            // is the buffer's most-recent present sample — `(tick, value)` — exactly the sample
            // `prepare_rollback` prefers as the rollback source. `conft` is the entity's OWN last
            // authoritative tick; when it stalls while the global `conf` keeps advancing, the
            // tank's confirmed updates stopped arriving (silent-desync branch a). Omit all three
            // when the buffer is still empty — a not-yet-confirmed frame carries no `conf*`.
            if let Ok((confp, confv)) = conf.get(entity) {
                if let Some((tick, position)) = confp.and_then(|h| h.newest_present()) {
                    obj.insert("confp".into(), vec3(position.0));
                    obj.insert("conft".into(), Value::from(u64::from(tick.0)));
                }
                if let Some((_, velocity)) = confv.and_then(|h| h.newest_present()) {
                    obj.insert("confv".into(), vec3(velocity.0));
                }
            }
        }
        trace.write(&row);
    }
}

/// Per fixed tick, per tank root: sim truth (`Position`/`Rotation`/velocities are the rolled-back,
/// replayed authority values) plus the derived contact state (grounded wheels, brush anchors, spring
/// loads, collision pairs). Runs in `FixedLast`, after the physics step and avian's contact update,
/// so `Collisions` is current for this tick.
///
/// Replay ticks are recorded too (client): a tick re-simulated during rollback is stamped `rp` so
/// analysis keeps the corrected value over the abandoned misprediction for that same tick number
/// (fix 4). The tick number comes from lightyear's `LocalTimeline` under net; when that resource is
/// absent (SP, or an SP-composition net build) it falls back to a local monotonic counter.
fn record_tick(
    mut trace: ResMut<TraceWriter>,
    roots: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &DriveState,
            &TankSim,
            Has<Controlled>,
        ),
        With<Tank>,
    >,
    children: Query<&Children>,
    wheels: Query<(&WheelIndex, &Suspension)>,
    collisions: Collisions,
    mut tick_counter: Local<u64>,
    #[cfg(feature = "net")] timeline: Option<Res<LocalTimeline>>,
    // Same source `is_in_rollback` reads (`Query<(), With<Rollback>>`): non-empty iff this tick is a
    // rollback-replay re-simulation, which is what `rp` marks.
    #[cfg(feature = "net")] replaying: Query<(), With<Rollback>>,
) {
    // One tick number for every tank this call. Net: the predicted/authoritative tick, so client and
    // server rows align. SP (or an SP-composition net build with no `LocalTimeline`): a monotonic
    // local counter, incremented once per tick here — enough for the analysis script to order rows.
    #[cfg(feature = "net")]
    let tick_no = match timeline.as_deref() {
        Some(timeline) => u64::from(timeline.tick().0),
        None => next_local_tick(&mut tick_counter),
    };
    #[cfg(not(feature = "net"))]
    let tick_no = next_local_tick(&mut tick_counter);

    #[cfg(feature = "net")]
    let is_replay = !replaying.is_empty();

    // Per-body contact count + deepest penetration in ONE pass over the contact graph (fix 5), so
    // the roots loop below is a map lookup rather than a full re-scan per tank. A tank-vs-tank pair
    // counts for both roots, which is correct — each body has that contact. `body1`/`body2` are the
    // rigid-body entities (the tank root here, since the hull/part colliders are its children).
    let mut contacts: std::collections::HashMap<Entity, (u32, f32)> = std::collections::HashMap::new();
    for pair in collisions.iter() {
        let deepest = pair.find_deepest_contact().map_or(0.0, |point| point.penetration);
        for body in [pair.body1, pair.body2].into_iter().flatten() {
            let entry = contacts.entry(body).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 = entry.1.max(deepest);
        }
    }

    for (entity, position, rotation, linvel, angvel, drive, sim, controlled) in &roots {
        // Wheels in stable `WheelIndex` order (the slot into `TankSim::anchors`), so the per-wheel
        // `loads` array is comparable row-to-row. Only this tank's own descendant wheels, exactly as
        // `apply_suspension` scopes support.
        let mut slots: Vec<(usize, &Suspension)> = children
            .iter_descendants(entity)
            .filter_map(|wheel| wheels.get(wheel).ok())
            .map(|(index, suspension)| (index.0, suspension))
            .collect();
        slots.sort_by_key(|(index, _)| *index);
        let grounded = slots
            .iter()
            .filter(|(_, suspension)| suspension.contact.is_some())
            .count();
        let loads: Vec<Value> = slots
            .iter()
            // Round to ~0.1 N: solver-noise digits past that are neither meaningful nor worth the
            // row width.
            .map(|(_, suspension)| num((suspension.load * 10.0).round() / 10.0))
            .collect();
        let anchors = sim.anchors.iter().filter(|a| a.is_some()).count();

        // Contact pairs whose rigid body is this tank root, and the deepest penetration among them
        // — the collision-stress signal the jitter correlates with.
        let (hull_contacts, penetration) = contacts.get(&entity).copied().unwrap_or((0, 0.0));

        // `mut` used only by the net-only `rp` stamp below; unused in the SP build.
        #[cfg_attr(not(feature = "net"), allow(unused_mut))]
        let mut row = json!({
            "k": "tick",
            "tick": tick_no,
            "e": entity.to_bits(),
            "p": vec3(position.0),
            "q": quat(rotation.0),
            "lv": vec3(linvel.0),
            "av": vec3(angvel.0),
            "gnd": grounded,
            "anc": anchors,
            "loads": Value::Array(loads),
            "thr": num(drive.throttle()),
            "str": num(drive.steer()),
            "hc": hull_contacts,
            "pen": num(penetration),
            "ctl": controlled,
        });
        // Mark rollback-replay ticks so analysis keeps the corrected value for this tick number.
        #[cfg(feature = "net")]
        if is_replay {
            row.as_object_mut()
                .expect("json! built an object")
                .insert("rp".into(), Value::Bool(true));
        }
        trace.write(&row);
    }
}

/// Read-and-bump the single-player tick counter: there is no network tick, so this monotonic count
/// orders SP (and SP-composition net-build) rows.
fn next_local_tick(counter: &mut Local<u64>) -> u64 {
    let current = **counter;
    **counter += 1;
    current
}

/// Flush the buffer ~once a second (gated by `on_timer`) so a long run's rows reach disk
/// incrementally — a live tail is useful while a session is still running.
fn flush_periodically(mut trace: ResMut<TraceWriter>) {
    let _ = trace.writer.flush();
}

// --- Rollback trigger attribution (net only) -------------------------------------------------

/// Set once the trace writer opens, so the rollback-condition closures in `net::protocol` can skip
/// the mutex entirely when tracing is off — the cost of instrumentation is a single relaxed atomic
/// load on the check_rollback hot path.
#[cfg(feature = "net")]
static TRACE_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The trigger-attribution slot: component/magnitude pairs pushed by the rollback-condition closures
/// (`net::protocol`) as they trip, drained by [`record_rollback`] into the `trg` field. A plain
/// `static Mutex` rather than a resource because the closures are `Fn` values with no `World` access.
/// Capped at 64 to bound a pathological burst; excess is dropped.
#[cfg(feature = "net")]
static ROLLBACK_TRIGGERS: std::sync::Mutex<Vec<(&'static str, f32)>> =
    std::sync::Mutex::new(Vec::new());

/// Record that `component`'s rollback condition tripped this check, by `magnitude` (the measured
/// client/server divergence). Called from [`note_if_tripped`] when a condition returns true. No-op
/// when tracing is off (the atomic guard) — so the closures pay nothing in an untraced run, and the
/// server (which never runs check_rollback) never reaches the push.
#[cfg(feature = "net")]
fn note_rollback_trigger(component: &'static str, magnitude: f32) {
    if !TRACE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Ok(mut triggers) = ROLLBACK_TRIGGERS.lock()
        && triggers.len() < 64
    {
        triggers.push((component, magnitude));
    }
}

/// The single rollback-condition body shared by every predicted component's closure (`net::protocol`
/// — fix 7): trip when `magnitude >= threshold`, and note the trip for trace attribution when it
/// does. Returns whether the component's condition tripped (lightyear's `should_rollback` contract).
#[cfg(feature = "net")]
pub(crate) fn note_if_tripped(component: &'static str, magnitude: f32, threshold: f32) -> bool {
    let trip = magnitude >= threshold;
    if trip {
        note_rollback_trigger(component, magnitude);
    }
    trip
}

/// Clear the trigger slot before each frame's `check_rollback` (registered `.before(Check)` on the
/// client). Lightyear's correction decay (`add_visual_correction`, PostUpdate) reuses the SAME
/// registered rollback conditions to test whether the residual error is still significant, so those
/// re-tests push into the slot every frame a `VisualCorrection` lives — pollution that would
/// misattribute (or, past the 64-cap, evict) the real triggers a later rollback drains. Wiping the
/// slot immediately before `check_rollback` guarantees the observer drains only THIS check's trips.
#[cfg(feature = "net")]
fn clear_rollback_triggers() {
    if let Ok(mut triggers) = ROLLBACK_TRIGGERS.lock() {
        triggers.clear();
    }
}

/// Take and clear the accumulated triggers. Called once per rollback start. With the
/// clear-before-check discipline ([`clear_rollback_triggers`]) the slot holds exactly the trips from
/// this frame's `check_rollback` at drain time — correction-decay re-tests from earlier frames were
/// already wiped — so the drained pairs are exact per-check attribution, not a weight.
#[cfg(feature = "net")]
fn drain_rollback_triggers() -> Vec<(&'static str, f32)> {
    ROLLBACK_TRIGGERS
        .lock()
        .map(|mut triggers| std::mem::take(&mut *triggers))
        .unwrap_or_default()
}

/// Net resources the `frame` row's client extras read, bundled so `record_frame`'s cfg-gated
/// parameter stays a single name. All three exist together on a real MP client (`LocalTimeline` +
/// `PredictionMetrics` from the prediction stack, `ReplicationCheckpointMap` from shared replication
/// registration) — but accessed OPTIONALLY so the SP-composition net build (no lightyear plugins)
/// doesn't panic system-param validation on the missing resources (fix 3).
#[cfg(feature = "net")]
#[derive(SystemParam)]
struct NetFrameCtx<'w> {
    timeline: Option<Res<'w, LocalTimeline>>,
    checkpoints: Option<Res<'w, ReplicationCheckpointMap>>,
    metrics: Option<Res<'w, PredictionMetrics>>,
}

/// Client rollback observer: `Rollback` is added to the `PredictionManager` (client link) entity
/// when a rollback is decided (`net-facts`), so `add.entity` is that entity — read its
/// `PredictionManager` for the start tick and the `Rollback` enum for the cause. Drains the trigger
/// slot the condition closures filled during this same check_rollback.
///
/// Registered only in a traced run (`client_plugin` gates on `install`), so `TraceWriter` is present
/// unconditionally — no Option guard needed.
#[cfg(feature = "net")]
fn record_rollback(
    add: On<Add, Rollback>,
    mut trace: ResMut<TraceWriter>,
    real: Res<Time<Real>>,
    timeline: Res<LocalTimeline>,
    managers: Query<&PredictionManager>,
    rollbacks: Query<&Rollback>,
) {
    let tick = timeline.tick();
    let start = managers
        .get(add.entity)
        .ok()
        .and_then(PredictionManager::get_rollback_start_tick);
    let cause = match rollbacks.get(add.entity) {
        Ok(Rollback::FromState) => "state",
        Ok(Rollback::FromInputs) => "input",
        Err(_) => "unknown",
    };
    let triggers: Vec<Value> = drain_rollback_triggers()
        .into_iter()
        .map(|(component, magnitude)| Value::Array(vec![Value::from(component), num(magnitude)]))
        .collect();
    let row = json!({
        "k": "rollback",
        "t": real.elapsed_secs() as f64,
        "tick": u64::from(tick.0),
        "start": start.map_or(Value::Null, |t| Value::from(u64::from(t.0))),
        "depth": start.map_or(Value::Null, |s| Value::from(i64::from(tick.0) - i64::from(s.0))),
        "cause": cause,
        "trg": Value::Array(triggers),
    });
    trace.write(&row);
}
