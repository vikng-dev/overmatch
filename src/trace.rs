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
//!   `e` entity bits, `p`/`q` world pose, `ctl` (present+true only for the controlled tank). `cam`
//!   [x,y,z] + `camq` [x,y,z,w] the primary 3D camera's world pose (`GlobalTransform`), same on every
//!   tank row of a frame — OMITTED (not null) on a headless client with no camera, so the analyzer's
//!   camera-space section is opt-in and old traces parse unchanged. Client
//!   extras: `tick` (predicted), `conf` (GLOBAL last-confirmed server tick or null — the whole
//!   replication stream's high-water mark), `rb`/`rbt` (cumulative rollback / rolled-back-tick
//!   counts), `cp`/`cq` (live `VisualCorrection` error translation/quat, present only while a
//!   correction decays). Per-ENTITY confirmed authority for the predicted tank root, from its
//!   `ConfirmedHistory<C>` newest present sample: `confp` [x,y,z] latest confirmed `Position`,
//!   `confv` [x,y,z] latest confirmed `LinearVelocity`, `conft` the lightyear tick that
//!   `confp`/`confv` belong to. `vo` [x,y,z] + `voq` [x,y,z,w] the predicted root's live
//!   render-space error offset (`net::render_error::RenderErrorOffset`) — the sim-snap-hiding
//!   displacement this frame adds to the render `Transform`. Present only on the predicted root that
//!   carries the offset (omitted, not null, elsewhere and in SP). It separates a SIM snap (which
//!   `confp`/`rb` show) from VIEW motion: the rendered `p`/`q` already fold the offset in (it is what
//!   renders), so `p − vo` recovers the lightyear-visible pose. This tick is the entity's OWN last
//!   authoritative update — it can
//!   lag the global `conf` when the tank's replicated components stop changing (or stop arriving),
//!   the key discriminator for a silent desync. All three are omitted (not null) when no confirmed
//!   sample exists yet, and absent entirely in SP / SP-composition net builds (no `ConfirmedHistory`).
//! - `tick`   — per fixed tick, per tank root: sim truth — `p`/`q`/`lv`/`av`, `gnd` grounded wheel
//!   count, `anc` anchored (loaded/grounded) wheel count, `ancm` per-wheel anchor bitmask (bit i =
//!   slot i anchored; transitions = grounding churn, wheels gaining/losing load — NOT grip flicker:
//!   since the static↔kinetic blend in `driving.rs`, anchors stay `Some` while the wheel bears load
//!   and the grip regime lives in the continuous `w_static` weight, so `anc`≈`gnd` by design),
//!   `loads` per-wheel spring load (N, ~0.1 N), `thr`/`str` drive
//!   intent, `hc` count of TOUCHING hull contact pairs (only pairs avian flags as actually
//!   touching AND with still-overlapping AABBs — not the speculative pairs `Collisions::iter`
//!   also carries, nor the stale pairs a rollback restore strands with a set `TOUCHING` flag but
//!   disjoint AABBs), `pen` deepest real penetration among them (m, clamped `>= 0`: a speculative
//!   contact's negative separation gap reads as zero, not a signed distance). Both are honest now
//!   — `hc`/`pen` sit at 0 while the tank drives wheel-borne, and rise on genuine hull-vs-ground
//!   overlap; the earlier multi-metre `pen` at hc=1 during normal driving was a phantom
//!   (client-only) non-touching / rollback-stale pair. THE DIVERGENCE INSTRUMENT'S per-tick fields
//!   (analyzed offline by `scripts/divergence/analyze.py`): `own` — the world-independent tank
//!   identity the client/server join pairs on (the player's own tank: `Controlled` on the client/SP,
//!   `ControlledBy` on the server; the ownerless bot is `false` on both — NEVER the entity id, which
//!   differs per world). `h` — the combined canonical state hash: an exhaustive "did anything differ
//!   this tick?" over the pose/velocity BITS plus the carried `TankSim`/`DriveState`, computed
//!   world-independently (fixed field order, `Vec`s in spawn-sorted slot order, no entity id) so an
//!   identical `h` on both ends is bit-exact agreement. `hpos`/`hrot`/`hlv`/`hav`/`hsim` — the
//!   per-component sub-hashes (`hsim` folds `DriveState` + `TankSim` servos/weapons/anchors, the
//!   hidden state no pose field exposes), so a mismatch localizes to a component and the analyzer can
//!   name the first-divergence sub-component. See `hash_tank_state`. Caveat that remains BY DESIGN:
//!   avian's
//!   narrow phase measures manifolds from the START-of-step pose while this row records the
//!   post-solve pose, so inside a client rollback-correction burst (rows near `rp`/`rollback`
//!   activity) `pen` can report a deep pre-solve overlap the solver then pushed out — real
//!   transient world state, not a filtering miss; server/SP rows never show it. Client-only: `rp`
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
//! process). Rows are line-buffered through [`JsonlSink`] (a `BufWriter` flushed ~every 1 s from
//! `write` itself, and on a clean World drop via `BufWriter::drop`); a hard-killed process may
//! lose the unflushed tail (acceptable). The sink + NaN-safe JSON helpers are `pub(crate)` — the
//! suspension-force recorder (`susp_trace`, driving.rs) shares them.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use avian3d::prelude::{AngularVelocity, Collisions, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use serde_json::{Value, json};

