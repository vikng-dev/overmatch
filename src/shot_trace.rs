//! `SPIKE_SHOT_TRACE`: the SHOT-LIFECYCLE recorder — a sibling of the jitter recorder
//! (`crate::trace`) and the cost recorder (`crate::cost`), built around the one thing those two stop
//! short of: the SHOT. We have a correlation spine ([`crate::ShotId`]) and, until this module, nothing
//! that records what happens to a shot once it leaves the muzzle.
//!
//! It exists to turn two THEORY-SIZED constants into MEASURED ones:
//! [`crate::ballistics::RICOCHET_HOLD_TICKS`] (the grace window a client shell waits at armor for the
//! server's verdict) and `SanctionedShots::MAX_AGE_SECS` (how long an unconsumed outcome lingers).
//! Both were sized from an RTT argument, never against a real link. The hold window must cover
//! `(P − S) + OWL` — the client's prediction lead plus one-way latency — and THAT quantity is exactly
//! what a keyframe's `recv_tick − bounce_tick` measures, so a hold-time histogram plus a
//! keyframe-arrival distribution is the number that should size the constant.
//!
//! PASSIVE, like every recorder here: nothing in this module writes sim state. OFF unless
//! `SPIKE_SHOT_TRACE=<path>` is set at startup — [`install`] opens the sink only then, so an unset var
//! costs one `std::env::var` lookup at plugin-build. The hot-path call sites (the ballistics march;
//! `receive_fire_events`, which sees an MG's 12.5 rounds/s/gun) read the recorder as
//! `Option<ResMut<ShotTrace>>` and pay one `Option` check when it is absent — the same discipline
//! `crate::cost::CostTrace` uses inside `integrate_projectiles`.
//!
//! The sink path is role-qualified through the shared [`crate::trace::role_path`], so a client and a
//! server launched from one shell with the same `SPIKE_SHOT_TRACE` value write
//! `<path>.client.jsonl` / `<path>.server.jsonl` and never truncate each other. As with the cost
//! recorder, give `SPIKE_TRACE` / `SPIKE_COST_TRACE` / `SPIKE_SHOT_TRACE` DIFFERENT base paths when
//! arming several at once, or two recorders role-qualify onto the same file and tear each other's rows.
//!
//! # NET-NEUTRAL BY DESIGN
//!
//! Like [`crate::ClientReplica`] / [`crate::Replaying`] / [`crate::PredictedPresent`], this module
//! lives at the crate root and names NO netcode: every tick it records is a plain `u32` (the sim's own
//! vocabulary — `ShotId::fire_tick`, `PredictedPresent`, and the wire ticks the net layer already
//! unwraps at its boundary). That is what lets the always-runnable sim layer (`ballistics`) emit the
//! contact/hold/re-seed rows without naming lightyear (`tests/net_boundary`), while `net::client` /
//! `net::server` emit the wire rows from their own side of the boundary.
//!
//! # Row schema (one compact JSON object per line, `k` = kind)
//!
//! Every shot-scoped row carries the [`ShotId`] triple — `sh` (the shooter entity's bits), `w` (weapon
//! slot), `ft` (fire tick) — plus `t`, the tick the row was observed at IN THAT PROCESS'S OWN
//! TIMELINE: the server tick on the server, the predicted present `P` on the client.
//!
//! **Cross-process joining.** `sh` is NOT comparable across ends (the client's `ShotId::shooter` is
//! its local replica of the server's tank — different entity ids in different worlds, exactly as
//! `crate::trace`'s `own` field exists to work around). So the analyzer joins server rows to client
//! rows on `(w, ft)` and disambiguates a same-tick same-slot collision (two tanks firing the same
//! weapon slot on one tick) with the fire ORIGIN, which both ends record verbatim off the wire. The
//! analyzer reports any residual ambiguity rather than hiding it.
//!
//! - `meta` — once, at Startup: `role` ("client"|"server"), `tick_hz`, `ver`, and the CONFIGURED
//!   windows the analyzer measures against: `hold_ticks` ([`crate::ballistics::RICOCHET_HOLD_TICKS`]),
//!   `overdue_ticks` ([`crate::ballistics::OVERDUE_MARGIN_TICKS`]), `max_age_secs`
//!   (`SanctionedShots::MAX_AGE_SECS`).
//!
//! SERVER rows (`net::server`) — TWO different facts, and keeping them apart is the point:
//!
//! *EMISSION*, the tick a thing HAPPENED in the authority's sim (written by the three observers that
//! push onto the redundancy window: `broadcast_fire` / `on_shell_ricochet` / `on_shell_terminal`).
//! Every cross-process measurement below keys off these ticks, never off a send:
//! - `fire` — a shot was fired: `t` = the fire tick, `o` origin, `tr` tracer, `cal` caliber.
//! - `kf` — the authority RESOLVED a ricochet: `t` = the server bounce tick, `seq` the bounce ordinal.
//! - `cf` — the authority resolved an impact confirm (the shot's armor TERMINAL): `t` = the server
//!   impact tick, `pen` the penetration verdict, `ab` how many bounces preceded it.
//!
//! *TRANSMISSION*, which datagram carried it (written by `net::server::broadcast_fire_window` — the
//! ONE send site since the redundancy window moved onto the clock):
//! - `send` — this event rode the burst broadcast on tick `t`: `s` = the stream ("fire"|"kf"|"cf"),
//!   `c` = the event's age in ticks at this send (0 on its own tick), plus `seq` on a keyframe. One row
//!   per event per burst, so COUNTING the `send` rows for one `(ShotId, s)` gives the datagram copies
//!   that event actually got — the redundancy the window claims, MEASURED rather than argued. It is
//!   only informative because emission and send are now different moments: under the old event-driven
//!   send they coincided, and an isolated 88 bounce rode exactly ONE copy (the flake the clock-driven
//!   window fixes). `scripts/shot/analyze.py` reports the copies-per-event distribution.
//!
//! CLIENT rows (`net::client` for the wire half, `ballistics` for the shell half):
//! - `fire_rx` — a `FireEvent` arrived: `t` = `P` at receive, `dup` whether the [`ShotId`] dedup
//!   rejected it (a redundancy-window duplicate), `cu` the catch-up ticks the shell will fly at spawn,
//!   `bnew` the NEWEST fire tick in the burst that carried it. `bnew > ft` on a NEW (`dup: false`)
//!   event means the burst sent for this shot never arrived and a LATER burst's redundancy window
//!   repaired the loss — the measurement that tells you whether the window earns its bytes.
//! - `kf_rx` — a `RicochetKeyframe` arrived: `t` = `P` at receive, `bt` the server bounce tick it
//!   carries, `dup` whether the shot already had this bounce ordinal stored. `t − bt` IS the quantity
//!   [`crate::ballistics::RICOCHET_HOLD_TICKS`] must cover (see the module intro).
//! - `cf_rx` — an `ImpactConfirm` arrived: `it` the server impact tick, `dup` as above.
//! - `drop` — an arriving event was REJECTED at the receive gate before it could key anything: `s` =
//!   the stream ("fire"|"kf"|"cf"), `res` = why ("unresolved_shooter" — the shot's shooter does not
//!   resolve to a live replicated tank, `net::client::shooter_is_live`). Recorded because that guard
//!   drops precisely the class this recorder was built to find (a fire outrunning its shooter's
//!   replica at connect/respawn/loss): counting only the events that SURVIVE the gate would hide both
//!   the race and the guard's cost. The row's `ShotId` is the garbage-keyed one, which the `(w, ft)`
//!   join sees through — so a `drop` followed by a clean `fire_rx` for the same shot reads as what it
//!   is: the redundancy window turning a corruption into a ~1-frame delay.
//! - `spawn` — a cosmetic shell was spawned for the shot: `src` = "obs" (an observer's replica, id
//!   straight off the wire) or "own" (a locally-fired shell, id completed by
//!   `net::protocol::stamp_shot_ids`; on the server that same stamp reads `src: "auth"` — the
//!   authoritative shell).
//! - `contact` — a `Shot`-carrying client shell reached ARMOR: `res` = how the state machine resolved
//!   it — "pre_bounce" (the keyframe had already arrived), "pre_term" (the confirm had), or "hold"
//!   (the expected path: freeze and wait).
//! - `hold` — a HOLD ended: `held` the ticks it waited, `res` = "bounce" (re-seeded from the
//!   sanctioned keyframe), "terminal" (resolved at the confirmed armor read), or "expired" (the
//!   window ran out — the quiet dissolve; correctness never depended on delivery, but the shot's
//!   picture is lost).
//! - `overdue` — F3's tick-triggered consumption fired: the shell MISSED the plate the server resolved
//!   on (interpolated-pose divergence) and the outcome was consumed by the clock instead. `res` =
//!   "bounce"|"terminal", `late` = how many ticks past the outcome's server tick `P` had run.
//! - `end` — the shell's picture ended: `why` = "terminal" (the confirmed armor read) |
//!   "bounce_dissolve" (the hold expired — the quiet dissolve) | "terrain" (a terrain stop, which
//!   needs no confirm: static geometry, both ends agree) | "kill_floor" (flew out of the world) |
//!   "catchup_landed" (the round had already resolved during its catch-up fast-forward, so no tracer
//!   ever flew).
//!
//! Rows are line-buffered through the shared [`JsonlSink`] (~1 s flush cadence, plus the clean-drop
//! flush), so a hard-killed process may lose the unflushed tail — accepted, as everywhere else.
//!
//! Offline analysis: `scripts/shot/analyze.py` (per-shot lifecycle reconstruction, the HOLD-TIME
//! HISTOGRAM, keyframe/confirm arrival-lead distributions, carry-through success rate, dedup/repair
//! rate, and the never-consumed counts).

