//! An opt-in, passive JSONL recorder for render pose, fixed-step state, and rollback events.
//!
//! Invariant: tracing never writes simulation state. `SPIKE_TRACE` enables recorder registration;
//! role-qualified paths prevent concurrently launched compositions from sharing a sink.
//!
//! Rows have `k` values `meta`, `frame`, `tick`, `rollback`, `grip_anchor_compare`,
//! `grip_resync_request`, or `grip_checkpoint_apply`. Fields unavailable in a composition are omitted
//! rather than represented as null, except a resync with no retained anchor context, whose diagnostic
//! values are explicitly null. Cross-process analysis joins on `tick` and `role`, never on entity
//! identifiers. [`scripts/divergence/analyze.py`](../../scripts/divergence/analyze.py) consumes the
//! base schema; grip rows are the repair-loop root-cause capture.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use avian3d::prelude::{AngularVelocity, Collisions, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use serde_json::{Value, json};

use crate::CombatantId;
use crate::tank::{Controlled, Tank, TankSim};
use crate::track::sim::{
    TankTransmission, TrackContacts, TrackDrive, TrackGrip, TrackGripEffect, TrackGripElements,
};
use crate::track::transmission::{TransmissionProjectionValue, transmission_state_projection};

mod state_hash;

use state_hash::hash_tank_state_with_elements;
#[cfg(test)]
pub(crate) use state_hash::{
    CanonicalTankStateDigest, canonical_element_hash, canonical_tank_state_digest,
};

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

    pub(crate) fn record_grip_anchor_compare(
        &mut self,
        sample: GripAnchorTrace,
        request_reason: GripRequestReason,
        evidence_spent: bool,
        request_due: bool,
        request_sent: bool,
    ) {
        let mut row = grip_trace_row("grip_anchor_compare", sample, request_reason);
        let object = row.as_object_mut().expect("grip trace row is an object");
        object.insert("evidence_spent".into(), Value::Bool(evidence_spent));
        object.insert("request_due".into(), Value::Bool(request_due));
        object.insert("request_sent".into(), Value::Bool(request_sent));
        self.write(&row);
    }

    pub(crate) fn record_grip_resync_request(
        &mut self,
        sample: GripAnchorTrace,
        request_reason: GripRequestReason,
    ) {
        self.write(&grip_trace_row(
            "grip_resync_request",
            sample,
            request_reason,
        ));
    }

    pub(crate) fn record_grip_resync_without_anchor(
        &mut self,
        combatant: CombatantId,
        tick: u32,
        epoch: u32,
        request_reason: GripRequestReason,
    ) {
        self.write(&json!({
            "k": "grip_resync_request",
            "tick": tick,
            "combatant": combatant.0,
            "request_reason": request_reason.as_str(),
            "anchor_producing_tick": Value::Null,
            "history_tick": Value::Null,
            "authority_force": Value::Null,
            "predicted_force": Value::Null,
            "authority_torque": Value::Null,
            "predicted_torque": Value::Null,
            "authority_belt": Value::Null,
            "predicted_belt": Value::Null,
            "e_v": Value::Null,
            "e_omega": Value::Null,
            "e_belt": Value::Null,
            "epoch": epoch,
            "authority_digest": Value::Null,
            "predicted_digest": Value::Null,
            "digest_match": Value::Null,
        }));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_grip_checkpoint_apply(
        &mut self,
        tick: u32,
        combatant: CombatantId,
        epoch: u32,
        state_entering_tick: u32,
        checkpoint_hash: u64,
        field_bits_changed: bool,
        rollback: bool,
    ) {
        self.write(&json!({
            "k": "grip_checkpoint_apply",
            "tick": tick,
            "combatant": combatant.0,
            "epoch": epoch,
            "state_entering_tick": state_entering_tick,
            "checkpoint_hash": checkpoint_hash,
            "field_bits_changed": field_bits_changed,
            "rollback": rollback,
        }));
    }
}

/// Complete values at one anchor/history comparison, copied so an ensuing resync request can emit
/// the same evidence into a separate JSONL row.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GripAnchorTrace {
    pub(crate) tick: u32,
    pub(crate) combatant: CombatantId,
    pub(crate) anchor_producing_tick: u32,
    pub(crate) history_tick: u32,
    pub(crate) authority: TrackGripEffect,
    pub(crate) predicted: TrackGripEffect,
    pub(crate) e_v: f32,
    pub(crate) e_omega: f32,
    pub(crate) e_belt: [f32; 2],
    pub(crate) epoch: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GripRequestReason {
    None,
    Effect,
    Digest,
    EffectAndDigest,
    StaleCheckpoint,
}

impl GripRequestReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Effect => "effect",
            Self::Digest => "digest",
            Self::EffectAndDigest => "effect_and_digest",
            Self::StaleCheckpoint => "stale_checkpoint",
        }
    }
}