use crate::driving::{DriveState, Suspension};
use crate::tank::{Controlled, Tank, TankSim, WheelIndex};

use bevy::ecs::system::SystemParam;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::{
    ControlledBy, LocalTimeline, PredictionManager, PredictionMetrics, ReplicationCheckpointMap,
    Rollback, RollbackSystems, VisualCorrection,
};

/// The one JSONL sink both recorders share (`TraceWriter` here, `susp_trace` in `driving.rs`):
/// buffered line writes with a ~1 s flush cadence folded into `write` itself, so every consumer
/// gets the same tail-protection behavior without owning a flush system. Rows go through
/// `serde_json::Value` (see [`num`]) so a corrupt frame emits `null`, never invalid-JSON
/// `NaN`/`inf` — the recorders exist precisely for the corrupt regimes.
///
/// Flushing is best-effort tail protection: line-buffered writes reach disk about every second
/// (checked on each write — rows arrive every frame/tick while a recorder is armed) and on a
/// clean exit (the `BufWriter` flushes on drop); a hard-killed process may lose the unflushed
/// remainder (accepted).
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
    /// The composition role, carried so the Startup `write_meta` system can stamp the `meta` row
    /// (it runs after `install`, which no longer writes the row itself — see fix 6).
    role: &'static str,
}

impl TraceWriter {
    /// Append one row — see [`JsonlSink::write`] for the error/flush discipline.
    fn write(&mut self, row: &Value) {
        self.sink.write(row);
    }
}

// --- JSON leaf helpers -----------------------------------------------------------------------
// serde_json's `From<f64>` maps NaN/Inf to `Null` rather than emitting invalid JSON — exactly the
// safety a pose recorder needs, since a corrupt frame must still produce a parseable line.
// `pub(crate)` so the suspension-force recorder (`susp_trace`, driving.rs) emits with the same
// NaN discipline instead of growing a parallel implementation.

pub(crate) fn num(x: f32) -> Value {
    Value::from(x as f64)
}

fn vec3(v: Vec3) -> Value {
    Value::Array(vec![num(v.x), num(v.y), num(v.z)])
}

fn quat(q: Quat) -> Value {
    Value::Array(vec![num(q.x), num(q.y), num(q.z), num(q.w)])
}

