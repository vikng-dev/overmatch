//! Optional passive shot-lifecycle JSONL recorder (`SPIKE_SHOT_TRACE=<path>`).
//!
//! Rows join by the stable [`ShotId`] fields `(c, w, ft)`; role-qualified paths isolate concurrent
//! compositions. `send` records transport admission, not delivery acknowledgement. See
//! [`scripts/shot/analyze.py`](../../scripts/shot/analyze.py) for field-level checks.

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
            "c": shot.combatant.0,
            "w": shot.weapon,
            "ft": shot.fire_tick,
        });
        merge_extra(&mut row, extra);
        self.sink.write(&row);
    }

    /// Append a non-shot-scoped row such as one authority tick's transport accounting.
    pub(crate) fn global_row(&mut self, kind: &'static str, tick: u32, extra: Value) {
        let mut row = json!({ "k": kind, "t": tick });
        merge_extra(&mut row, extra);
        self.sink.write(&row);
    }

    #[cfg(test)]
    pub(crate) fn for_test(path: &std::path::Path) -> Self {
        Self {
            sink: JsonlSink::create(path).expect("test shot trace opens"),
            role: "client",
        }
    }
}

fn merge_extra(row: &mut Value, extra: Value) {
    let object = row.as_object_mut().expect("trace row is an object");
    if let Value::Object(fields) = extra {
        for (key, value) in fields {
            if object.contains_key(&key) {
                error!("shot_trace: refusing to overwrite reserved row field `{key}`");
                continue;
            }
            object.insert(key, value);
        }
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

/// Record one unscoped row through an optional recorder.
pub(crate) fn record_global(
    trace: &mut Option<ResMut<ShotTrace>>,
    kind: &'static str,
    tick: u32,
    extra: impl FnOnce() -> Value,
) {
    if let Some(trace) = trace {
        trace.global_row(kind, tick, extra());
    }
}

/// Open the role-qualified sink and register the `meta` row — only when `SPIKE_SHOT_TRACE` is set.
/// Returns `true` iff armed, mirroring [`crate::cost::install`]'s contract.
fn install(app: &mut App, role: &'static str) -> bool {
    let Some(path) = crate::env_value("SPIKE_SHOT_TRACE") else {
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

/// The `meta` row records the role, tick rate, and configured client outcome bounds used by the run.
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
        "max_bounces_per_shot": SanctionedShots::MAX_BOUNCES_PER_SHOT,
    });
    trace.sink.write(&row);
}

/// MP client: the receiving/consuming half of a public shot's life (fire/keyframe/terminal arrivals
/// and the cosmetic shell's spawn → catch-up/contact → hold → resolution → trail → end).
pub fn client_plugin(app: &mut App) {
    install(app, "client");
}

/// MP server: the authority's emissions (fire, ricochet, terminal, and damage confirmation).
pub fn server_plugin(app: &mut App) {
    install(app, "server");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_fields_cannot_overwrite_shot_identity() {
        let mut row = json!({ "k": "send", "t": 10, "c": 7, "w": 1, "ft": 9 });
        merge_extra(&mut row, json!({ "c": 99, "age": 3 }));

        assert_eq!(row["c"], 7);
        assert_eq!(row["age"], 3);
    }
}
