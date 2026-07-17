//! An opt-in, passive JSONL recorder for render pose, fixed-step state, and rollback events.
//!
//! Invariant: tracing never writes simulation state. `SPIKE_TRACE` enables recorder registration;
//! role-qualified paths prevent concurrently launched compositions from sharing a sink.
//!
//! Rows have `k` values `meta`, `frame`, `tick`, or `rollback`. Fields unavailable in a composition
//! are omitted rather than represented as null. Cross-process analysis joins on `tick` and `role`,
//! never on entity identifiers. [`scripts/divergence/analyze.py`](../../scripts/divergence/analyze.py)
//! consumes the schema.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use avian3d::prelude::{AngularVelocity, Collisions, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use serde_json::{Value, json};

use crate::tank::{Controlled, Tank, TankSim};
use crate::track::sim::{TrackContacts, TrackDrive};

use bevy::ecs::system::SystemParam;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::{
    ControlledBy, LocalTimeline, PredictionManager, PredictionMetrics, ReplicationCheckpointMap,
    Rollback, RollbackSystems, VisualCorrection,
};

/// Shared JSONL sink. Values pass through `serde_json::Value` so non-finite floats serialize as
/// `null`, preserving valid JSON. Writes are best-effort: diagnostics must not perturb simulation.
pub(crate) struct JsonlSink {
    writer: BufWriter<File>,
    last_flush: Instant,
}

impl JsonlSink {
    pub(crate) fn create(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(File::create(path)?),
            last_flush: Instant::now(),
        })
    }

    /// Append one row, best-effort. A passive observer never lets an I/O hiccup disturb the sim, so
    /// write errors are dropped (the periodic flush + the parse check surface a broken file).
    pub(crate) fn write(&mut self, row: &Value) {
        let _ = writeln!(self.writer, "{row}");
        if self.last_flush.elapsed() >= Duration::from_secs(1) {
            let _ = self.writer.flush();
            self.last_flush = Instant::now();
        }
    }
}