// --- Per-tick state hash (the divergence instrument's exhaustive boolean) ---------------------
// A canonical, WORLD-INDEPENDENT hash of a tank root's sim state, computed identically on client and
// server so the offline join (`scripts/divergence/analyze.py`) can answer "did anything differ this
// tick?" with a single u64 compare, and localize a difference to a sub-component. World-independence
// is by construction: the hash consumes ONLY f32 bit patterns of pose/velocity/carried-sim, in a
// fixed field order, with every `Vec` walked in its spawn-sorted index order (`WheelIndex`/
// `ServoIndex`/`WeaponIndex` — identical across the two ECS worlds by `spawn_tank_sim`'s
// sorted-by-name assignment). NOTHING that differs between worlds enters it — no entity id, no
// pointer, no `HashMap` iteration, no archetype order. Two worlds that reached the same logical state
// therefore hash identically even though their entity indices differ (measured 4294966669 vs
// 4294966650 for the same tank). The per-tank ROW carries `own` (below) as the cross-world pairing
// key, so the join never needs the entity id it cannot compare.
//
// Bit-exactness is the bar (flat-ground cruise is already measured bit-exact client-vs-server), so
// every f32 enters as its raw `to_bits()` — `+0.0`/`−0.0` and any last-ulp difference flip the hash.

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

/// One tank's per-tick state hash: the combined exhaustive boolean plus a per-component breakdown so
/// the analyzer can localize a divergence (the measured signature is `|Δav|`-first at contact
/// transients). `sim` folds both `DriveState` and the carried `TankSim` (servos, weapon reloads/
/// recoil, wheel anchors) — the hidden state no pose/velocity field exposes, so the hash is the ONLY
/// cross-world witness for it.
struct TankStateHash {
    combined: u64,
    pos: u64,
    rot: u64,
    lv: u64,
    av: u64,
    sim: u64,
}

