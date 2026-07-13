//! The suspension-force recorder: an env-gated, per-wheel/per-tick JSONL log of the exact force
//! decomposition `apply_suspension` and `apply_drive` feed `apply_force_at_point` — spring,
//! damper, bump-stop (with its impulse-cap state), the `max(0)` clip, and the drive/anchor force
//! with the contact velocity it acted on. The raw material for offline force/energy audits: it is
//! what decomposed the at-rest limit-cycle's energy pump down to sphere-cast TOI noise (see
//! [`crate::sphere_cast_ground_contact`]).
//!
//! A PASSIVE observer, like [`crate::trace`]: nothing here writes sim state. The whole module is
//! OFF unless `SUSP_TRACE=<path>` names a file at startup — an unset var costs one
//! `std::env::var` lookup on first use and a `OnceLock` read per call thereafter, zero writes
//! (both systems hoist that read to once per run, so the per-wheel hot loops pay nothing).
//! Unlike `SPIKE_TRACE` the path is used VERBATIM (no role suffix): give each process its own
//! path when tracing both wire ends from one shell. The sink itself is [`crate::trace`]'s
//! [`crate::trace::JsonlSink`] — same NaN-safe emission (a non-finite f32 serializes as `null`,
//! never invalid-JSON `NaN`/`inf`; this recorder targets exactly the corrupt regimes) and the
//! same ~1 s flush cadence, not a parallel implementation.
//!
//! ## Row schema (one compact JSON object per line, `k` = kind)
//! - `"s"` — per wheel, per `apply_suspension` run: `n` trace tick (the join key), `w` wheel
//!   slot, `c`/`cc` raw/clamped compression, `ss` spring speed, `fs`/`fd`/`fb` spring/damper/
//!   bump-stop force, `cap` whether the stop's impulse cap engaged, `clip` force removed by the
//!   `max(0)` floor, `ld` the applied load, `cy` contact y, `py` body y, `vy` hull vertical
//!   velocity, `wx`/`wz` hull angular-velocity x/z, `oy` probe-origin y, `gd` probed ground
//!   distance, `we` wheel entity bits.
//! - `"d"` — per anchored wheel, per `apply_drive` run: `n` the same tick's join key, `w` wheel
//!   slot, `vf`/`vl` contact velocity fore-aft/lateral, `ws` static-grip blend weight, `df`/`dl`
//!   anchor deflection fore-aft/lateral, `f` the applied force vector, `pow` the instantaneous
//!   power that force feeds the body (positive = energy in).
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use crate::trace::JsonlSink;

/// The shared-with-`crate::trace` sink (`Mutex` because the recorders are plain functions with
/// no `World` access, unlike `TraceWriter`'s resource): NaN-safe rows, ~1 s flush cadence.
static SINK: OnceLock<Option<Mutex<JsonlSink>>> = OnceLock::new();
/// Monotone per-`apply_suspension`-run counter — the join key between suspension and drive
/// rows of the same tick (`apply_drive` reads, never bumps).
static TICK: AtomicU64 = AtomicU64::new(0);

fn sink() -> Option<&'static Mutex<JsonlSink>> {
    SINK.get_or_init(|| {
        std::env::var("SUSP_TRACE").ok().map(|path| {
            Mutex::new(JsonlSink::create(Path::new(&path)).expect("SUSP_TRACE file creation"))
        })
    })
    .as_ref()
}

pub(super) fn enabled() -> bool {
    sink().is_some()
}

/// Bump the per-tick counter (once per `apply_suspension` run) and return it.
pub(super) fn next_tick() -> u64 {
    TICK.fetch_add(1, Ordering::Relaxed) + 1
}

pub(super) fn tick() -> u64 {
    TICK.load(Ordering::Relaxed)
}

pub(super) fn write(row: &Value) {
    if let Some(sink) = sink() {
        sink.lock().expect("SUSP_TRACE sink poisoned").write(row);
    }
}