fn grip_trace_row(kind: &'static str, sample: GripAnchorTrace, reason: GripRequestReason) -> Value {
    json!({
        "k": kind,
        "tick": sample.tick,
        "combatant": sample.combatant.0,
        "request_reason": reason.as_str(),
        "anchor_producing_tick": sample.anchor_producing_tick,
        "history_tick": sample.history_tick,
        "authority_force": vec3(sample.authority.traction_force),
        "predicted_force": vec3(sample.predicted.traction_force),
        "authority_torque": vec3(sample.authority.traction_torque),
        "predicted_torque": vec3(sample.predicted.traction_torque),
        "authority_belt": [
            num(sample.authority.belt_reaction[0]),
            num(sample.authority.belt_reaction[1]),
        ],
        "predicted_belt": [
            num(sample.predicted.belt_reaction[0]),
            num(sample.predicted.belt_reaction[1]),
        ],
        "e_v": num(sample.e_v),
        "e_omega": num(sample.e_omega),
        "e_belt": [num(sample.e_belt[0]), num(sample.e_belt[1])],
        "epoch": sample.epoch,
        "authority_digest": sample.authority.field_digest,
        "predicted_digest": sample.predicted.field_digest,
        "digest_match": sample.authority.field_digest == sample.predicted.field_digest,
    })
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
            &TrackGrip,
            Option<&TrackGripElements>,
            &TankTransmission,
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

    for (
        entity,
        position,
        rotation,
        linvel,
        angvel,
        drive,
        grip,
        elements,
        transmission,
        track_contacts,
        sim,
        controlled,
    ) in &roots
    {
        // Trace schema v2 (phase B): the per-wheel suspension topology is gone. `gnd` counts
        // contacting SIDES (0–2), `loads` is per-side ELASTIC load sums (the stable baseline
        // channel — the damped actual load, which scales grip, rides as `loads_act`),
        // `thr`/`str` are the shaped command, `belt`/`bph` the per-side belt speed and phase.
        let grounded = track_contacts
            .0
            .iter()
            .filter(|side| !side.is_empty())
            .count();
        // Round to ~0.1 N: solver-noise digits past that are neither meaningful nor worth
        // the row width.
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
        let (hull_contacts, penetration) = contacts.get(&entity).copied().unwrap_or((0, 0.0));

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

        // The world-independent authority-simulation hash. It feeds off tick-truth pose/velocity
        // and carried state already in hand, except the view-only tracer phase documented on
        // `TankStateHash`; costs a few dozen FNV rounds and runs only when tracing is armed.
        let hash = hash_tank_state_with_elements(
            position.0,
            rotation.0,
            linvel.0,
            angvel.0,
            drive,
            grip,
            elements,
            transmission,
            sim,
        );
        // Remote clients intentionally receive no exact element field. Null makes that disclosure
        // boundary explicit so the analyzer omits private-state and combined equality for the pair.
        let disclosed_element_hash = own.then_some(hash.elm);

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
            "loads_act": Value::Array(loads_act),
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
            "htrn": hash.trn,
            "helm": disclosed_element_hash,
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
            // Same stable field order as `htrn`; the scheduler occupies tag/from/to slots so the
            // verbose diagnostic shape stays fixed across variants.
            let mut trn = Vec::with_capacity(18);
            for field in transmission_state_projection(&transmission.0) {
                match field.value {
                    TransmissionProjectionValue::U8(value) => trn.push(Value::from(value)),
                    TransmissionProjectionValue::I8(value) => trn.push(Value::from(value)),
                    TransmissionProjectionValue::Bool(value) => trn.push(Value::from(value)),
                    TransmissionProjectionValue::F32(value) => trn.push(num(value)),
                    TransmissionProjectionValue::Scheduler { tag, from, to } => {
                        trn.extend([Value::from(tag), Value::from(from), Value::from(to)]);
                    }
                }
            }
            let trn = Value::Array(trn);
            row.as_object_mut()
                .expect("json! built an object")
                .insert("simf".into(), json!({"srv": srv, "wpn": wpn, "trn": trn}));
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

    #[test]
    fn grip_anchor_trace_contains_the_root_cause_capture_fields() {
        let row = grip_trace_row(
            "grip_anchor_compare",
            GripAnchorTrace {
                tick: 20,
                combatant: CombatantId(7),
                anchor_producing_tick: 18,
                history_tick: 18,
                authority: TrackGripEffect {
                    traction_force: Vec3::X,
                    traction_torque: Vec3::Y,
                    belt_reaction: [1.0, 2.0],
                    field_digest: 3,
                },
                predicted: TrackGripEffect {
                    traction_force: Vec3::Z,
                    traction_torque: -Vec3::Y,
                    belt_reaction: [4.0, 5.0],
                    field_digest: 6,
                },
                e_v: 0.1,
                e_omega: 0.2,
                e_belt: [0.3, 0.4],
                epoch: 8,
            },
            GripRequestReason::Effect,
        );
        let object = row.as_object().expect("trace row is an object");
        for field in [
            "request_reason",
            "anchor_producing_tick",
            "history_tick",
            "authority_force",
            "predicted_force",
            "authority_torque",
            "predicted_torque",
            "authority_belt",
            "predicted_belt",
            "e_v",
            "e_omega",
            "e_belt",
            "epoch",
            "authority_digest",
            "predicted_digest",
            "digest_match",
        ] {
            assert!(
                object.contains_key(field),
                "missing grip trace field {field}"
            );
        }
    }
}