/// Hash a tank root's canonical sim state (see the module-level note on world-independence). Pure and
/// ECS-free precisely so it is unit-testable: same inputs → same hash, one flipped velocity bit → a
/// different hash, and — because no entity ever enters it — hash equality is independent of the two
/// worlds' entity ids. Field order is fixed and load-bearing: `position, rotation, linvel, angvel`,
/// then `DriveState(throttle, steer)`, then each `TankSim` `Vec` in slot order.
fn hash_tank_state(
    position: Vec3,
    rotation: Quat,
    linvel: Vec3,
    angvel: Vec3,
    drive: &DriveState,
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

    let mut hs = Fnv64::new();
    hs.write_f32(drive.throttle());
    hs.write_f32(drive.steer());
    for servo in &sim.servos {
        for field in servo.hash_fields() {
            hs.write_f32(field);
        }
    }
    for weapon in &sim.weapons {
        hs.write_f32(weapon.reload_remaining);
        hs.write_f32(weapon.recoil_offset);
        hs.write_f32(weapon.recoil_velocity);
    }
    for anchor in &sim.anchors {
        // A discriminant distinguishes `None` (slipping/airborne) from `Some((0,0,0))` — a grip
        // released vs a grip anchored exactly at the origin are different sim states and must hash
        // apart.
        match anchor {
            None => hs.write_u32(0),
            Some(point) => {
                hs.write_u32(1);
                hs.write_vec3(*point);
            }
        }
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
    }
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
    let sink = match JsonlSink::create(&resolved) {
        Ok(sink) => sink,
        Err(err) => {
            error!("trace: cannot open {}: {err}", resolved.display());
            return false;
        }
    };
    info!("trace: recording {role} rows to {}", resolved.display());
    app.insert_resource(TraceWriter { sink, role });
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

/// MP client: frame (with prediction/correction extras) + tick + the rollback observer. Replay ticks
/// ARE recorded (stamped `rp` — fix 4), so analysis sees the corrected re-simulation, not the
/// abandoned misprediction. A PreUpdate system clears the trigger slot before lightyear's rollback
/// check so `trg` attribution is exact (fix 2).
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
    // The primary 3D world camera's pose. Recorded so the analyzer can resolve the controlled tank
    // into camera space and catch viewer-side transients — a camera-follow scheduling race steps the
    // camera a frame relative to the tank, which is invisible in the world-space pose stream but is a
    // visible lurch on screen. Empty on a headless client (no camera spawned) → the fields are then
    // OMITTED, not null, keeping the row shape identical to a pre-instrumentation trace.
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
    view_offset: Query<&crate::net::render_error::RenderErrorOffset>,
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
            // The prediction/replication resources exist together on a real MP client. In the
            // single-player composition none are present, so access them optionally and skip the net
            // extras rather than panic on system-param validation (fix 3): the row then has the same
            // shape as an SP frame row.
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

    // Per-body count of TOUCHING contact pairs + deepest real penetration in ONE pass over the
    // contact graph (fix 5), so the roots loop below is a map lookup rather than a full re-scan per
    // tank. A tank-vs-tank pair counts for both roots, which is correct — each body has that
    // contact. `body1`/`body2` are the rigid-body entities (the tank root here, since the hull/part
    // colliders are its children).
    //
    // Only pairs avian reports as actually TOUCHING count (`ContactPair::is_touching` — the
    // `TOUCHING` flag, verified in avian 0.7 source). A contact PAIR exists as soon as two colliders'
    // speculative margins overlap, and a fast body's margin is unbounded; the client compounds it,
    // since a rollback-restored contact graph can carry a stale manifold whose overlap never
    // happened. Counting those inflated `hc` and, worse, `pen` — the measured hc=1 / pen≈2.9 m while
    // the tank drove cleanly on its wheels. Penetration is likewise read only from a touching pair's
    // deepest contact and clamped to `>= 0`: a speculative contact reports a NEGATIVE penetration
    // (the separation gap), which must read as "no penetration", not a signed distance.
    //
    // `aabbs_disjoint` closes the remaining stale-flag hole (measured on rollback-REPLAY ticks of a
    // drop test: multi-metre `pen` while airborne): when a rollback restore teleports the body, the
    // pair's AABBs stop overlapping and avian's narrow phase EARLY-OUTS — it sets `DISJOINT_AABB`
    // and returns without clearing `TOUCHING` or the manifolds (vendored 0.7 source,
    // `narrow_phase/system_param.rs`), leaving the abandoned timeline's contact data flagged as
    // touching until the pair's deferred removal. A pair both "touching" and "AABBs no longer
    // overlap" is definitionally stale.
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
        // `anc` counts anchored wheels — since the static↔kinetic friction blend (`ramp_drive`,
        // src/driving.rs) an anchor stays `Some` for as long as the wheel bears load and releases
        // only on the airborne/unloaded paths, so this is a loaded/grounded-wheel count (≈ `gnd`),
        // NOT a grip-state count. The grip regime is the continuous `w_static` weight in
        // `ramp_drive` now — it never appears here as a discrete state to count.
        let anchors = sim.anchors.iter().filter(|a| a.is_some()).count();
        // Per-wheel anchor bitmask (bit i = slot i anchored), so analysis can count per-wheel
        // anchor TRANSITIONS across ticks. Post-blend these transitions are grounding churn (a
        // wheel gaining/losing load), not the stick-speed plant/release grip flicker the
        // friction-continuity work measured with this field — that flicker no longer exists as a
        // Some/None flip (see the anchor-relax comment in src/driving.rs). The plain `anc` count
        // hides a simultaneous gain+loss (one wheel loads as another unloads, net count unchanged);
        // the bitmask exposes each slot's flip. u32 covers any plausible wheel count (Tiger has
        // 16); higher slots would silently drop, acceptable.
        let anchor_mask: u32 = sim
            .anchors
            .iter()
            .take(32)
            .enumerate()
            .filter(|(_, a)| a.is_some())
            .fold(0u32, |m, (i, _)| m | (1 << i));

        // Contact pairs whose rigid body is this tank root, and the deepest penetration among them
        // — the collision-stress signal the jitter correlates with.
        let (hull_contacts, penetration) = contacts.get(&entity).copied().unwrap_or((0, 0.0));

        // The world-independent per-tick state hash — the divergence instrument's exhaustive boolean.
        // Feeds off the tick-truth pose/velocity and the carried sim state already in hand; costs a
        // few dozen FNV rounds, and only when tracing is armed (this whole system is registered only
        // then).
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

        // `mut` for the `rp` stamp below.
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
            "ancm": anchor_mask,
            "loads": Value::Array(loads),
            "thr": num(drive.throttle()),
            "str": num(drive.steer()),
            "hc": hull_contacts,
            "pen": num(penetration),
            "ctl": controlled,
            // Cross-world tank identity for the hash join (world-independent — never the entity id).
            "own": own,
            // The per-tick state hash: `h` combined (exhaustive "did anything differ?"), then the
            // per-component breakdown so a difference localizes to pose vs velocity vs carried sim.
            "h": hash.combined,
            "hpos": hash.pos,
            "hrot": hash.rot,
            "hlv": hash.lv,
            "hav": hash.av,
            "hsim": hash.sim,
        });
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

