//! An opt-in, passive JSONL recorder for render pose and fixed-step state.
//!
//! Invariant: tracing never writes simulation state. `SPIKE_TRACE` enables recorder registration;
//! role-qualified paths prevent concurrently launched compositions from sharing a sink.
//!
//! Rows have `k` values `meta`, `frame`, or `tick`. Fields unavailable in a
//! composition are omitted rather than represented as null. Cross-process analysis joins on `tick`
//! and `role`, never on entity identifiers.
//! [`scripts/divergence/analyze.py`](../../scripts/divergence/analyze.py) consumes the base schema.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use avian3d::collision::contact_types::ContactId;
use avian3d::prelude::{AngularVelocity, Collisions, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use bevy::tasks::ComputeTaskPool;
use serde_json::{Value, json};

use crate::tank::{
    Controlled, RemoteServos, Rig, Tank, TankServos, TankSim, ViewNode, WeaponGate, WeaponLoads,
    rig_world_pose, selected,
};
use crate::track::sim::{
    DriveSignal, TankTransmission, TrackContacts, TrackDrive, TrackGrip, TrackGripElements,
};
use crate::track::transmission::{TransmissionProjectionValue, transmission_state_projection};
use crate::view::PlayerView;

mod state_hash;

pub(crate) use state_hash::TankState;
use state_hash::hash_tank_state;
#[cfg(test)]
pub(crate) use state_hash::{CanonicalTankStateDigest, canonical_tank_state_digest};

use bevy::ecs::system::SystemParam;
use bevy::utils::Parallel;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::{
    Client, ControlledBy, LocalTimeline, PingManager, ReplicationCheckpointMap,
};

/// A recorder's outside-input I/O failure, retaining the operation and output path.
#[derive(Debug)]
pub(crate) enum SinkError {
    Create(PathBuf, std::io::Error),
    Write(PathBuf, std::io::Error),
    Flush(PathBuf, std::io::Error),
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (operation, path, error) = match self {
            Self::Create(path, error) => ("create", path, error),
            Self::Write(path, error) => ("write", path, error),
            Self::Flush(path, error) => ("flush", path, error),
        };
        write!(
            f,
            "recorder: cannot {operation} {}: {error}",
            path.display()
        )
    }
}
impl std::error::Error for SinkError {}

/// Resolution: batch flushes to limit synchronous I/O while retaining a recent crash tail.
const JSONL_FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Shared JSONL sink. Non-finite floats serialize as `null`, preserving valid JSON. An armed
/// recorder must produce its stream: an I/O failure ends the run visibly, never a partial success.
pub(crate) struct JsonlSink {
    path: PathBuf,
    writer: BufWriter<File>,
    last_flush: Instant,
}

impl JsonlSink {
    pub(crate) fn create(path: &Path) -> Result<Self, SinkError> {
        Ok(Self {
            path: path.to_owned(),
            writer: BufWriter::new(
                File::create(path).map_err(|error| SinkError::Create(path.to_owned(), error))?,
            ),
            last_flush: Instant::now(),
        })
    }

    pub(crate) fn write(&mut self, row: &Value) {
        writeln!(self.writer, "{row}")
            .map_err(|error| SinkError::Write(self.path.clone(), error))
            .unwrap_or_else(crate::refuse_start);
        if self.last_flush.elapsed() >= JSONL_FLUSH_INTERVAL {
            self.flush().unwrap_or_else(crate::refuse_start);
            self.last_flush = Instant::now();
        }
    }

    fn flush(&mut self) -> Result<(), SinkError> {
        self.writer
            .flush()
            .map_err(|error| SinkError::Flush(self.path.clone(), error))
    }
}

impl Drop for JsonlSink {
    fn drop(&mut self) {
        if let Err(error) = self.flush() {
            if std::thread::panicking() {
                // The run is already failing; retain the I/O cause without masking its panic.
                eprintln!("{error}");
            } else {
                crate::refuse_start::<()>(error);
            }
        }
    }
}