use bevy::prelude::*;
use serde_json::{Value, json};

use crate::ShotId;
use crate::ballistics::{OVERDUE_MARGIN_TICKS, RICOCHET_HOLD_TICKS, SanctionedShots};
use crate::trace::{JsonlSink, role_path};

/// The open shot-lifecycle sink. Present iff `SPIKE_SHOT_TRACE` was set at startup ([`install`]
/// inserts it and reports whether it did), so the recorder's own systems are registered only in an
/// armed run and every call site elsewhere reads it as an `Option`.
#[derive(Resource)]
pub(crate) struct ShotTrace {
    sink: JsonlSink,
    /// The composition role, carried for the Startup `meta` row (Startup runs before any recorder
    /// call site, so the row stays first in the file).
    role: &'static str,
}

impl ShotTrace {
    /// Append one shot-scoped row: the kind, the observing tick, the [`ShotId`] triple, and the
    /// kind's own fields (a `json!({...})` object, merged in).
    pub(crate) fn row(&mut self, kind: &'static str, tick: u32, shot: ShotId, extra: Value) {
        let mut row = json!({
            "k": kind,
            "t": tick,
            "sh": shot.shooter.to_bits(),
            "w": shot.weapon,
            "ft": shot.fire_tick,
        });
        let object = row.as_object_mut().expect("json! built an object");
        if let Value::Object(fields) = extra {
            for (key, value) in fields {
                object.insert(key, value);
            }
        }
        self.sink.write(&row);
    }
}