/// Record that `component`'s rollback condition tripped this check, by `magnitude` (the measured
/// client/server divergence). Called from [`note_if_tripped`] when a condition returns true. No-op
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

/// The single rollback-condition body shared by every predicted component's closure (`net::protocol`
/// — fix 7): trip when `magnitude >= threshold`, and note the trip for trace attribution when it
/// does. Returns whether the component's condition tripped (lightyear's `should_rollback` contract).
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

/// Take and clear the accumulated triggers. Called once per rollback start. With the
/// clear-before-check discipline ([`clear_rollback_triggers`]) the slot holds exactly the trips from
/// this frame's `check_rollback` at drain time — correction-decay re-tests from earlier frames were
/// already wiped — so the drained pairs are exact per-check attribution, not a weight.
fn drain_rollback_triggers() -> Vec<(&'static str, f32)> {
    ROLLBACK_TRIGGERS
        .lock()
        .map(|mut triggers| std::mem::take(&mut *triggers))
        .unwrap_or_default()
}

/// Net resources the `frame` row's client extras read, bundled so `record_frame`'s parameter stays a
/// single name. All three exist together on a real MP client (`LocalTimeline` + `PredictionMetrics`
/// from the prediction stack, `ReplicationCheckpointMap` from shared replication registration) — but
/// accessed OPTIONALLY so the single-player composition (no lightyear plugins) doesn't panic
/// system-param validation on the missing resources (fix 3).
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

    /// A representative, non-trivial sim state (both `Vec`s populated, one anchor set and one
    /// released) so the canonicalization is exercised over every field path.
    fn sample() -> (Vec3, Quat, Vec3, Vec3, DriveState, TankSim) {
        let position = Vec3::new(1.5, 2.0, -70.25);
        let rotation = Quat::from_rotation_y(0.3);
        let linvel = Vec3::new(0.0, -0.153, 4.2);
        let angvel = Vec3::new(0.01, -0.02, 0.03);
        let drive = DriveState::default();
        let sim = TankSim {
            servos: Vec::new(),
            weapons: vec![WeaponState {
                reload_remaining: 1.25,
                recoil_offset: -0.4,
                recoil_velocity: 0.0,
            }],
            anchors: vec![Some(Vec3::new(3.0, 0.0, -70.0)), None],
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

    /// A released anchor (`None`) must not collide with one anchored at the origin (`Some(0,0,0)`):
    /// the discriminant is load-bearing carried sim state.
    #[test]
    fn anchor_none_differs_from_some_origin() {
        let (p, q, lv, av, drive, _sim) = sample();
        let none = TankSim {
            servos: vec![],
            weapons: vec![],
            anchors: vec![None],
        };
        let some = TankSim {
            servos: vec![],
            weapons: vec![],
            anchors: vec![Some(Vec3::ZERO)],
        };
        let hn = hash_tank_state(p, q, lv, av, &drive, &none);
        let hs = hash_tank_state(p, q, lv, av, &drive, &some);
        assert_ne!(hn.sim, hs.sim);
    }

    /// Entity-id independence, made explicit: the hash function takes no entity and no ECS handle, so
    /// two "worlds" whose only difference is the (absent) entity id produce the same hash. This is the
    /// world-independence guarantee the join relies on, expressed as a test.
    #[test]
    fn hash_is_entity_id_independent() {
        // There is simply no entity to pass — the signature itself is the proof. Two calls standing in
        // for the client world and the server world (different entity ids there, identical state here)
        // agree.
        let (p, q, lv, av, drive, sim) = sample();
        let client_world = hash_tank_state(p, q, lv, av, &drive, &sim);
        let server_world = hash_tank_state(p, q, lv, av, &drive, &sim);
        assert_eq!(client_world.combined, server_world.combined);
    }
}