/// The open trace sink. Present iff `SPIKE_TRACE` was set at startup — [`install`] both inserts it
/// and returns whether it did, so the recorder systems gate on that return value at registration
/// time rather than on a per-frame `resource_exists` check.
#[derive(Resource)]
pub(crate) struct TraceWriter {
    sink: JsonlSink,
    /// Composition role for the Startup `meta` row.
    role: &'static str,
    /// `SPIKE_TRACE_SIM_FIELDS` was set: tick rows also carry `simf`, the raw carried-state values
    /// behind the `hsim` sub-hashes, so the offline analyzer can report magnitudes (Δreload,
    /// Δservo angle) instead of hash booleans. Off by default — it widens
    /// every tick row.
    sim_fields: bool,
}

impl TraceWriter {
    /// Append one row — see [`JsonlSink::write`] for the error/flush discipline.
    fn write(&mut self, row: &Value) {
        self.sink.write(row);
    }
}

// JSON helpers shared across the recorders. Non-finite values become JSON `null`.

pub(crate) fn num(x: f32) -> Value {
    Value::from(x as f64)
}

fn vec3(v: Vec3) -> Value {
    Value::Array(vec![num(v.x), num(v.y), num(v.z)])
}

fn quat(q: Quat) -> Value {
    Value::Array(vec![num(q.x), num(q.y), num(q.z), num(q.w)])
}

/// Link timings are recorded in milliseconds — the unit the interpolation law is stated in.
fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Insert `role` before the extension of the raw `SPIKE_TRACE` value, so concurrently-launched
/// processes sharing one value write to distinct files. `/tmp/t.jsonl` → `/tmp/t.<role>.jsonl`; a
/// value with no extension gets `.<role>.jsonl` appended (`/tmp/t` → `/tmp/t.<role>.jsonl`).
pub(crate) fn role_path(path: &str, role: &str) -> PathBuf {
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

/// Open the role-qualified sink and register the shared meta/flush systems when `SPIKE_TRACE` arms
/// it. Returns `true` iff tracing is armed, so each composition plugin registers its recorders only
/// in a traced run (dropping per-frame `resource_exists` gating). Called once per composition root.
///
/// Users: `SPIKE_TRACE` is set by `scripts/connect/batch.sh`; `SPIKE_TRACE_SIM_FIELDS` by the
/// operator for the jittered two-client combat capture ADR-0029 and ADR-0030 name as their product
/// verification (it reads `simf.wpn` and `simf.srv`); no tracked script sets it.
fn install(app: &mut App, role: &'static str) -> bool {
    let Some(path) = crate::env_value("SPIKE_TRACE").unwrap_or_else(crate::refuse_start) else {
        return false;
    };
    let resolved = role_path(&path, role);
    let sink = JsonlSink::create(&resolved).unwrap_or_else(crate::refuse_start);
    info!("trace: recording {role} rows to {}", resolved.display());
    let sim_fields =
        crate::env_flag("SPIKE_TRACE_SIM_FIELDS", false).unwrap_or_else(crate::refuse_start);
    app.insert_resource(TraceWriter {
        sink,
        role,
        sim_fields,
    });
    // The `meta` row is written from Startup (not here) so `tick_hz` can read the app's actual
    // `Time<Fixed>` timestep, which may not be configured at plugin-build time. Startup runs before
    // any Update/FixedUpdate recorder, so it stays the first row.
    app.add_systems(Startup, write_meta);
    // Flush cadence lives inside `JsonlSink::write` (~1 s, checked per row — rows arrive every
    // frame while tracing), so no periodic flush system is needed.
    true
}

/// Single-player: frame + tick recorders, no net extras (the net resources the extras read are simply
/// absent at runtime in this composition). Two tanks spawn; both are recorded, told apart by `e`/`ctl`
/// in analysis.
pub fn sp_plugin(app: &mut App) {
    if !install(app, "sp") {
        return;
    }
    app.add_systems(PostUpdate, record_frame.after(TransformSystems::Propagate));
    app.add_systems(FixedLast, record_tick);
}

/// MP client: frame and tick rows.
pub fn client_plugin(app: &mut App) {
    if !install(app, "client") {
        return;
    }
    app.add_systems(PostUpdate, record_frame.after(TransformSystems::Propagate));
    app.add_systems(FixedLast, record_tick);
}

/// MP server: tick rows only (it renders nothing, hence no frame rows).
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
    let meta =
        json!({ "k": "meta", "role": role, "tick_hz": tick_hz, "ver": env!("CARGO_PKG_VERSION") });
    trace.write(&meta);
}