/// Record one row through an OPTIONAL recorder — the shape every call site uses, so an unarmed run
/// pays exactly one `Option` check (the hot path is the ballistics march and `receive_fire_events`;
/// see the module doc). A no-op when the recorder is absent.
///
/// The row's fields arrive as a CLOSURE, not a built `Value`: the `json!` at each call site is then
/// evaluated only in an armed run, so an unrecorded run allocates nothing — the "zero cost when off"
/// claim is structural, not a promise about an optimizer.
pub(crate) fn record(
    trace: &mut Option<ResMut<ShotTrace>>,
    kind: &'static str,
    tick: u32,
    shot: ShotId,
    extra: impl FnOnce() -> Value,
) {
    if let Some(trace) = trace {
        trace.row(kind, tick, shot, extra());
    }
}

/// Open the role-qualified sink and register the `meta` row — only when `SPIKE_SHOT_TRACE` is set.
/// Returns `true` iff armed, mirroring [`crate::cost::install`]'s contract.
fn install(app: &mut App, role: &'static str) -> bool {
    let Ok(path) = std::env::var("SPIKE_SHOT_TRACE") else {
        return false;
    };
    let resolved = role_path(&path, role);
    let sink = match JsonlSink::create(&resolved) {
        Ok(sink) => sink,
        Err(err) => {
            error!("shot_trace: cannot open {}: {err}", resolved.display());
            return false;
        }
    };
    info!(
        "shot_trace: recording {role} shot-lifecycle rows to {}",
        resolved.display()
    );
    app.insert_resource(ShotTrace { sink, role });
    // The `meta` row is written from Startup so `tick_hz` reads the app's ACTUAL `Time<Fixed>`
    // timestep (not a hardcoded 64), exactly as `crate::trace`/`crate::cost` do.
    app.add_systems(Startup, write_meta);
    true
}

/// The `meta` row: the role, the tick rate, and the three CONFIGURED windows the analyzer measures
/// the observed distributions against — so a trace recorded before a constant is re-tuned still
/// reports against the constant it actually ran with.
fn write_meta(mut trace: ResMut<ShotTrace>, fixed: Res<Time<Fixed>>) {
    let role = trace.role;
    let tick_hz = (1.0 / fixed.timestep().as_secs_f64()).round() as u64;
    let row = json!({
        "k": "meta",
        "role": role,
        "tick_hz": tick_hz,
        "ver": env!("CARGO_PKG_VERSION"),
        "hold_ticks": RICOCHET_HOLD_TICKS,
        "overdue_ticks": OVERDUE_MARGIN_TICKS,
        "max_age_secs": SanctionedShots::MAX_AGE_SECS,
    });
    trace.sink.write(&row);
}

/// MP client: the receiving/consuming half of a shot's life (fire/keyframe/confirm arrivals, the
/// cosmetic shell's spawn → contact → hold → resolution → end).
pub fn client_plugin(app: &mut App) {
    install(app, "client");
}

/// MP server: the authority's emissions (fire broadcast, ricochet keyframe, impact confirm).
pub fn server_plugin(app: &mut App) {
    install(app, "server");
}