/// The open trace sink. Present iff `SPIKE_TRACE` was set at startup — [`install`] both inserts it
/// and returns whether it did, so the recorder systems gate on that return value at registration
/// time rather than on a per-frame `resource_exists` check.
#[derive(Resource)]
struct TraceWriter {
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

// Per-tick state hashes use a fixed field/slot order and raw float bits. Entity IDs and unordered
// collections must not enter the hash: client and server use different ECS identities.

/// A tiny FNV-1a 64-bit hasher over an explicit byte stream. Chosen over `std::hash::DefaultHasher`
/// deliberately: its algorithm is fixed here (not a std-version-dependent SipHash seed), so a hash is
/// reproducible across builds and trivially re-derivable by an offline tool, and it is fed only the
/// f32 bits we hand it — the world-independence guarantee lives in WHAT we write, and this type keeps
/// the HOW dependency-free and testable.
struct Fnv64(u64);

impl Fnv64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn write_u32(&mut self, x: u32) {
        for b in x.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn write_u64(&mut self, x: u64) {
        for b in x.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    /// Hash the f32's RAW BITS — bit-exactness is the divergence bar, so `1.0` and the next
    /// representable value must hash apart, and `+0.0`/`−0.0` must not collide.
    fn write_f32(&mut self, x: f32) {
        self.write_u32(x.to_bits());
    }

    fn write_vec3(&mut self, v: Vec3) {
        self.write_f32(v.x);
        self.write_f32(v.y);
        self.write_f32(v.z);
    }

    fn write_quat(&mut self, q: Quat) {
        self.write_f32(q.x);
        self.write_f32(q.y);
        self.write_f32(q.z);
        self.write_f32(q.w);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// One tank's per-tick state hash plus per-component breakdown. `sim` includes authority-relevant
/// carried state that pose and velocity fields do not expose. `WeaponState::rounds_fired` is
/// deliberately absent: it selects a local tracer phase and can legitimately lag the authority by
/// one predicted round, so feeding it into a cross-world divergence rate would make a benign view
/// skew look like simulation drift. The fresh-App test has a stricter, rollback-complete digest.
struct TankStateHash {
    combined: u64,
    pos: u64,
    rot: u64,
    lv: u64,
    av: u64,
    /// The carried-state combination (fixed order: `drv, srv, rld, rec, blt`) — kept so existing
    /// analysis keyed on `hsim` still gets its single "did any carried state differ?" boolean.
    sim: u64,
    /// `TrackDrive` shaped throttle/steer.
    drv: u64,
    /// Servo current/previous/velocity, every servo in slot order.
    srv: u64,
    /// Weapon reload timers, every weapon in slot order.
    rld: u64,
    /// Barrel recoil offset/velocity, every weapon in slot order.
    rec: u64,
    /// Per-side belt state: speed + phase.
    blt: u64,
}

/// Hash a tank root's canonical sim state (see the module-level note on world-independence). Pure and
/// ECS-free precisely so it is unit-testable: same inputs → same hash, one flipped velocity bit → a
/// different hash, and — because no entity ever enters it — hash equality is independent of the two
/// worlds' entity ids. Field order is fixed and load-bearing: `position, rotation, linvel, angvel`,
/// then `TrackDrive` (shaped command + per-side belt state), then each `TankSim` `Vec` in slot
/// order.
fn hash_tank_state(
    position: Vec3,
    rotation: Quat,
    linvel: Vec3,
    angvel: Vec3,
    drive: &TrackDrive,
    sim: &TankSim,
) -> TankStateHash {
    let mut hp = Fnv64::new();
    hp.write_vec3(position);
    let pos = hp.finish();

    let mut hr = Fnv64::new();
    hr.write_quat(rotation);
    let rot = hr.finish();

    let mut hl = Fnv64::new();
    hl.write_vec3(linvel);
    let lv = hl.finish();

    let mut ha = Fnv64::new();
    ha.write_vec3(angvel);
    let av = ha.finish();

    // The carried state hashes as five per-field-family streams so a `hsim` mismatch names its
    // field (servo vs reload vs recoil vs belt vs drive), then combines into the single `sim`
    // boolean existing analysis keys on.
    let mut hd = Fnv64::new();
    hd.write_f32(drive.throttle);
    hd.write_f32(drive.steer);
    let drv = hd.finish();

    let mut hsv = Fnv64::new();
    for servo in &sim.servos {
        for field in servo.hash_fields() {
            hsv.write_f32(field);
        }
    }
    let srv = hsv.finish();

    let mut hrl = Fnv64::new();
    let mut hrc = Fnv64::new();
    for weapon in &sim.weapons {
        hrl.write_f32(weapon.reload_remaining);
        // `belt_remaining` GATES fire (a dry belt cannot shoot; the swap timer's meaning depends
        // on it), so it enters the hash — in the reload stream, whose fire-timer it modulates.
        // Contrast `rounds_fired`, which is deliberately EXCLUDED: that counter only picks which
        // rounds trace, a cosmetic phase that a dropped predicted shot legitimately skews by one
        // (see `WeaponState::rounds_fired`) — hashing it would flag benign skew as divergence.
        hrl.write_u32(weapon.belt_remaining);
        hrc.write_f32(weapon.recoil_offset);
        hrc.write_f32(weapon.recoil_velocity);
    }
    let rld = hrl.finish();
    let rec = hrc.finish();

    let mut hbl = Fnv64::new();
    for side in &drive.sides {
        hbl.write_f32(side.speed);
        // Phase is f64 sim state; both halves enter so no precision is silently dropped.
        hbl.write_u64(side.phase.to_bits());
    }
    let blt = hbl.finish();

    let mut hs = Fnv64::new();
    for sub in [drv, srv, rld, rec, blt] {
        hs.write_u64(sub);
    }
    let sim_hash = hs.finish();

    // Combine the sub-hashes in fixed order so `combined` reflects EVERY field and no sub-component's
    // difference can cancel another's.
    let mut hc = Fnv64::new();
    for sub in [pos, rot, lv, av, sim_hash] {
        hc.write_u64(sub);
    }
    TankStateHash {
        combined: hc.finish(),
        pos,
        rot,
        lv,
        av,
        sim: sim_hash,
        drv,
        srv,
        rld,
        rec,
        blt,
    }
}

/// In-memory fresh-App digest. `simulation` is exactly the production trace's cross-world hash;
/// `rollback` additionally folds every `WeaponState::rounds_fired` value. The latter is rollback
/// state but deliberately excluded from the production trace (see [`TankStateHash`]), whose job is
/// to compare authority-relevant simulation across a predicted client and server.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CanonicalTankStateDigest {
    simulation: u64,
    rollback: u64,
    position: u64,
    rotation: u64,
    linear_velocity: u64,
    angular_velocity: u64,
    drive: u64,
    servo: u64,
    reload: u64,
    recoil: u64,
    belts: u64,
    rounds_fired: u64,
}

#[cfg(test)]
pub(crate) fn canonical_tank_state_digest(
    position: Vec3,
    rotation: Quat,
    linvel: Vec3,
    angvel: Vec3,
    drive: &TrackDrive,
    sim: &TankSim,
) -> CanonicalTankStateDigest {
    let hash = hash_tank_state(position, rotation, linvel, angvel, drive, sim);
    let mut phase = Fnv64::new();
    for weapon in &sim.weapons {
        phase.write_u32(weapon.rounds_fired);
    }
    let rounds_fired = phase.finish();
    let mut rollback = Fnv64::new();
    rollback.write_u64(hash.combined);
    rollback.write_u64(rounds_fired);
    CanonicalTankStateDigest {
        simulation: hash.combined,
        rollback: rollback.finish(),
        position: hash.pos,
        rotation: hash.rot,
        linear_velocity: hash.lv,
        angular_velocity: hash.av,
        drive: hash.drv,
        servo: hash.srv,
        reload: hash.rld,
        recoil: hash.rec,
        belts: hash.blt,
        rounds_fired,
    }
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

/// Open the role-qualified sink and register the shared meta/flush systems — only when `SPIKE_TRACE`
/// is set. Returns `true` iff tracing is armed, so each composition plugin registers its recorders
/// only in a traced run (dropping per-frame `resource_exists` gating). Called once per composition
/// root.
fn install(app: &mut App, role: &'static str) -> bool {
    let Ok(path) = std::env::var("SPIKE_TRACE") else {
        return false;
    };
    let resolved = role_path(&path, role);
    let sink = match JsonlSink::create(&resolved) {
        Ok(sink) => sink,
        Err(err) => {
            error!("trace: cannot open {}: {err}", resolved.display());
            return false;
        }
    };
    info!("trace: recording {role} rows to {}", resolved.display());
    let sim_fields = std::env::var("SPIKE_TRACE_SIM_FIELDS").is_ok();
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
    // Arm the trigger-attribution slot's fast path. The slot only fills on the client (check_rollback
    // is client-only), but the flag is role-agnostic and cheap; the server never calls
    // `note_rollback_trigger`, so its slot stays empty regardless — as does the single-player
    // composition, which registers no rollback conditions.
    TRACE_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
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

/// MP client: frame and tick rows plus rollback observation. Replay rows carry `rp`; clearing the
/// trigger slot before Lightyear's check scopes `trg` attribution to that check.
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
/// The net extras (`net`, `corr`, `conf`, `view_offset`) read prediction/correction state that only
/// a real MP client mounts; every one is accessed OPTIONALLY, so the same system runs unchanged in
/// the single-player composition (where those resources/components are simply absent at runtime) and
/// emits an SP-shaped row.
fn record_frame(
    mut trace: ResMut<TraceWriter>,
    real: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    roots: Query<(Entity, &GlobalTransform, Has<Controlled>), With<Tank>>,
    // The optional world-camera pose permits camera-space analysis; headless rows omit it.
    camera: Query<&GlobalTransform, With<Camera3d>>,
    net: NetFrameCtx,
    corr: Query<(
        Option<&VisualCorrection<Position>>,
        Option<&VisualCorrection<Rotation>>,
    )>,
    // The predicted root's confirmed-authority history: lightyear seeds these buffers from every
    // replication receive (`add_confirmed_to_history`) and `prepare_rollback` reads them as the
    // rollback source. `Option<&…>` per component and an unfiltered query (like `corr`) so the
    // single-player composition — which never registers `ConfirmedHistory` — yields `(None, None)`
    // rather than failing system-param validation.
    conf: Query<(
        Option<&ConfirmedHistory<Position>>,
        Option<&ConfirmedHistory<LinearVelocity>>,
    )>,
    // The predicted root's live render-space error offset. Present only on the entity `net::render_error`
    // armed (the client's predicted tank); an unfiltered `Option`-style `get` keeps every other tank
    // row — and the single-player composition, which never mounts the layer — omitting the field.
    view_offset: Query<&crate::net::RenderErrorOffset>,
) {
    // One camera pose for every tank row this frame (recorded after Propagate, so the third-person
    // orbit camera's `GlobalTransform` is final). `None` on a headless client → `cam`/`camq` omitted.
    let cam = camera
        .iter()
        .next()
        .map(GlobalTransform::to_scale_rotation_translation);
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
        // The camera's world pose this frame — same for every tank row. Omitted (not null) when no
        // camera exists (headless client), so the analyzer's camera-space section stays opt-in.
        if let Some((_, cam_rot, cam_tr)) = cam {
            obj.insert("cam".into(), vec3(cam_tr));
            obj.insert("camq".into(), quat(cam_rot));
        }
        {
            // Net resources are optional so this shared system remains valid in single-player.
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
            // The render-space error offset this frame folds into the rendered `p`/`q`. Present only
            // on the predicted root carrying `RenderErrorOffset`; omitted (not null) on every other row.
            if let Ok(offset) = view_offset.get(entity) {
                obj.insert("vo".into(), vec3(offset.translation));
                obj.insert("voq".into(), quat(offset.rotation));
            }
        }
        trace.write(&row);
    }
}

/// Per fixed tick, per tank root: sim truth (`Position`/`Rotation`/velocities are the rolled-back,
/// replayed authority values) plus the derived contact state (grounded track sides, per-side belt
/// loads, collision pairs). Runs in `FixedLast`, after the physics step and avian's contact update,
/// so `Collisions` is current for this tick.
///
/// Replay ticks carry `rp`; network compositions use `LocalTimeline`, while others use a local
/// monotonic counter.
fn record_tick(
    mut trace: ResMut<TraceWriter>,
    roots: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &TrackDrive,
            &TrackContacts,
            &TankSim,
            Has<Controlled>,
        ),
        With<Tank>,
    >,
    collisions: Collisions,
    mut tick_counter: Local<u64>,
    timeline: Option<Res<LocalTimeline>>,
    // Same source `is_in_rollback` reads (`Query<(), With<Rollback>>`): non-empty iff this tick is a
    // rollback-replay re-simulation, which is what `rp` marks. Empty in the single-player composition
    // (no rollback), which is correct.
    replaying: Query<(), With<Rollback>>,
    // The server's ownership marker (`spawn_player_tank` inserts `ControlledBy` on every player
    // tank; the ownerless test bot has none). It is the SERVER-side half of the cross-world identity
    // the hash join pairs on: the client's own predicted tank carries the game `Controlled` marker
    // (already in the roots query), the server's authoritative copy of that same logical tank carries
    // `ControlledBy` instead — neither replicates the other's marker, so `own` is computed per role
    // below. Empty result on a client / single-player, which is correct (they pair on `Controlled`).
    owners: Query<(), With<ControlledBy>>,
) {
    // The composition role, copied before the mutable `trace` borrow the write loop needs. It selects
    // which marker means "the player's own tank" for the world-independent pairing key `own`.
    let role = trace.role;
    // One tick number for every tank this call. MP: the predicted/authoritative tick, so client and
    // server rows align. Single-player (no `LocalTimeline`): a monotonic local counter, incremented
    // once per tick here — enough for the analysis script to order rows.
    let tick_no = match timeline.as_deref() {
        Some(timeline) => u64::from(timeline.tick().0),
        None => next_local_tick(&mut tick_counter),
    };

    let is_replay = !replaying.is_empty();

    // `hc`/`pen` include only touching, overlapping Avian pairs. A non-overlapping AABB makes a
    // contact record stale for this diagnostic; negative separations clamp to zero penetration.
    let mut contacts: std::collections::HashMap<Entity, (u32, f32)> =
        std::collections::HashMap::new();
    for pair in collisions.iter() {
        if !pair.is_touching() || pair.aabbs_disjoint() {
            continue;
        }
        let deepest = pair
            .find_deepest_contact()
            .map_or(0.0, |point| point.penetration)
            .max(0.0);
        for body in [pair.body1, pair.body2].into_iter().flatten() {
            let entry = contacts.entry(body).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 = entry.1.max(deepest);
        }
    }

    for (entity, position, rotation, linvel, angvel, drive, track_contacts, sim, controlled) in
        &roots
    {
        // Trace schema v2 (phase B): the per-wheel suspension topology is gone. `gnd` counts
        // contacting SIDES (0–2), `loads` is per-side elastic load sums, `thr`/`str` are the
        // shaped command, and `belt`/`bph` are the per-side belt speed and phase.
        let grounded = track_contacts
            .0
            .iter()
            .filter(|side| !side.is_empty())
            .count();
        let loads: Vec<Value> = track_contacts
            .0
            .iter()
            // Round to ~0.1 N: solver-noise digits past that are neither meaningful nor worth
            // the row width.
            .map(|side| {
                let sum: f32 = side.iter().map(|c| c.load).sum();
                num((sum * 10.0).round() / 10.0)
            })
            .collect();

        // Contact pairs whose rigid body is this tank root, and the deepest penetration among them
        // — the collision-stress signal the jitter correlates with.
        let (hull_contacts, penetration) = contacts.get(&entity).copied().unwrap_or((0, 0.0));

        // The world-independent authority-simulation hash. It feeds off tick-truth pose/velocity
        // and carried state already in hand, except the view-only tracer phase documented on
        // `TankStateHash`; costs a few dozen FNV rounds and runs only when tracing is armed.
        let hash = hash_tank_state(position.0, rotation.0, linvel.0, angvel.0, drive, sim);

        // The cross-world pairing key. Client / SP: the game `Controlled` marker (own predicted tank).
        // Server: `ControlledBy` (the authoritative copy of that same player tank). Kept per-role so a
        // 2-player world stays correct — on the client only OUR avatar carries `Controlled`, while the
        // server marks every player tank `ControlledBy` its own link. Single-player has no server
        // role, so `own` falls through to `Controlled` there too.
        let own = if role == "server" {
            owners.get(entity).is_ok()
        } else {
            controlled
        };

        // `mut` for the `rp` stamp and the `simf` verbose dump below.
        let mut row = json!({
            "k": "tick",
            "tick": tick_no,
            "e": entity.to_bits(),
            "p": vec3(position.0),
            "q": quat(rotation.0),
            "lv": vec3(linvel.0),
            "av": vec3(angvel.0),
            "gnd": grounded,
            "loads": Value::Array(loads),
            "thr": num(drive.throttle),
            "str": num(drive.steer),
            "belt": [num(drive.sides[0].speed), num(drive.sides[1].speed)],
            // Full f64 — the phase IS f64 sim state; narrowing here would hide sub-f32-ULP
            // divergence from the offline join.
            "bph": [Value::from(drive.sides[0].phase), Value::from(drive.sides[1].phase)],
            "hc": hull_contacts,
            "pen": num(penetration),
            "ctl": controlled,
            // Cross-world tank identity for the hash join (world-independent — never the entity id).
            "own": own,
            // The per-tick authority-simulation hash: `h` combined, then the per-component
            // breakdown so a difference localizes to pose vs velocity vs carried sim.
            "h": hash.combined,
            "hpos": hash.pos,
            "hrot": hash.rot,
            "hlv": hash.lv,
            "hav": hash.av,
            "hsim": hash.sim,
            // The carried-state decode: which field family a `hsim` mismatch lives in.
            "hdrv": hash.drv,
            "hsrv": hash.srv,
            "hrld": hash.rld,
            "hrec": hash.rec,
            "hblt": hash.blt,
        });
        // Raw carried-state values (`SPIKE_TRACE_SIM_FIELDS`): the magnitudes behind the sub-hash
        // booleans. `thr`/`str` (TrackDrive) are already row fields above.
        if trace.sim_fields {
            let srv: Vec<Value> = sim
                .servos
                .iter()
                .map(|s| Value::Array(s.hash_fields().iter().map(|&f| num(f)).collect()))
                .collect();
            let wpn: Vec<Value> = sim
                .weapons
                .iter()
                .map(|w| {
                    Value::Array(vec![
                        num(w.reload_remaining),
                        num(w.recoil_offset),
                        num(w.recoil_velocity),
                        Value::from(w.belt_remaining),
                    ])
                })
                .collect();
            row.as_object_mut()
                .expect("json! built an object")
                .insert("simf".into(), json!({"srv": srv, "wpn": wpn}));
        }
        // Mark rollback-replay ticks so analysis keeps the corrected value for this tick number.
        // Never set in the single-player composition (`is_replay` is always false there — no rollback).
        if is_replay {
            row.as_object_mut()
                .expect("json! built an object")
                .insert("rp".into(), Value::Bool(true));
        }
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

// --- Rollback trigger attribution --------------------------------------------------------------
// Reached only on a real MP client (the server never runs check_rollback; single-player registers no
// rollback conditions), but always compiled — the guard below makes it free when tracing is off.

/// Set once the trace writer opens, so the rollback-condition closures in `net::protocol` can skip
/// the mutex entirely when tracing is off — the cost of instrumentation is a single relaxed atomic
/// load on the check_rollback hot path.
static TRACE_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The trigger-attribution slot: component/magnitude pairs pushed by the rollback-condition closures
/// (`net::protocol`) as they trip, drained by [`record_rollback`] into the `trg` field. A plain
/// `static Mutex` rather than a resource because the closures are `Fn` values with no `World` access.
/// Capped at 64 to bound a pathological burst; excess is dropped.
static ROLLBACK_TRIGGERS: std::sync::Mutex<Vec<(&'static str, f32)>> =
    std::sync::Mutex::new(Vec::new());

/// Record that `component`'s rollback condition tripped this check, by `magnitude`. Called from
/// [`note_if_tripped`] when a condition returns true. No-op
/// when tracing is off (the atomic guard) — so the closures pay nothing in an untraced run, and the
/// server (which never runs check_rollback) never reaches the push.
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

/// Shared rollback condition: trip when `magnitude >= threshold` and record the attribution.
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
fn clear_rollback_triggers() {
    if let Ok(mut triggers) = ROLLBACK_TRIGGERS.lock() {
        triggers.clear();
    }
}

/// Take and clear the accumulated triggers. Clearing immediately before `check_rollback` makes the
/// result exact per-check attribution.
fn drain_rollback_triggers() -> Vec<(&'static str, f32)> {
    ROLLBACK_TRIGGERS
        .lock()
        .map(|mut triggers| std::mem::take(&mut *triggers))
        .unwrap_or_default()
}

/// Optional net resources used by frame rows. Optional access keeps the shared system valid in
/// single-player.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tank::WeaponState;

    /// A representative, non-trivial sim state (every `Vec` populated, one side anchored and
    /// one released, non-default drive) so the canonicalization is exercised over every
    /// production field path — including each of the five carried-state sub-hash streams.
    fn sample() -> (Vec3, Quat, Vec3, Vec3, TrackDrive, TankSim) {
        let position = Vec3::new(1.5, 2.0, -70.25);
        let rotation = Quat::from_rotation_y(0.3);
        let linvel = Vec3::new(0.0, -0.153, 4.2);
        let angvel = Vec3::new(0.01, -0.02, 0.03);
        let drive = TrackDrive {
            throttle: 0.75,
            steer: -0.25,
            sides: [
                crate::track::sim::TrackDriveSide {
                    speed: 4.2,
                    phase: 137.25,
                },
                crate::track::sim::TrackDriveSide {
                    speed: 4.1,
                    phase: 136.9,
                },
            ],
        };
        let sim = TankSim {
            servos: vec![crate::tank::ServoState::test_new(0.4, 0.39, 0.64)],
            weapons: vec![WeaponState {
                reload_remaining: 1.25,
                recoil_offset: -0.4,
                recoil_velocity: 0.0,
                // Belt counter — cosmetic, deliberately NOT folded into the state hash below (see
                // `WeaponState::rounds_fired`); carried here only so the literal is complete.
                rounds_fired: 3,
                // Rounds left on the belt — fire-gating, hashed (in the `rld` stream), unlike the
                // cosmetic counter above.
                belt_remaining: 47,
            }],
        };
        (position, rotation, linvel, angvel, drive, sim)
    }

    /// Same state → same combined hash AND same sub-hashes. This is the join's core assumption: two
    /// worlds that reached an identical logical state must produce byte-identical hashes.
    #[test]
    fn identical_state_hashes_identically() {
        let (p, q, lv, av, drive, sim) = sample();
        let a = hash_tank_state(p, q, lv, av, &drive, &sim);
        let b = hash_tank_state(p, q, lv, av, &drive, &sim);
        assert_eq!(a.combined, b.combined);
        assert_eq!(
            (a.pos, a.rot, a.lv, a.av, a.sim),
            (b.pos, b.rot, b.lv, b.av, b.sim)
        );
    }

    /// A SINGLE flipped bit of angular velocity changes the combined hash and the `av` sub-hash, and
    /// leaves every other sub-hash untouched — the property that lets the analyzer localize a
    /// divergence to one component. The flip is one ULP: `av.z`'s least-significant mantissa bit.
    #[test]
    fn one_flipped_velocity_bit_diverges_only_that_component() {
        let (p, q, lv, av, drive, sim) = sample();
        let base = hash_tank_state(p, q, lv, av, &drive, &sim);

        let mut av2 = av;
        av2.z = f32::from_bits(av.z.to_bits() ^ 1);
        assert_ne!(
            av2.z.to_bits(),
            av.z.to_bits(),
            "the bit flip must change the bits"
        );
        let flipped = hash_tank_state(p, q, lv, av2, &drive, &sim);

        assert_ne!(
            base.combined, flipped.combined,
            "combined hash must catch the flip"
        );
        assert_ne!(base.av, flipped.av, "the av sub-hash must catch the flip");
        // Every OTHER component is unchanged — the localization guarantee.
        assert_eq!(base.pos, flipped.pos);
        assert_eq!(base.rot, flipped.rot);
        assert_eq!(base.lv, flipped.lv);
        assert_eq!(base.sim, flipped.sim);
    }

    /// `+0.0` and `−0.0` are the same number but different bits, and bit-exactness is the bar, so they
    /// must hash apart (a sign-flip through zero is a real last-bit divergence).
    #[test]
    fn signed_zero_hashes_apart() {
        let (p, q, _lv, av, drive, sim) = sample();
        let pos_zero = hash_tank_state(p, q, Vec3::new(0.0, 0.0, 0.0), av, &drive, &sim);
        let neg_zero = hash_tank_state(p, q, Vec3::new(-0.0, 0.0, 0.0), av, &drive, &sim);
        assert_ne!(pos_zero.lv, neg_zero.lv);
    }

    /// A one-ULP belt-phase difference must hash apart and localize to the `blt` stream —
    /// phase is force-station-advecting sim state, not cosmetic.
    #[test]
    fn belt_phase_ulp_localizes_to_belt_stream() {
        let (p, q, lv, av, drive, sim) = sample();
        let mut shifted = drive;
        shifted.sides[1].phase = f64::from_bits(drive.sides[1].phase.to_bits() ^ 1);
        let hn = hash_tank_state(p, q, lv, av, &drive, &sim);
        let hs = hash_tank_state(p, q, lv, av, &shifted, &sim);
        assert_ne!(hn.sim, hs.sim);
        assert_ne!(hn.blt, hs.blt);
        // The other carried-state streams are untouched by a belt flip.
        assert_eq!(hn.drv, hs.drv);
        assert_eq!(hn.srv, hs.srv);
        assert_eq!(hn.rld, hs.rld);
        assert_eq!(hn.rec, hs.rec);
    }

    /// Each carried-state field family flips ITS sub-hash (plus `sim` and `combined`) and no other —
    /// the per-field decode the window attribution relies on. One-ULP flips, same bar as the
    /// velocity-bit test.
    #[test]
    fn carried_state_flip_localizes_to_its_family() {
        let (p, q, lv, av, drive, sim) = sample();
        let base = hash_tank_state(p, q, lv, av, &drive, &sim);

        // Drive: steer one ULP off.
        let mut drive2 = drive;
        drive2.steer = f32::from_bits(drive.steer.to_bits() ^ 1);
        let d = hash_tank_state(p, q, lv, av, &drive2, &sim);
        assert_ne!(base.drv, d.drv);
        assert_ne!(base.sim, d.sim);
        assert_ne!(base.combined, d.combined);
        assert_eq!(
            (base.srv, base.rld, base.rec, base.blt),
            (d.srv, d.rld, d.rec, d.blt)
        );
        assert_eq!(
            (base.pos, base.rot, base.lv, base.av),
            (d.pos, d.rot, d.lv, d.av)
        );

        // Servo: velocity one ULP off.
        let [cur, prev, vel] = sim.servos[0].hash_fields();
        let mut sim2 = sim.clone();
        sim2.servos[0] =
            crate::tank::ServoState::test_new(cur, prev, f32::from_bits(vel.to_bits() ^ 1));
        let s = hash_tank_state(p, q, lv, av, &drive, &sim2);
        assert_ne!(base.srv, s.srv);
        assert_ne!(base.sim, s.sim);
        assert_eq!(
            (base.drv, base.rld, base.rec, base.blt),
            (s.drv, s.rld, s.rec, s.blt)
        );

        // Reload: timer one ULP off — must NOT touch the recoil stream despite sharing the weapon.
        let mut sim3 = sim.clone();
        sim3.weapons[0].reload_remaining =
            f32::from_bits(sim.weapons[0].reload_remaining.to_bits() ^ 1);
        let r = hash_tank_state(p, q, lv, av, &drive, &sim3);
        assert_ne!(base.rld, r.rld);
        assert_ne!(base.sim, r.sim);
        assert_eq!(
            (base.drv, base.srv, base.rec, base.blt),
            (r.drv, r.srv, r.rec, r.blt)
        );

        // Belt count: one round off — a fire-gating difference, so it must land in the reload
        // stream (`rld`, the fire-timer family it modulates) and nowhere else. (`rounds_fired`
        // has no such case: it is cosmetic and deliberately unhashed.)
        let mut simb = sim.clone();
        simb.weapons[0].belt_remaining = sim.weapons[0].belt_remaining.wrapping_add(1);
        let b = hash_tank_state(p, q, lv, av, &drive, &simb);
        assert_ne!(base.rld, b.rld);
        assert_ne!(base.sim, b.sim);
        assert_ne!(base.combined, b.combined);
        assert_eq!(
            (base.drv, base.srv, base.rec, base.blt),
            (b.drv, b.srv, b.rec, b.blt)
        );

        // Recoil: offset one ULP off — must NOT touch the reload stream.
        let mut sim4 = sim.clone();
        sim4.weapons[0].recoil_offset = f32::from_bits(sim.weapons[0].recoil_offset.to_bits() ^ 1);
        let c = hash_tank_state(p, q, lv, av, &drive, &sim4);
        assert_ne!(base.rec, c.rec);
        assert_ne!(base.sim, c.sim);
        assert_eq!(
            (base.drv, base.srv, base.rld, base.blt),
            (c.drv, c.srv, c.rld, c.blt)
        );

        // Belt state: the left side's speed one ULP off — localizes to the `blt` stream.
        let mut drive5 = drive;
        drive5.sides[0].speed = f32::from_bits(drive.sides[0].speed.to_bits() ^ 1);
        let a = hash_tank_state(p, q, lv, av, &drive5, &sim);
        assert_ne!(base.blt, a.blt);
        assert_ne!(base.sim, a.sim);
        assert_eq!(
            (base.drv, base.srv, base.rld, base.rec),
            (a.drv, a.srv, a.rld, a.rec)
        );
    }

    /// `rounds_fired` rolls back because it derives tracer cadence, but a dropped predicted round
    /// may leave that phase one round from authority without changing simulation truth. The
    /// production trace therefore excludes it; the same-platform fresh-App digest must not.
    #[test]
    fn fresh_app_digest_covers_the_rollback_tracer_phase() {
        let (p, q, lv, av, drive, sim) = sample();
        let base_trace = hash_tank_state(p, q, lv, av, &drive, &sim);
        let base = canonical_tank_state_digest(p, q, lv, av, &drive, &sim);

        let mut phase_shifted = sim.clone();
        phase_shifted.weapons[0].rounds_fired =
            phase_shifted.weapons[0].rounds_fired.wrapping_add(1);
        let shifted_trace = hash_tank_state(p, q, lv, av, &drive, &phase_shifted);
        let shifted = canonical_tank_state_digest(p, q, lv, av, &drive, &phase_shifted);

        assert_eq!(base_trace.combined, shifted_trace.combined);
        assert_eq!(base.simulation, shifted.simulation);
        assert_ne!(base.rounds_fired, shifted.rounds_fired);
        assert_ne!(base.rollback, shifted.rollback);
        assert_ne!(base, shifted);
    }
}