/// Per render frame, per tank root: the pose that actually rendered this frame. Ordered
/// `after(TransformSystems::Propagate)` so `GlobalTransform` already reflects the frame-interpolated
/// + correction-adjusted `Position`/`Rotation` (MP) or avian's render interpolation (SP).
///
/// The net extras (`net`, `conf`) read connection state that only a real MP client mounts; every
/// one is accessed OPTIONALLY, so the same system runs unchanged in the single-player composition
/// (where those resources/components are simply absent at runtime) and emits an SP-shaped row.
#[expect(
    clippy::too_many_arguments,
    reason = "a per-frame trace spanning the writer, two clocks, roots, camera, net frame and confirmed history"
)]
fn record_frame(
    mut trace: ResMut<TraceWriter>,
    real: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    roots: Query<(Entity, &GlobalTransform, Has<Controlled>, Option<&Rig>), With<Tank>>,
    bores: DrawnBores,
    // The optional world-camera pose permits camera-space analysis; headless rows omit it.
    camera: Query<&GlobalTransform, With<PlayerView>>,
    net: NetFrameCtx,
    // The root's confirmed-authority history: lightyear seeds these buffers from every replication
    // receive (`add_confirmed_to_history`) and the interpolation cursor walks them. `Option<&…>`
    // per component and an unfiltered query so the single-player composition — which never
    // registers `ConfirmedHistory` — yields `(None, None)` rather than failing system-param
    // validation.
    conf: Query<(
        Option<&ConfirmedHistory<Position>>,
        Option<&ConfirmedHistory<LinearVelocity>>,
    )>,
    (view, pings): (
        Option<Res<crate::net::view_clock::ViewClock>>,
        Query<&PingManager, With<Client>>,
    ),
) {
    // One camera pose for every tank row this frame (recorded after Propagate, so the third-person
    // orbit camera's `GlobalTransform` is final). `None` on a headless client → `cam`/`camq` omitted.
    let cam = camera
        .iter()
        .next()
        .map(GlobalTransform::to_scale_rotation_translation);
    // One connection state for every tank row this frame, the way `tick`/`conf` already repeat.
    // `itick` carries the overstep because headroom (`conft − itick`) is a sub-tick quantity —
    // the whole point of the measurement is how little of it is left.
    let view_tick = view
        .as_ref()
        .and_then(|clock| clock.now())
        .map(|now| now.tick().0);
    let link = view
        .and_then(|clock| clock.now())
        .zip(pings.iter().next())
        .map(|(cursor, pings)| {
            (
                f64::from(cursor.tick().0) + f64::from(cursor.overstep().to_f32()),
                millis(pings.rtt()),
                millis(pings.jitter()),
            )
        });
    for (entity, global, controlled, rig) in &roots {
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
        // The drawn main bore; omitted before a replica's rig attaches.
        if let Some(rig) = rig {
            let (muzzle, direction) = bores.of(rig);
            obj.insert("bore".into(), bore(muzzle, direction));
        }
        // The camera's world pose this frame — same for every tank row. Omitted (not null) when no
        // camera exists (headless client), so the analyzer's camera-space section stays opt-in.
        if let Some((_, cam_rot, cam_tr)) = cam {
            obj.insert("cam".into(), vec3(cam_tr));
            obj.insert("camq".into(), quat(cam_rot));
        }
        {
            if let (Some(timeline), Some(checkpoints)) =
                (net.timeline.as_deref(), net.checkpoints.as_deref())
            {
                obj.insert("local_tick".into(), Value::from(timeline.tick().0));
                obj.insert(
                    "tick".into(),
                    serde_json::to_value(view_tick).expect("optional integer serializes"),
                );
                obj.insert(
                    "conf".into(),
                    checkpoints
                        .last_confirmed_tick()
                        .map_or(Value::Null, |t| Value::from(u64::from(t.0))),
                );
            }
            // RTT and ping jitter are diagnostic only.
            if let Some((itick, rtt, jitter)) = link {
                obj.insert("itick".into(), Value::from(itick));
                obj.insert("rtt".into(), Value::from(rtt));
                obj.insert("jit".into(), Value::from(jitter));
            }
            // The LATEST confirmed (server-authoritative) Position/LinearVelocity this root has
            // received, plus the tick it belongs to. `ConfirmedHistory::newest_present` is the
            // buffer's most-recent present sample — `(tick, value)`. `conft` is the entity's OWN
            // last authoritative tick; when it stalls while the global `conf` keeps advancing, the
            // tank's confirmed updates stopped arriving (silent-desync branch a). Omit all three
            // when the buffer is still empty — a not-yet-confirmed frame carries no `conf*`.
            // `hlen` is that buffer's occupancy: the keyframes the interpolation cursor has left to
            // walk, recorded whenever the buffer exists (zero included — that IS the starved case).
            if let Ok((confp, confv)) = conf.get(entity) {
                if let Some(history) = confp {
                    obj.insert("hlen".into(), Value::from(history.len() as u64));
                }
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

/// Reusable pair work and per-thread results; no contact references survive the fixed tick.
#[derive(Default)]
struct TickContacts {
    pairs: Vec<ContactId>,
    summaries: Parallel<Vec<([Option<Entity>; 2], f32)>>,
    by_body: std::collections::HashMap<Entity, (u32, f32)>,
}

impl TickContacts {
    fn update(&mut self, collisions: &Collisions) {
        self.pairs.clear();
        self.pairs
            .extend(collisions.iter().map(|pair| pair.contact_id));
        let pool = ComputeTaskPool::get();
        let chunk_size = self.pairs.len().div_ceil(pool.thread_num()).max(1);
        let summaries = &self.summaries;
        pool.scope(|scope| {
            for ids in self.pairs.chunks(chunk_size) {
                scope.spawn(async move {
                    summaries.scope(|rows| {
                        for &id in ids {
                            let (_, pair) = collisions
                                .graph()
                                .get_by_id(id)
                                .expect("pair IDs came from this immutable contact graph");
                            // Only touching, overlapping pairs contribute; negative separations
                            // clamp to zero penetration rather than retaining stale AABB contacts.
                            if pair.is_touching() && !pair.aabbs_disjoint() {
                                let deepest = pair
                                    .find_deepest_contact()
                                    .map_or(0.0, |point| point.penetration)
                                    .max(0.0);
                                rows.push(([pair.body1, pair.body2], deepest));
                            }
                        }
                    });
                });
            }
        });
        self.by_body.clear();
        // Counts and maxima commute; reduce shared body incidence after pair evaluation.
        for (bodies, deepest) in self.summaries.iter_mut().flat_map(|rows| rows.drain(..)) {
            for body in bodies.into_iter().flatten() {
                let entry = self.by_body.entry(body).or_insert((0, 0.0));
                entry.0 += 1;
                entry.1 = entry.1.max(deepest);
            }
        }
    }
}

/// Per fixed gameplay tick, per tank root: current pose and velocities plus contact state
/// (grounded track sides, per-side belt loads, collision pairs). Runs in `FixedLast`, after
/// physics and contact updates. Client poses are presented server state, never predictions.
///
/// Client `tick` is the server-domain view tick (null before a stamp pair); `local_tick`
/// separately counts fixed gameplay. Servers use authority ticks; offline runs count locally.
fn record_tick(
    mut trace: ResMut<TraceWriter>,
    roots: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            (&TrackDrive, Option<&DriveSignal>),
            &TrackGrip,
            Option<&TrackGripElements>,
            Option<&TankTransmission>,
            (Option<&WeaponGate>, Option<&WeaponLoads>),
            Option<&TankServos>,
            Option<&RemoteServos>,
            &TrackContacts,
            &TankSim,
            (Has<Controlled>, Option<&Rig>),
        ),
        With<Tank>,
    >,
    // The rig chain for each row's skeleton bore (`rig_world_pose`).
    rig_chain: (Query<&ChildOf>, Query<&Transform>),
    (collisions, mut contacts): (Collisions, Local<TickContacts>),
    // Each tank's row is prepared on the compute pool; the writer commits them in entity order.
    (mut tick_counter, mut rows): (Local<u64>, Local<Parallel<Vec<(Entity, Value)>>>),
    // The local fixed clock and the view clock a client row is stamped from.
    (timeline, view): (
        Option<Res<LocalTimeline>>,
        Option<Res<crate::net::view_clock::ViewClock>>,
    ),
    // `own` joins roles: ControlledBy on authority player tanks, Controlled on clients/offline.
    owners: Query<(), With<ControlledBy>>,
) {
    let (role, sim_fields) = (trace.role, trace.sim_fields);
    let local_tick = timeline.as_deref().map_or_else(
        || next_local_tick(&mut tick_counter),
        |timeline| u64::from(timeline.tick().0),
    );
    let tick_no = match role {
        "client" => view
            .as_ref()
            .expect("the client trace composition mounts ViewClock")
            .now()
            .map(|now| u64::from(now.tick().0)),
        _ => Some(local_tick),
    };

    contacts.update(&collisions);

    roots.par_iter().for_each(
        |(
            entity,
            position,
            rotation,
            linvel,
            angvel,
            (drive, signal),
            grip,
            elements,
            transmission,
            (weapon_gate, weapon_loads),
            servos,
            remote_servos,
            track_contacts,
            sim,
            (controlled, rig),
        )| {
            // `gnd` counts contacting SIDES (0–2), `loads` is per-side ELASTIC load sums (the stable
            // baseline channel — the damped actual load, which scales grip, rides as `loads_act`),
            // `thr`/`str` are the shaped command, `belt`/`bph` the per-side belt speed and phase.
            let grounded = track_contacts
                .0
                .iter()
                .filter(|side| !side.is_empty())
                .count();
            // Resolution: 0.1 N omits solver-noise digits from the diagnostic row.
            let side_sum = |f: fn(&crate::track::forces::BeltContact) -> f32| -> Vec<Value> {
                track_contacts
                    .0
                    .iter()
                    .map(|side| {
                        let sum: f32 = side.iter().map(f).sum();
                        num((sum * 10.0).round() / 10.0)
                    })
                    .collect()
            };
            let loads = side_sum(|c| c.load_elastic);
            let loads_act = side_sum(|c| c.load);

            // Contact pairs whose rigid body is this tank root, and the deepest penetration among them
            // — the collision-stress signal the jitter correlates with.
            let (hull_contacts, penetration) =
                contacts.by_body.get(&entity).copied().unwrap_or((0, 0.0));

            let own = if role == "server" {
                owners.get(entity).is_ok()
            } else {
                controlled
            };
            // Every tank a client traces is a replica on a static body (`net::rig`). Angular velocity
            // is not on the wire, so that body holds Avian's zero, not the authority's spin: null.
            let replica = role == "client";
            let angular = if replica { Value::Null } else { vec3(angvel.0) };

            // The world-independent authority-simulation hash. It feeds off tick-truth pose/velocity
            // and carried state already in hand, except the view-only tracer phase documented on
            // `TankStateHash`; costs a few dozen FNV rounds and runs only when tracing is armed.
            // The servos are the root's one integrator: `RemoteServos` on a client replica, the
            // authoritative `TankServos` on the server and single-player.
            let servo_states = selected(
                remote_servos.map(|servos| servos.0.as_slice()),
                servos.map(|servos| servos.states.as_slice()),
            );
            let hash = hash_tank_state(&TankState {
                position: position.0,
                rotation: rotation.0,
                linvel: linvel.0,
                angvel: angvel.0,
                drive,
                signal,
                grip,
                elements,
                transmission,
                weapon_gate,
                loads: weapon_loads,
                servos: servo_states,
                sim,
            });
            // Remote clients intentionally receive no exact element field. Null makes that disclosure
            // boundary explicit so the analyzer omits private-state and combined equality for the pair.
            let disclosed_element_hash = own.then_some(hash.elm);

            let mut row = json!({
                "k": "tick",
                "tick": tick_no,
                "local_tick": local_tick,
                "e": entity.to_bits(),
                "p": vec3(position.0),
                "q": quat(rotation.0),
                "lv": vec3(linvel.0),
                "av": angular,
                "gnd": grounded,
                "loads": Value::Array(loads),
                "loads_act": Value::Array(loads_act),
                // Null on a replica of a tank this client does not own: owner-only on the wire.
                "thr": signal.map_or(Value::Null, |signal| num(signal.throttle)),
                "str": signal.map_or(Value::Null, |signal| num(signal.steer)),
                "belt": [num(drive.sides[0].speed), num(drive.sides[1].speed)],
                // Full f64 — the phase IS f64 sim state; narrowing here would hide sub-f32-ULP
                // divergence from the offline join.
                "bph": [Value::from(drive.sides[0].phase), Value::from(drive.sides[1].phase)],
                "hc": hull_contacts,
                "pen": num(penetration),
                "ctl": controlled,
                "own": own,
                // The per-tick authority-simulation hash: `h` combined, then the per-component
                // breakdown so a difference localizes to pose vs velocity vs carried sim.
                "h": hash.combined,
                "hpos": hash.pos,
                "hrot": hash.rot,
                "hlv": hash.lv,
                "hav": (!replica).then_some(hash.av),
                "hsim": hash.sim,
                // The carried-state decode: which field family a `hsim` mismatch lives in. These
                // streams are exactly what `hsim` folds.
                "hdrv": hash.drv,
                "hsrv": hash.srv,
                "hrld": hash.rld,
                "hrec": hash.rec,
                "hblt": hash.blt,
                "htrn": hash.trn,
                "helm": disclosed_element_hash,
            });
            // The main bore of this tick's skeleton, composed as `shooting::fire` composes it: the
            // authority's on the server; on a client, the presentation state its fixed step copied
            // into the sim nodes (`restore_servo_truth`), not authority truth. Omitted before a
            // replica's rig attaches, and once a launched assembly has taken the muzzle off the root.
            let (parents, locals) = &rig_chain;
            if let Some((muzzle, turn)) = rig.and_then(|rig| {
                rig_world_pose(rig.muzzle, entity, position.0, rotation.0, parents, locals)
            }) {
                row.as_object_mut()
                    .expect("json! built an object")
                    .insert("bore".into(), bore(muzzle, turn * Vec3::NEG_Z));
            }
            // Raw carried-state values (`SPIKE_TRACE_SIM_FIELDS`): the magnitudes behind the sub-hash
            // booleans. `thr`/`str` (TrackDrive) are already row fields above.
            if sim_fields {
                // Fixed field order per deterministic slot: current, previous, angular velocity.
                let srv: Vec<Value> = servo_states
                    .iter()
                    .map(|s| Value::Array(s.hash_fields().iter().map(|&f| num(f)).collect()))
                    .collect();
                let wpn: Vec<Value> = sim
                    .weapons
                    .iter()
                    .enumerate()
                    .map(|(slot, w)| {
                        let gate = weapon_gate.and_then(|gate| gate.weapons.get(slot));
                        Value::Array(vec![
                            gate.and_then(|gate| gate.ready_tick())
                                .map_or(Value::Null, |tick| Value::from(tick.raw())),
                            gate.and_then(|gate| gate.paused_at_tick())
                                .map_or(Value::Null, |tick| Value::from(tick.raw())),
                            num(w.recoil_offset),
                            num(w.recoil_velocity),
                            gate.map_or(Value::Null, |gate| Value::from(gate.belt_remaining)),
                        ])
                    })
                    .collect();
                // Canonical transmission order; null on non-owned replicas (owner-only wire state).
                let trn = transmission.map_or(Value::Null, |transmission| {
                    let trn = transmission_state_projection(&transmission.0)
                        .into_iter()
                        .map(|field| match field {
                            TransmissionProjectionValue::U8(value) => Value::from(value),
                            TransmissionProjectionValue::Bool(value) => Value::from(value),
                            TransmissionProjectionValue::F32(value) => num(value),
                        })
                        .collect();
                    Value::Array(trn)
                });
                row.as_object_mut()
                    .expect("json! built an object")
                    .insert("simf".into(), json!({"srv": srv, "wpn": wpn, "trn": trn}));
            }
            rows.scope(|rows| rows.push((entity, row)));
        },
    );
    let mut prepared: Vec<_> = rows.iter_mut().flat_map(|rows| rows.drain(..)).collect();
    prepared.sort_unstable_by_key(|&(entity, _)| entity);
    for (_, row) in prepared {
        trace.write(&row);
    }
}

/// Read-and-bump the single-player tick counter: there is no network tick, so this monotonic count
/// orders single-player rows.
fn next_local_tick(counter: &mut Local<u64>) -> u64 {
    let current = **counter;
    **counter += 1;
    current
}

/// Optional net resources used by frame rows. Optional access keeps the shared system valid in
/// single-player.
#[derive(SystemParam)]
struct NetFrameCtx<'w> {
    timeline: Option<Res<'w, LocalTimeline>>,
    checkpoints: Option<Res<'w, ReplicationCheckpointMap>>,
}

/// A rig's main bore as the frame draws it: its muzzle's view node, or the sim node before the
/// view binds (`ViewNode::resolve`). Read after transform propagation.
#[derive(SystemParam)]
pub(crate) struct DrawnBores<'w, 's> {
    views: Query<'w, 's, Option<&'static ViewNode>>,
    globals: Query<'w, 's, &'static GlobalTransform>,
}

impl DrawnBores<'_, '_> {
    /// The drawn muzzle's world position and bore direction (its −Z, as `shooting::fire` fires).
    pub(crate) fn of(&self, rig: &Rig) -> (Vec3, Vec3) {
        let view = ViewNode::resolve(
            self.views
                .get(rig.muzzle)
                .expect("a rig's muzzle is a live entity"),
            rig.muzzle,
        );
        let drawn = self
            .globals
            .get(view)
            .expect("a drawn muzzle carries its propagated transform");
        (drawn.translation(), drawn.rotation() * Vec3::NEG_Z)
    }
}

/// `[x, y, z, dx, dy, dz]`: a muzzle position and its bore direction, one row field.
pub(crate) fn bore(muzzle: Vec3, direction: Vec3) -> Value {
    Value::Array(
        [muzzle.to_array(), direction.to_array()]
            .into_iter()
            .flatten()
            .map(num)
            .collect(),
    )
}

#[cfg(test)]
mod sink_tests {
    use super::*;

    /// An armed recorder cannot silently become an unrecorded run. Its edge retains a typed
    /// creation failure and its actual path, so every composition can refuse visibly.
    #[test]
    fn an_unwritable_sink_names_its_path_and_operation() {
        let path = std::env::temp_dir();
        let error = match JsonlSink::create(&path) {
            Ok(_) => panic!("a directory cannot be a JSONL file"),
            Err(error) => error,
        };
        assert!(matches!(error, SinkError::Create(_, _)));
        assert!(error.to_string().contains(&path.display().to_string()));
    }
}
