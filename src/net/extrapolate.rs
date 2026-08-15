//! The interpolation buffer's EDGE: starvation instruments (always on) and the bounded
//! extrapolation gap-filler (default ON; `OVERMATCH_EXTRAPOLATE=0` opts out).
//!
//! When the interpolation cursor overruns the newest confirmed snapshot, lightyear clamps the
//! sample to that newest value (`lightyear_interpolation` `registry.rs`, `fraction.clamp(0.0,
//! 1.0)`) — freeze, then step, with no counter or event anywhere on the path. This module makes
//! that edge observable and, behind the lever, lawful:
//!
//! - **Instruments (unconditional, read-only).** Per interpolated hull, every frame the cursor
//!   sits at/past its newest confirmed `Position` sample is a starved frame; a maximal run of them
//!   is one gap. Gaps are counted with their durations (ticks), `SyncEvent` occurrences on the
//!   interpolation timeline are counted (steady-state must be zero after the handshake resync),
//!   and each closed gap is checked against a ledger of recent impulse-class authority ticks
//!   (fire announcements, damage confirms, `HullShock` episodes) for gap∧impulse coincidence.
//!   A summary line logs every [`SUMMARY_PERIOD_SECS`], on disconnect, and at app exit.
//!
//! - **Gap-filler (default ON; `OVERMATCH_EXTRAPOLATE=0` opts out).** Instead of the clamp, hull
//!   kinematics are
//!   projected at constant velocity from the newest confirmed `Position`/`Rotation` using the
//!   live replicated `LinearVelocity`/`AngularVelocity`, up to the derived horizon; past the
//!   horizon the HORIZON POSE HOLDS — the presentation never reverts to the clamp mid-gap (the
//!   ε bound is only honest inside the horizon, so the projection stops advancing there, but a
//!   backward snap is a worse pose than the held one by the same bound). EVERY gap close —
//!   overrun closes included — folds the residual by projective velocity blending: the old
//!   projection continues (held at the horizon if it was held) and blends into the lawful sample
//!   over the derived window below — never a pose snap. Scope is replicated hull spatial state
//!   only (`NetTank` + `Interpolated` roots); discrete events, gates, and belt state are never
//!   extrapolated.
//!
//! # DERIVATIONS
//!
//! ```text
//! a_max   = μ·g                      μ = track::sim::MU (isotropic surface friction),
//!                                    g = track::derive::G — hulls are traction-limited, so μg
//!                                    bounds hull acceleration and ½·a_max·t² bounds the
//!                                    constant-velocity projection error for a gap of t seconds.
//! ε_vis   = 12.08 mm                 the certified residual bar: the recoil microsim's
//!                                    mid-bounce residual accepted as "real trajectory
//!                                    difference" (commit 3bca010, wave-B certification).
//! g*      = sqrt(2·ε_vis / a_max)    the largest gap whose worst-case projection error stays
//!                                    under ε_vis (≈ 52 ms at μ = 0.9). Beyond g* the bound is
//!                                    not honest, so the clamp presents instead.
//! interval = one tick                the server replicates every tick (send_interval = 0, the
//!                                    degenerate `tests/net_interp_delay.rs` pins).
//! v_ref   = max(|v|, ε_vis/interval) the fold-rate ceiling: the stream's own speed, floored at
//!                                    the 0.77 m/s rate that folding ε_vis over one interval
//!                                    already rides (under the stream's travel at any speed
//!                                    above it).
//! w       = clamp(residual/v_ref,    the blend window: folding at v_ref, the residual takes
//!            1, ⌈g*/interval⌉        residual/v_ref — so the per-frame correction never
//!            intervals)              exceeds the stream's own per-frame travel (the ε-bar law
//!                                    the blend test pins). The cap is the horizon in whole
//!                                    intervals (4): the blend is presentation off the lawful
//!                                    sample, so its duration budget is the same certified
//!                                    horizon; a residual larger than v_ref·cap (a gap far
//!                                    beyond the certified burst class) folds proportionally
//!                                    faster, still never in one frame.
//! ```
//!
//! # SEAM
//!
//! Neither the vendored-crate patch nor `custom_interpolation_logic` re-registration:
//! `interpolate::<C>` rewrites the live component from the confirmed history every `Update`
//! frame, so a system ordered after `InterpolationSystems::Interpolate` sees the lawful (clamped)
//! sample and can overwrite `Position`/`Rotation` before Avian's `PostUpdate` transform sync
//! consumes them. The lightyear crate is untouched; its clamp default stays pinned by its own
//! `registry.rs` test.
//!
//! Research basis: `.agents/scratch/adaptive-cursor-frontier-2026-08-15.md` §1 and §4.

use std::collections::VecDeque;

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::app::AppExit;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::core::tick::{Tick, TickDuration};
use lightyear::core::timeline::SyncEvent;
use lightyear::interpolation::plugin::InterpolationSystems;
use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
use lightyear::prelude::{Connected, Interpolated, IsSynced, NetworkTimeline};

use super::protocol::NetTank;
use crate::ballistics::HullShock;

/// The certified visibility budget for a projected pose, in metres — the recoil microsim's
/// mid-bounce residual (12.08 mm) ratified as a real-trajectory difference, not an error
/// (commit 3bca010). An extrapolation whose worst-case error stays under this bar inherits an
/// already-ratified standard.
const EPSILON_VIS_M: f32 = 0.012_08;

/// Summary cadence, from the instrument brief (a normal client run must surface the counters at
/// info level roughly twice a minute). Diagnostics pacing only — no behavior reads it.
const SUMMARY_PERIOD_SECS: f32 = 30.0;

/// The traction ceiling: hulls are friction-limited, so no honest trajectory accelerates faster.
fn a_max() -> f32 {
    crate::track::sim::MU * crate::track::derive::G
}

/// The extrapolation horizon `g* = sqrt(2·ε_vis/a_max)`: the longest gap whose worst-case
/// constant-velocity projection error `½·a_max·t²` stays under [`EPSILON_VIS_M`].
fn horizon_secs() -> f32 {
    (2.0 * EPSILON_VIS_M / a_max()).sqrt()
}

/// Gap-filler writes armed, default ON. `OVERMATCH_EXTRAPOLATE=0` opts out, leaving this module
/// instruments only with every presented pose bit-identical to the clamp.
#[derive(Resource, Debug)]
pub(super) struct ExtrapolateHulls;

/// Read the lever once, mount the instruments unconditionally.
pub(super) fn install(app: &mut App) {
    if super::harness::env_flag("OVERMATCH_EXTRAPOLATE", true) {
        info!(
            "net: hull extrapolation ON (default) — horizon {:.1} ms = \
             sqrt(2·ε_vis/μg) (ε_vis {:.2} mm, μg {:.2} m/s²), hold at the horizon, \
             blend w = clamp(residual/v_ref, 1..4 send intervals) [OVERMATCH_EXTRAPOLATE=0 \
             opts out]",
            horizon_secs() * 1000.0,
            EPSILON_VIS_M * 1000.0,
            a_max(),
        );
        app.insert_resource(ExtrapolateHulls);
    } else {
        info!("net: hull extrapolation OFF [OVERMATCH_EXTRAPOLATE=0] — buffer-edge clamp");
    }
    app.init_resource::<ImpulseTicks>();
    app.init_resource::<FrontierDiag>();
    app.init_resource::<HullEdges>();
    app.add_observer(count_interp_sync_events);
    app.add_observer(log_summary_on_disconnect);
    app.add_systems(
        Update,
        (
            record_hull_shock_impulses,
            // After the lawful sample is written, before PostUpdate consumes the components.
            drive_hull_edges.after(InterpolationSystems::Interpolate),
            log_periodic_summary,
        ),
    );
    app.add_systems(Last, log_summary_on_exit);
}

// --- Impulse ledger --------------------------------------------------------------------------

/// Which class of authority fact stamped an impulse tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImpulseClass {
    /// A fire announcement (`FireEvent::fire_tick`) — the shooter's hull surges on this tick.
    Fire,
    /// An owner-private damage confirm (`DamageConfirm::damage_tick`) — the victim's hull takes
    /// the shot's impulse on this tick.
    Damage,
    /// A replicated [`HullShock`] episode boundary — the authority applied an impulse here.
    Shock,
}

/// Ring of recent impulse-class authority ticks, consulted when a starvation gap closes.
///
/// CAP derivation mirrors `CursorQueue::CAP`: 256 exceeds the synchronized 30-tank volley and the
/// coincidence check only needs ticks young enough to fall inside a live gap (gaps are bounded by
/// the interpolation delay). A memory bound only.
#[derive(Resource, Default, Debug)]
pub(super) struct ImpulseTicks {
    ring: VecDeque<(Tick, ImpulseClass)>,
}

impl ImpulseTicks {
    const CAP: usize = 256;

    pub(super) fn record(&mut self, tick: Tick, class: ImpulseClass) {
        if self.ring.iter().any(|entry| *entry == (tick, class)) {
            return;
        }
        self.ring.push_back((tick, class));
        if self.ring.len() > Self::CAP {
            self.ring.pop_front();
        }
    }

    /// Whether any recorded impulse tick lies in the half-open gap span `(floor, ceil]` —
    /// lightyear's own wrapping tick difference, so the span survives the `u32::MAX` boundary.
    fn any_within(&self, floor: Tick, ceil: Tick) -> bool {
        self.ring
            .iter()
            .any(|(tick, _)| *tick - floor > 0 && *tick - ceil <= 0)
    }
}

/// Stamp `HullShock` episode boundaries into the ledger as they replicate in. `[opened, tick]` is
/// the episode's own impulse span; both ends are recorded (interior ticks are unknowable from the
/// wire and the episode window is ≤ `SHOCK_EPISODE_TICKS`).
fn record_hull_shock_impulses(
    shocks: Query<&HullShock, (With<NetTank>, Changed<HullShock>)>,
    mut impulses: ResMut<ImpulseTicks>,
) {
    for shock in &shocks {
        if shock.count == 0 {
            // The spawn-bundle default: no episode has ever been applied.
            continue;
        }
        impulses.record(Tick(shock.opened), ImpulseClass::Shock);
        if shock.tick != shock.opened {
            impulses.record(Tick(shock.tick), ImpulseClass::Shock);
        }
    }
}

// --- Diagnostics -----------------------------------------------------------------------------

/// The frontier counters. All hull-gap counts are per hull per episode (three hulls starving on
/// one shared stall count three gaps — the unit every line names).
#[derive(Resource, Default, Debug)]
pub(super) struct FrontierDiag {
    /// Closed gap episodes.
    gaps: u64,
    /// Gaps with at least one impulse-class tick inside their span.
    coincident: u64,
    /// Closed gaps that presented extrapolated poses (the lever was armed).
    extrapolated: u64,
    /// Closed gaps that exceeded the horizon (the horizon pose held for the excess).
    beyond_horizon: u64,
    /// Frames on which at least one hull was starved.
    starved_frames: u64,
    /// Sum of closed-gap durations, in ticks (each gap's maximum cursor overrun).
    gap_ticks_total: f64,
    /// The single longest closed gap, in ticks.
    gap_ticks_max: f64,
    /// Blend-back residual statistics, metres.
    blend_count: u64,
    blend_residual_sum_m: f64,
    blend_residual_max_m: f32,
    /// `SyncEvent<InterpolationConfig>` count. The handshake resync emits exactly one; every
    /// event past the first is a steady-state resync and must not happen.
    sync_events: u64,
}

impl FrontierDiag {
    fn steady_sync_events(&self) -> u64 {
        self.sync_events.saturating_sub(1)
    }

    fn summary(&self, open_gaps: usize) -> String {
        let mean_blend_mm = if self.blend_count > 0 {
            self.blend_residual_sum_m * 1000.0 / self.blend_count as f64
        } else {
            0.0
        };
        format!(
            "hull-gaps={} (coincident={} extrapolated={} beyond_horizon={} open={}) \
             starved_frames={} gap_ticks(total={:.1} max={:.2}) \
             blend_residual(n={} mean={:.2}mm max={:.2}mm) sync_events={} (steady={})",
            self.gaps,
            self.coincident,
            self.extrapolated,
            self.beyond_horizon,
            open_gaps,
            self.starved_frames,
            self.gap_ticks_total,
            self.gap_ticks_max,
            self.blend_count,
            mean_blend_mm,
            self.blend_residual_max_m * 1000.0,
            self.sync_events,
            self.steady_sync_events(),
        )
    }
}

/// Count interpolation-timeline resyncs. Steady state after the handshake must be zero — a
/// nonzero steady count is every remote hull jumping, and the periodic summary flags it.
fn count_interp_sync_events(
    _event: On<SyncEvent<InterpolationConfig>>,
    mut diag: ResMut<FrontierDiag>,
) {
    diag.sync_events += 1;
    if diag.steady_sync_events() > 0 {
        warn!(
            "net: FRONTIER interpolation SyncEvent #{} — a steady-state resync snapped every \
             remote hull",
            diag.sync_events
        );
    }
}

/// The estimator and own-fire digests riding the FRONTIER line — each absent in worlds that never
/// mounted its module (unit fixtures, the server).
fn sibling_digests(
    arrival: &Option<Res<super::sync_margin::ArrivalDelay>>,
    own_fire: &Option<Res<super::fire_presentation::OwnFireDiag>>,
) -> String {
    let mut digest = String::new();
    if let Some(estimator) = arrival {
        digest.push(' ');
        digest.push_str(&estimator.describe());
    }
    if let Some(own_fire) = own_fire {
        digest.push(' ');
        digest.push_str(&own_fire.describe());
    }
    digest
}

fn log_periodic_summary(
    time: Res<Time<Real>>,
    diag: Res<FrontierDiag>,
    edges: Res<HullEdges>,
    arrival: Option<Res<super::sync_margin::ArrivalDelay>>,
    own_fire: Option<Res<super::fire_presentation::OwnFireDiag>>,
    mut elapsed: Local<f32>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < SUMMARY_PERIOD_SECS {
        return;
    }
    *elapsed = 0.0;
    info!(
        "net: FRONTIER {}{}",
        diag.summary(edges.open_gaps()),
        sibling_digests(&arrival, &own_fire)
    );
}

fn log_summary_on_disconnect(
    _disconnected: On<Remove, Connected>,
    diag: Res<FrontierDiag>,
    edges: Res<HullEdges>,
    arrival: Option<Res<super::sync_margin::ArrivalDelay>>,
    own_fire: Option<Res<super::fire_presentation::OwnFireDiag>>,
) {
    info!(
        "net: FRONTIER (disconnect) {}{}",
        diag.summary(edges.open_gaps()),
        sibling_digests(&arrival, &own_fire)
    );
}

fn log_summary_on_exit(
    mut exits: MessageReader<AppExit>,
    diag: Res<FrontierDiag>,
    edges: Res<HullEdges>,
    arrival: Option<Res<super::sync_margin::ArrivalDelay>>,
    own_fire: Option<Res<super::fire_presentation::OwnFireDiag>>,
    mut logged: Local<bool>,
) {
    if *logged || exits.read().next().is_none() {
        return;
    }
    *logged = true;
    info!(
        "net: FRONTIER (final) {}{}",
        diag.summary(edges.open_gaps()),
        sibling_digests(&arrival, &own_fire)
    );
}

// --- The edge state machine ------------------------------------------------------------------

/// The projection basis captured from the freshest lawful data: the newest confirmed pose and the
/// live replicated velocities at that moment.
#[derive(Clone, Copy, Debug)]
struct Basis {
    tick: Tick,
    pos: Vec3,
    rot: Quat,
    vel: Vec3,
    ang: Vec3,
}

/// Constant-velocity projection of the basis `dt` seconds forward.
fn project(basis: &Basis, dt: f32) -> (Vec3, Quat) {
    (
        basis.pos + basis.vel * dt,
        (Quat::from_scaled_axis(basis.ang * dt) * basis.rot).normalize(),
    )
}

#[derive(Debug, Default)]
enum Phase {
    /// The cursor is inside the buffer; the lawful interpolated sample presents.
    #[default]
    Tracking,
    /// The cursor is at/past the newest confirmed sample — lightyear's clamp is active.
    Starved {
        basis: Basis,
        /// Newest confirmed tick when the gap opened — the exclusive floor of the gap's span.
        floor: Tick,
        max_gap_ticks: f64,
        /// The gap exceeded the horizon at some point; the horizon pose held for the excess.
        overran: bool,
    },
    /// Fresh data ended an extrapolated gap; the blend's projection continues from the pose
    /// presented at close (`basis.pos`/`basis.rot`, held at the horizon when the gap overran)
    /// at the gap's stream velocities, blending into the lawful sample over `window_secs`.
    /// Against a same-velocity stream the residual is constant, so the per-frame correction is
    /// exactly residual/window ≤ v_ref.
    Blending {
        basis: Basis,
        start: (Tick, f64),
        window_secs: f32,
    },
}

/// One closed gap episode, for the diagnostics and the per-gap log line.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ClosedGap {
    /// Exclusive floor of the gap span (newest confirmed tick at open).
    floor: Tick,
    /// Inclusive ceiling (cursor tick at close).
    ceil: Tick,
    max_gap_ticks: f64,
    extrapolated: bool,
    overran: bool,
    /// Projection-vs-lawful distance at close, when a blend starts.
    blend_residual_m: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Presented {
    /// Leave the component as lightyear wrote it (the lawful sample; during starvation, the clamp).
    Lawful,
    /// Overwrite the hull pose.
    Pose(Vec3, Quat),
}

/// Everything `HullEdge::step` needs beyond its own state.
#[derive(Debug, Clone, Copy)]
struct EdgeParams {
    tick_secs: f32,
    horizon_secs: f32,
    /// One send interval (one tick — the server replicates per tick): the blend window's unit.
    interval_secs: f32,
    /// The lever: false = instruments only, every verdict is `Lawful`.
    extrapolate: bool,
}

/// The blend-window law (module doc DERIVATIONS): `w = clamp(residual/v_ref, 1 interval,
/// ⌈g*/interval⌉ intervals)`, `v_ref = max(stream speed, ε_vis/interval)`.
fn blend_window_secs(params: &EdgeParams, residual: f32, stream_speed: f32) -> f32 {
    let v_ref = stream_speed.max(EPSILON_VIS_M / params.interval_secs);
    let cap = (params.horizon_secs / params.interval_secs).ceil() * params.interval_secs;
    (residual / v_ref).clamp(params.interval_secs, cap)
}

/// Per-hull edge tracker.
#[derive(Debug, Default)]
struct HullEdge {
    phase: Phase,
    /// Mark-and-sweep stamp; entries missing a frame belong to despawned hulls.
    stamp: u64,
}

impl HullEdge {
    /// One frame of the edge law. `cursor` is the fractional interpolation cursor, `newest` the
    /// newest confirmed hull sample, `vel`/`ang` the live replicated velocities, `lawful` the pose
    /// lightyear just wrote (during starvation: the clamp).
    fn step(
        &mut self,
        params: &EdgeParams,
        cursor: (Tick, f64),
        newest: (Tick, Vec3, Quat),
        vel: Vec3,
        ang: Vec3,
        lawful: (Vec3, Quat),
    ) -> (Presented, Option<ClosedGap>) {
        // Strictly past the newest sample: the fraction clamps and the presented pose freezes.
        // AT the sample (gap = 0) the sample is exact — not starvation.
        let gap_ticks = f64::from(cursor.0 - newest.0) + cursor.1;
        let starving = gap_ticks > 0.0;
        let fresh_basis = || Basis {
            tick: newest.0,
            pos: newest.1,
            rot: newest.2,
            vel,
            ang,
        };
        match std::mem::take(&mut self.phase) {
            Phase::Tracking => {
                if !starving {
                    return (Presented::Lawful, None);
                }
                let basis = fresh_basis();
                let (presented, overran) = starved_pose(params, gap_ticks, &basis);
                self.phase = Phase::Starved {
                    basis,
                    floor: newest.0,
                    max_gap_ticks: gap_ticks,
                    overran,
                };
                (presented, None)
            }
            Phase::Starved {
                mut basis,
                floor,
                max_gap_ticks,
                overran,
            } => {
                if starving {
                    if basis.tick != newest.0 {
                        // Partial catch-up: data arrived but the cursor is still ahead. Rebase the
                        // projection on the freshest lawful state; the ε bound is honest again
                        // (`overran` keeps the episode's diagnostic truth).
                        basis = fresh_basis();
                    }
                    let (presented, overran_now) = starved_pose(params, gap_ticks, &basis);
                    self.phase = Phase::Starved {
                        basis,
                        floor,
                        max_gap_ticks: max_gap_ticks.max(gap_ticks),
                        overran: overran || overran_now,
                    };
                    return (presented, None);
                }
                let closed = ClosedGap {
                    floor,
                    ceil: cursor.0,
                    max_gap_ticks,
                    extrapolated: params.extrapolate,
                    overran,
                    blend_residual_m: None,
                };
                if params.extrapolate {
                    // Projective velocity blending, on EVERY close: the close frame (α = 0)
                    // presents the pose the last starved frame showed (held at the horizon when
                    // the gap overran) — continuity, never a snap — and the blend's projection
                    // continues from it at the gap's stream velocities.
                    let age =
                        projection_age_secs(params, cursor, basis.tick).min(params.horizon_secs);
                    let (proj_pos, proj_rot) = project(&basis, age);
                    let residual = proj_pos.distance(lawful.0);
                    let window_secs = blend_window_secs(params, residual, vel.length());
                    self.phase = Phase::Blending {
                        basis: Basis {
                            tick: cursor.0,
                            pos: proj_pos,
                            rot: proj_rot,
                            vel: basis.vel,
                            ang: basis.ang,
                        },
                        start: cursor,
                        window_secs,
                    };
                    (
                        Presented::Pose(proj_pos, proj_rot),
                        Some(ClosedGap {
                            blend_residual_m: Some(residual),
                            ..closed
                        }),
                    )
                } else {
                    self.phase = Phase::Tracking;
                    (Presented::Lawful, Some(closed))
                }
            }
            Phase::Blending {
                basis,
                start,
                window_secs,
            } => {
                if starving {
                    // Re-starved mid-blend: open a fresh gap from the freshest lawful state. The
                    // discontinuity is the residual's unfolded fraction.
                    let basis = fresh_basis();
                    let (presented, overran) = starved_pose(params, gap_ticks, &basis);
                    self.phase = Phase::Starved {
                        basis,
                        floor: newest.0,
                        max_gap_ticks: gap_ticks,
                        overran,
                    };
                    return (presented, None);
                }
                let elapsed_ticks = f64::from(cursor.0 - start.0) + (cursor.1 - start.1);
                let elapsed_secs = elapsed_ticks as f32 * params.tick_secs;
                let alpha = (elapsed_secs / window_secs).clamp(0.0, 1.0);
                if alpha >= 1.0 {
                    self.phase = Phase::Tracking;
                    return (Presented::Lawful, None);
                }
                // `basis` is anchored at the close (`start`): the projection age is the blend's
                // own elapsed time.
                let (proj_pos, proj_rot) = project(&basis, elapsed_secs);
                self.phase = Phase::Blending {
                    basis,
                    start,
                    window_secs,
                };
                (
                    Presented::Pose(
                        proj_pos.lerp(lawful.0, alpha),
                        proj_rot.slerp(lawful.1, alpha).normalize(),
                    ),
                    None,
                )
            }
        }
    }
}

fn projection_age_secs(params: &EdgeParams, cursor: (Tick, f64), basis_tick: Tick) -> f32 {
    (f64::from(cursor.0 - basis_tick) + cursor.1) as f32 * params.tick_secs
}

/// The starved-frame verdict: extrapolate inside the horizon, HOLD the horizon pose beyond it
/// (and report the overrun — the presentation never reverts to the clamp mid-gap), clamp always
/// with the lever unset.
fn starved_pose(params: &EdgeParams, gap_ticks: f64, basis: &Basis) -> (Presented, bool) {
    if !params.extrapolate {
        return (Presented::Lawful, false);
    }
    let dt = gap_ticks as f32 * params.tick_secs;
    let (pos, rot) = project(basis, dt.min(params.horizon_secs));
    (Presented::Pose(pos, rot), dt > params.horizon_secs)
}

/// Per-hull [`HullEdge`] states, resource-keyed so replicated entities never change archetype.
#[derive(Resource, Default, Debug)]
struct HullEdges {
    map: HashMap<Entity, HullEdge>,
    frame: u64,
}

impl HullEdges {
    fn open_gaps(&self) -> usize {
        self.map
            .values()
            .filter(|edge| matches!(edge.phase, Phase::Starved { .. }))
            .count()
    }
}

/// The edge driver: detect starvation on every interpolated hull (instrument), and under the
/// lever replace the clamp with the bounded projection / blend-back.
fn drive_hull_edges(
    lever: Option<Res<ExtrapolateHulls>>,
    tick: Res<TickDuration>,
    cursors: Query<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    mut hulls: Query<
        (
            Entity,
            &ConfirmedHistory<Position>,
            &ConfirmedHistory<Rotation>,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
            &mut Position,
            &mut Rotation,
        ),
        (With<NetTank>, With<Interpolated>),
    >,
    mut edges: ResMut<HullEdges>,
    impulses: Res<ImpulseTicks>,
    mut diag: ResMut<FrontierDiag>,
) {
    let Ok(timeline) = cursors.single() else {
        // No synced cursor: no starvation is defined, and stale trackers must not carry a gap
        // across a resync.
        edges.map.clear();
        return;
    };
    let cursor = (timeline.tick(), f64::from(timeline.overstep().to_f32()));
    let tick_secs = tick.0.as_secs_f32();
    let params = EdgeParams {
        tick_secs,
        horizon_secs: horizon_secs(),
        interval_secs: tick_secs,
        extrapolate: lever.is_some(),
    };
    edges.frame += 1;
    let frame = edges.frame;
    let mut starved_this_frame = false;
    for (entity, pos_history, rot_history, vel, ang, mut position, mut rotation) in &mut hulls {
        let Some((newest_tick, newest_pos)) = pos_history
            .newest_present()
            .map(|(tick, value)| (tick, value.0))
        else {
            continue;
        };
        // Rotation history advances in lockstep with Position (both ride the same completeness
        // signal); the at-or-before lookup covers a same-frame skew.
        let newest_rot = rot_history
            .get_present(newest_tick)
            .map_or(rotation.0, |value| value.0);
        let edge = edges.map.entry(entity).or_default();
        edge.stamp = frame;
        if matches!(edge.phase, Phase::Starved { .. })
            || f64::from(cursor.0 - newest_tick) + cursor.1 > 0.0
        {
            starved_this_frame = true;
        }
        let (presented, closed) = edge.step(
            &params,
            cursor,
            (newest_tick, newest_pos, newest_rot),
            vel.map_or(Vec3::ZERO, |v| v.0),
            ang.map_or(Vec3::ZERO, |v| v.0),
            (position.0, rotation.0),
        );
        if let Presented::Pose(pos, rot) = presented {
            position.0 = pos;
            rotation.0 = rot;
        }
        if let Some(gap) = closed {
            let coincident = impulses.any_within(gap.floor, gap.ceil);
            diag.gaps += 1;
            diag.gap_ticks_total += gap.max_gap_ticks;
            diag.gap_ticks_max = diag.gap_ticks_max.max(gap.max_gap_ticks);
            if coincident {
                diag.coincident += 1;
            }
            if gap.extrapolated {
                diag.extrapolated += 1;
            }
            if gap.overran {
                diag.beyond_horizon += 1;
            }
            if let Some(residual) = gap.blend_residual_m {
                diag.blend_count += 1;
                diag.blend_residual_sum_m += f64::from(residual);
                diag.blend_residual_max_m = diag.blend_residual_max_m.max(residual);
            }
            info!(
                "net: FRONTIER gap closed {entity} span=({},{}] gap={:.2}t extrapolated={} \
                 overran={} impulse={} blend_residual={}",
                gap.floor.0,
                gap.ceil.0,
                gap.max_gap_ticks,
                gap.extrapolated,
                gap.overran,
                coincident,
                gap.blend_residual_m
                    .map_or_else(|| "-".into(), |r| format!("{:.2}mm", r * 1000.0)),
            );
        }
    }
    if starved_this_frame {
        diag.starved_frames += 1;
    }
    // Sweep despawned hulls so an orphaned tracker cannot hold an open gap forever.
    edges.map.retain(|_, edge| edge.stamp == frame);
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::core::time::TickInstant;

    use super::*;

    /// The game's fixed tick (64 Hz), matching `ClientPlugins { tick_duration }` in `net::client`.
    const TICK_SECS: f32 = 1.0 / 64.0;

    fn params(extrapolate: bool) -> EdgeParams {
        EdgeParams {
            tick_secs: TICK_SECS,
            horizon_secs: horizon_secs(),
            interval_secs: TICK_SECS,
            extrapolate,
        }
    }

    /// THE HORIZON IS THE ε-BOUND INVERTED, recomputed here from the raw constants (μ = 0.9,
    /// g = 9.81, ε_vis = 12.08 mm) independently of the production expression. Any structural
    /// change to `horizon_secs` (dropping the 2, the sqrt, or either constant) reds one of these.
    #[test]
    fn the_horizon_is_the_epsilon_bound_inverted() {
        let expected = (2.0_f32 * 0.012_08 / (0.9 * 9.81)).sqrt();
        assert!(
            (horizon_secs() - expected).abs() < 1e-6,
            "g* = sqrt(2·ε_vis/μg): derived {} s, expected {expected} s",
            horizon_secs()
        );
        // The bound closes exactly at the horizon: ½·a_max·g*² = ε_vis.
        let worst_error = 0.5 * a_max() * horizon_secs() * horizon_secs();
        assert!(
            (worst_error - EPSILON_VIS_M).abs() < 1e-6,
            "½·a_max·g*² must equal ε_vis (got {worst_error} m)"
        );
        // And the horizon covers real jitter gaps: between 3 and 4 ticks at 64 Hz.
        assert!(horizon_secs() > 3.0 * TICK_SECS && horizon_secs() < 4.0 * TICK_SECS);
    }

    fn basis_inputs() -> ((Tick, Vec3, Quat), Vec3, Vec3) {
        (
            (Tick(100), Vec3::new(10.0, 0.0, -4.0), Quat::IDENTITY),
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::new(0.0, 0.4, 0.0),
        )
    }

    /// A CRUISE GAP INSIDE THE HORIZON PRESENTS THE CONSTANT-VELOCITY PROJECTION — exactly, and
    /// therefore within ε(t) = ½·a_max·t² of any traction-limited truth. Breaking the projection
    /// (holding the clamp, wrong dt, velocity dropped) reds the exact-pose assert; widening the
    /// horizon gate is caught by `a_gap_beyond_the_horizon_holds_the_horizon_pose`.
    #[test]
    fn a_cruise_gap_inside_the_horizon_presents_the_constant_velocity_projection() {
        let (newest, vel, ang) = basis_inputs();
        let mut edge = HullEdge::default();
        let clamp = (newest.1, newest.2);
        // Two ticks of gap: 31.25 ms, inside g* ≈ 52 ms.
        let (presented, closed) =
            edge.step(&params(true), (Tick(102), 0.0), newest, vel, ang, clamp);
        assert_eq!(closed, None, "the gap is still open");
        let dt = 2.0 * TICK_SECS;
        let expected_pos = newest.1 + vel * dt;
        let expected_rot = (Quat::from_scaled_axis(ang * dt) * newest.2).normalize();
        let Presented::Pose(pos, rot) = presented else {
            panic!("inside the horizon the projection must present, not the clamp");
        };
        assert!(pos.distance(expected_pos) < 1e-6, "constant-velocity law");
        assert!(rot.angle_between(expected_rot) < 1e-6, "angular law");
        // The worst-case divergence from a traction-limited truth is under the certified bar.
        let worst = 0.5 * a_max() * dt * dt;
        assert!(worst <= EPSILON_VIS_M, "ε(t) inside the horizon holds");
    }

    /// A GAP BEYOND THE HORIZON HOLDS THE HORIZON POSE — the projection stops advancing at
    /// exactly g* and the presentation NEVER reverts to the clamp mid-gap. A revert to `Lawful`
    /// past the horizon reds the pose assert; a projection that keeps advancing past g* reds the
    /// frozen assert; dropping the overrun report reds the phase assert.
    #[test]
    fn a_gap_beyond_the_horizon_holds_the_horizon_pose() {
        let (newest, vel, ang) = basis_inputs();
        let mut edge = HullEdge::default();
        let clamp = (newest.1, newest.2);
        let basis = Basis {
            tick: newest.0,
            pos: newest.1,
            rot: newest.2,
            vel,
            ang,
        };
        let horizon_pose = project(&basis, horizon_secs());
        // 5 ticks = 78 ms > g* ≈ 52 ms.
        let (presented, _) = edge.step(&params(true), (Tick(105), 0.0), newest, vel, ang, clamp);
        let Presented::Pose(pos, rot) = presented else {
            panic!("past the horizon the held pose presents — never the clamp");
        };
        assert!(
            pos.distance(horizon_pose.0) < 1e-6,
            "the held pose is the projection at exactly g*",
        );
        assert!(rot.angle_between(horizon_pose.1) < 1e-6, "angular hold");
        // Deeper into the same gap the pose is FROZEN: no further advance, no revert.
        let (deeper, closed) = edge.step(&params(true), (Tick(106), 0.25), newest, vel, ang, clamp);
        assert_eq!(closed, None, "the gap is still open");
        assert_eq!(deeper, Presented::Pose(pos, rot), "the horizon pose holds");
        assert!(
            matches!(edge.phase, Phase::Starved { overran: true, .. }),
            "the overrun is reported",
        );
    }

    /// THE BOUNDARY FRAME IS NOT STARVED: a cursor exactly ON the newest sample reads the exact
    /// sample (no gap opens); the first fraction past it is starvation. Loosening the strict
    /// `> 0` comparison in either direction reds one half.
    #[test]
    fn the_boundary_frame_is_not_starved() {
        let (newest, vel, ang) = basis_inputs();
        let clamp = (newest.1, newest.2);
        let mut edge = HullEdge::default();
        let (presented, _) = edge.step(&params(true), (Tick(100), 0.0), newest, vel, ang, clamp);
        assert_eq!(presented, Presented::Lawful, "gap 0 is the exact sample");
        assert!(matches!(edge.phase, Phase::Tracking), "no episode opens");
        let (presented, _) = edge.step(&params(true), (Tick(100), 0.01), newest, vel, ang, clamp);
        assert!(
            matches!(presented, Presented::Pose(..)),
            "the first fraction past the sample is a starved frame",
        );
        assert!(matches!(edge.phase, Phase::Starved { .. }));
    }

    /// BLEND-BACK FOLDS THE RESIDUAL WITHOUT A SNAP. A ~2-tick gap at 5 m/s leaves the certified
    /// worst-case residual ½·a_max·t² (~4.9 mm here), laid PERPENDICULAR to the motion so the
    /// stream's travel and the blend's correction decouple exactly; frames at 240 Hz then must
    /// show (1) the close frame presents the projection (zero correction — continuity), (2) no
    /// frame's motion exceeds the stream's own travel, and (3) per-frame correction is exactly the
    /// blend law's residual·Δα rate — folding everything in one frame (a snap) reds (3).
    #[test]
    fn blend_back_folds_the_residual_without_a_snap() {
        let (newest, _, _) = basis_inputs();
        let vel = Vec3::new(5.0, 0.0, 0.0);
        let ang = Vec3::ZERO;
        let clamp = (newest.1, newest.2);
        let mut edge = HullEdge::default();
        let p = params(true);

        // The gap: the cursor walks 2 ticks past the newest sample in 240 Hz frames.
        let frame_ticks = 64.0 / 240.0;
        let mut cursor_f = 0.0_f64;
        let mut last_pose = clamp.0;
        while cursor_f < 2.0 {
            cursor_f += frame_ticks;
            let cursor = split_cursor(Tick(100), cursor_f);
            let (presented, _) = edge.step(&p, cursor, newest, vel, ang, clamp);
            if let Presented::Pose(pos, _) = presented {
                last_pose = pos;
            }
        }

        // Fresh data: the truth drifted the certified worst-case ½·a_max·t² off the projection,
        // laid on Z (perpendicular to the 5 m/s X motion), and cruises on at the same velocity.
        let gap_secs = (cursor_f as f32) * TICK_SECS;
        let residual = 0.5 * a_max() * gap_secs * gap_secs;
        let offset = Vec3::new(0.0, 0.0, residual);
        let lawful_at = |cursor_f: f64| newest.1 + vel * (cursor_f as f32 * TICK_SECS) + offset;
        // The catch-up burst lands samples well ahead of the cursor, so the blend runs to
        // completion without re-starving.
        let newest_now = (Tick(105), lawful_at(5.0), newest.2);

        let frame_secs = frame_ticks as f32 * TICK_SECS;
        let stream_travel = vel.length() * frame_secs;
        // The residual is sub-ε at cruise speed, so the window floors at one interval;
        // Δα per frame = frame/window and the correction per frame is exactly residual · Δα.
        let blend_rate = residual * (frame_secs / p.interval_secs);
        let mut first = true;
        let mut blended = 0;
        loop {
            // The next frame: the cursor advances, and the first of them finds the fresh data.
            cursor_f += frame_ticks;
            let cursor = split_cursor(Tick(100), cursor_f);
            let lawful_pos = lawful_at(cursor_f);
            let (presented, closed) =
                edge.step(&p, cursor, newest_now, vel, ang, (lawful_pos, newest.2));
            let pose = match presented {
                Presented::Pose(pos, _) => pos,
                Presented::Lawful => lawful_pos,
            };
            // The correction: frame motion beyond the stream's uniform travel.
            let correction = (pose - last_pose - vel * frame_secs).length();
            if first {
                let gap = closed.expect("the gap closes on the first fresh frame");
                let reported = gap.blend_residual_m.expect("an extrapolated close blends");
                assert!(
                    (reported - residual).abs() < 1e-4,
                    "the close reports the projection residual ({reported} vs {residual})",
                );
                assert!(
                    correction < 1e-5,
                    "the close frame presents the projection itself — continuity, no snap \
                     (correction {correction})",
                );
                first = false;
            } else if matches!(presented, Presented::Pose(..)) {
                assert!(
                    (correction - blend_rate).abs() <= blend_rate * 0.05 + 1e-6,
                    "the correction must fold at the blend law's residual·Δα rate, never as a \
                     snap (correction {correction}, rate {blend_rate})",
                );
            } else {
                // The final fold to Lawful carries the last partial Δα — never more than a full
                // frame's rate.
                assert!(
                    correction <= blend_rate + 1e-6,
                    "the terminal fold stays under one frame's blend rate \
                     (correction {correction}, rate {blend_rate})",
                );
            }
            let step = pose.distance(last_pose);
            assert!(
                step <= (stream_travel * stream_travel + blend_rate * blend_rate).sqrt() + 1e-6,
                "no frame's motion may exceed the stream's own travel plus the lawful fold \
                 (step {step}, travel {stream_travel})",
            );
            last_pose = pose;
            blended += 1;
            if matches!(edge.phase, Phase::Tracking) {
                break;
            }
            assert!(
                blended < 16,
                "the blend must finish within one send interval"
            );
        }
        assert!(
            blended >= 3,
            "at 240 Hz a one-tick blend spans several frames (got {blended})",
        );
    }

    /// THE HORIZON CROSSING NEVER SNAPS BACKWARD: walking the cursor in 240 Hz frames from
    /// inside the horizon to well past it, every frame's pose displacement is non-negative along
    /// the stream velocity and never exceeds one frame of stream travel. The revert-to-clamp
    /// law (present the newest sample past g*) is a ~0.26 m backward snap at the crossing and
    /// reds the non-negative assert; an unclamped projection reds the travel bound only through
    /// the frozen-hold assert in `a_gap_beyond_the_horizon_holds_the_horizon_pose`, so the
    /// travel bound here doubles as the monotone ceiling.
    #[test]
    fn the_horizon_crossing_never_snaps_backward() {
        let (newest, vel, _) = basis_inputs();
        let ang = Vec3::ZERO;
        let clamp = (newest.1, newest.2);
        let mut edge = HullEdge::default();
        let p = params(true);
        let frame_ticks = 64.0 / 240.0;
        let frame_secs = frame_ticks as f32 * TICK_SECS;
        let dir = vel.normalize();
        let mut cursor_f = 0.0_f64;
        let mut last_pose = clamp.0;
        // 6 ticks = 94 ms: the walk crosses g* ≈ 52 ms mid-way.
        while cursor_f < 6.0 {
            cursor_f += frame_ticks;
            let cursor = split_cursor(Tick(100), cursor_f);
            let (presented, _) = edge.step(&p, cursor, newest, vel, ang, clamp);
            let pose = match presented {
                Presented::Pose(pos, _) => pos,
                Presented::Lawful => clamp.0,
            };
            let step = pose - last_pose;
            // Tolerances are float noise at the 10 m coordinate scale, far under the ~0.26 m
            // snap the revert law produces at the crossing.
            assert!(
                step.dot(dir) >= -1e-5,
                "the presentation never moves backward (step {} m at {:.2}t)",
                step.dot(dir),
                cursor_f,
            );
            assert!(
                step.length() <= vel.length() * frame_secs + 1e-5,
                "no frame outruns the stream's own travel (step {} m)",
                step.length(),
            );
            last_pose = pose;
        }
        // The walk did reach the held region.
        assert!(matches!(edge.phase, Phase::Starved { overran: true, .. }));
    }

    /// AN OVERRUN CLOSE FOLDS THE RESIDUAL THROUGH THE WIDENED WINDOW. A 6-tick gap at 5 m/s
    /// leaves the lawful stream ~0.23 m ahead of the held horizon pose; the close frame must
    /// present the held pose itself (a step-close — the pre-blend law — is a single-frame
    /// ~0.23 m snap and reds the continuity assert), the window must stretch to residual/v_ref
    /// (~2.9 send intervals — a fixed one-interval window folds ~3× too fast and reds the rate
    /// assert), and every blend frame's correction beyond the stream's own travel holds the
    /// v_ref fold rate.
    #[test]
    fn an_overrun_close_folds_the_residual_through_the_widened_window() {
        let (newest, _, _) = basis_inputs();
        let vel = Vec3::new(5.0, 0.0, 0.0);
        let ang = Vec3::ZERO;
        let clamp = (newest.1, newest.2);
        let mut edge = HullEdge::default();
        let p = params(true);

        // The gap: the cursor walks 6 ticks (94 ms > g* ≈ 52 ms) past the newest sample.
        let frame_ticks = 64.0 / 240.0;
        let mut cursor_f = 0.0_f64;
        let mut last_pose = clamp.0;
        while cursor_f < 6.0 {
            cursor_f += frame_ticks;
            let cursor = split_cursor(Tick(100), cursor_f);
            let (presented, _) = edge.step(&p, cursor, newest, vel, ang, clamp);
            if let Presented::Pose(pos, _) = presented {
                last_pose = pos;
            }
        }
        let held_pose = newest.1 + vel * p.horizon_secs;
        assert!(
            last_pose.distance(held_pose) < 1e-5,
            "precondition: the last starved frame held the horizon pose",
        );

        // Fresh data: the lawful stream never stalled — it is at newest.1 + vel·t, ahead of the
        // held pose by vel·(gap − g*). The catch-up burst lands samples far enough past the
        // cursor that the ~3-interval blend runs to completion without re-starving.
        let lawful_at = |cursor_f: f64| newest.1 + vel * (cursor_f as f32 * TICK_SECS);
        let newest_now = (Tick(112), lawful_at(12.0), newest.2);

        let frame_secs = frame_ticks as f32 * TICK_SECS;
        let stream_travel = vel.length() * frame_secs;
        let mut first = true;
        let mut expected_rate = 0.0_f32;
        let mut expected_window = 0.0_f32;
        let mut blended = 0;
        loop {
            cursor_f += frame_ticks;
            let cursor = split_cursor(Tick(100), cursor_f);
            let lawful_pos = lawful_at(cursor_f);
            let (presented, closed) =
                edge.step(&p, cursor, newest_now, vel, ang, (lawful_pos, newest.2));
            let pose = match presented {
                Presented::Pose(pos, _) => pos,
                Presented::Lawful => lawful_pos,
            };
            // The correction: frame motion beyond the stream's uniform travel.
            let correction = (pose - last_pose - vel * frame_secs).length();
            if first {
                let gap = closed.expect("the gap closes on the first fresh frame");
                assert!(gap.overran, "the episode overran the horizon");
                assert!(gap.extrapolated, "the lever was armed for the whole gap");
                let residual = gap.blend_residual_m.expect("an overrun close still blends");
                let expected_residual = (lawful_pos - held_pose).length();
                assert!(
                    (residual - expected_residual).abs() < 1e-4,
                    "the close reports the held-pose residual ({residual} vs {expected_residual})",
                );
                // Continuity: the close frame presents the held pose itself — the pose the last
                // starved frame showed, still frozen (a step-close jumps ~0.23 m to lawful).
                assert!(
                    pose.distance(last_pose) < 1e-5,
                    "the close frame presents the held pose — continuity, no snap \
                     (moved {} m)",
                    pose.distance(last_pose),
                );
                // The window law: residual/v_ref, between one interval and the horizon's ceiling.
                expected_window = blend_window_secs(&p, residual, vel.length());
                assert!(
                    expected_window > 2.0 * p.interval_secs
                        && expected_window < 4.0 * p.interval_secs,
                    "this residual must widen the window past two intervals \
                     (got {expected_window} s)",
                );
                expected_rate = residual * (frame_secs / expected_window);
                first = false;
            } else if matches!(presented, Presented::Pose(..)) {
                assert!(
                    (correction - expected_rate).abs() <= expected_rate * 0.05 + 1e-6,
                    "the correction folds at residual/window per frame — v_ref, never faster \
                     (correction {correction}, rate {expected_rate})",
                );
                // At this residual the rate sits exactly AT v_ref = the stream speed; the 5%
                // slack is float noise at the 10 m coordinate scale.
                assert!(
                    correction <= stream_travel * 1.05 + 1e-6,
                    "the fold never outruns the stream's own per-frame travel",
                );
            } else {
                assert!(
                    correction <= expected_rate * 1.05 + 1e-6,
                    "the terminal fold stays under one frame's rate (correction {correction})",
                );
            }
            last_pose = pose;
            blended += 1;
            if matches!(edge.phase, Phase::Tracking) {
                break;
            }
            assert!(blended < 64, "the blend must terminate");
        }
        // The blend's duration IS the window (one frame of slack at each edge).
        let duration = blended as f32 * frame_secs;
        assert!(
            (duration - expected_window).abs() <= 2.0 * frame_secs,
            "the fold spans the widened window ({duration} s vs {expected_window} s)",
        );
    }

    /// A PARTIAL CATCH-UP REBASES THE PROJECTION: data that arrives while the cursor is still
    /// ahead re-anchors the basis on the freshest lawful state instead of projecting stale data
    /// further. Keeping the old basis reds the exact-pose assert.
    #[test]
    fn a_partial_catch_up_rebases_the_projection() {
        let (newest, vel, ang) = basis_inputs();
        let clamp = (newest.1, newest.2);
        let mut edge = HullEdge::default();
        let p = params(true);
        edge.step(&p, (Tick(101), 0.5), newest, vel, ang, clamp);
        // One sample lands (tick 101) but the cursor is already at 102.5: still starving.
        let fresh = (Tick(101), newest.1 + vel * TICK_SECS, newest.2);
        let fresh_vel = Vec3::new(4.0, 0.0, 0.0);
        let (presented, closed) = edge.step(
            &p,
            (Tick(102), 0.5),
            fresh,
            fresh_vel,
            ang,
            (fresh.1, fresh.2),
        );
        assert_eq!(
            closed, None,
            "the gap stays open through a partial catch-up"
        );
        let dt = 1.5 * TICK_SECS;
        let Presented::Pose(pos, _) = presented else {
            panic!("still inside the horizon: the projection presents");
        };
        assert!(
            pos.distance(fresh.1 + fresh_vel * dt) < 1e-6,
            "the projection must continue from the fresh sample and fresh velocity",
        );
    }

    /// Split a fractional tick offset from `base` into lightyear's `(Tick, overstep)` shape.
    fn split_cursor(base: Tick, offset: f64) -> (Tick, f64) {
        let whole = offset.floor();
        (base + whole as i32, offset - whole)
    }

    /// IMPULSE∧GAP COINCIDENCE is membership of the half-open span `(floor, ceil]`, wrap-safe.
    /// Widening either end (inclusive floor, exclusive ceiling) reds an endpoint assert; a naive
    /// non-wrapping compare reds the boundary case.
    #[test]
    fn impulse_ticks_inside_a_gap_span_count_as_coincident() {
        let mut ledger = ImpulseTicks::default();
        ledger.record(Tick(102), ImpulseClass::Fire);
        assert!(ledger.any_within(Tick(100), Tick(103)), "interior tick");
        assert!(
            !ledger.any_within(Tick(102), Tick(105)),
            "floor is exclusive"
        );
        assert!(
            ledger.any_within(Tick(99), Tick(102)),
            "ceiling is inclusive"
        );
        assert!(!ledger.any_within(Tick(103), Tick(110)), "outside misses");
        // The span survives the u32 boundary.
        let mut wrapped = ImpulseTicks::default();
        wrapped.record(Tick(1), ImpulseClass::Damage);
        assert!(wrapped.any_within(Tick(u32::MAX - 1), Tick(2)));
        // Recording is idempotent per (tick, class).
        ledger.record(Tick(102), ImpulseClass::Fire);
        assert_eq!(ledger.ring.len(), 1, "a duplicate stamp records once");
    }

    fn edge_world(cursor: TickInstant) -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(TickDuration(core::time::Duration::from_secs_f64(
            1.0 / 64.0,
        )));
        world.init_resource::<ImpulseTicks>();
        world.init_resource::<FrontierDiag>();
        world.init_resource::<HullEdges>();
        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(cursor);
        world.spawn((timeline, IsSynced::<InterpolationTimeline>::default()));
        let mut pos_history = ConfirmedHistory::<Position>::default();
        pos_history.insert_present(Tick(100), Position(Vec3::new(10.0, 0.0, -4.0)));
        let mut rot_history = ConfirmedHistory::<Rotation>::default();
        rot_history.insert_present(Tick(100), Rotation(Quat::IDENTITY));
        let hull = world
            .spawn((
                NetTank,
                Interpolated,
                pos_history,
                rot_history,
                LinearVelocity(Vec3::new(5.0, 0.0, 0.0)),
                AngularVelocity(Vec3::ZERO),
                // What lightyear's clamp leaves during starvation: the newest confirmed pose.
                Position(Vec3::new(10.0, 0.0, -4.0)),
                Rotation(Quat::IDENTITY),
            ))
            .id();
        (world, hull)
    }

    /// LEVER UNSET = BIT-IDENTICAL TO BASE: a starving hull's pose is never written, while the
    /// instrument still counts the starved frame. The same world with the lever writes the
    /// projection — so an unconditional write path reds the first half, and a dead write path
    /// reds the second.
    #[test]
    fn without_the_lever_nothing_is_written_and_the_instrument_still_counts() {
        // Cursor two ticks past the newest sample (inside the horizon).
        let (mut world, hull) = edge_world(TickInstant::from(Tick(102)));
        world
            .run_system_once(drive_hull_edges)
            .expect("edge driver runs");
        let clamp = Vec3::new(10.0, 0.0, -4.0);
        assert_eq!(
            world.get::<Position>(hull).expect("hull position").0,
            clamp,
            "without the lever the clamp must present untouched",
        );
        assert_eq!(
            world.resource::<FrontierDiag>().starved_frames,
            1,
            "the instrument counts regardless of the lever",
        );

        world.insert_resource(ExtrapolateHulls);
        world
            .run_system_once(drive_hull_edges)
            .expect("edge driver runs");
        let expected = clamp + Vec3::new(5.0, 0.0, 0.0) * (2.0 * TICK_SECS);
        assert!(
            world
                .get::<Position>(hull)
                .expect("hull position")
                .0
                .distance(expected)
                < 1e-5,
            "with the lever the projection presents",
        );
    }

    /// THE STARVATION COUNTER SEES THE CLAMP AND THE GAP CLOSES WITH ITS DURATION. Fresh data
    /// (a newer confirmed sample behind the cursor) ends the episode; the closed gap carries the
    /// maximum overrun in ticks and feeds the counters. Breaking episode-close detection (never
    /// closing, or closing while still starved) reds the second half.
    #[test]
    fn a_starvation_episode_opens_at_the_clamp_and_closes_on_fresh_data() {
        let (mut world, hull) = edge_world(TickInstant::from(Tick(103)));
        world
            .run_system_once(drive_hull_edges)
            .expect("edge driver runs");
        {
            let edges = world.resource::<HullEdges>();
            assert_eq!(edges.open_gaps(), 1, "the clamp is a live, observable gap");
        }
        assert_eq!(world.resource::<FrontierDiag>().gaps, 0, "not closed yet");

        // Fresh data lands ahead of the cursor: history now reaches tick 105.
        let mut history = world
            .get_mut::<ConfirmedHistory<Position>>(hull)
            .expect("history");
        history.insert_present(Tick(105), Position(Vec3::new(11.0, 0.0, -4.0)));
        let mut rot = world
            .get_mut::<ConfirmedHistory<Rotation>>(hull)
            .expect("rot history");
        rot.insert_present(Tick(105), Rotation(Quat::IDENTITY));
        world
            .run_system_once(drive_hull_edges)
            .expect("edge driver runs");
        let diag = world.resource::<FrontierDiag>();
        assert_eq!(diag.gaps, 1, "the episode closed");
        assert!(
            (diag.gap_ticks_max - 3.0).abs() < 1e-9,
            "the gap's duration is the cursor's maximum overrun (3 ticks, got {})",
            diag.gap_ticks_max,
        );
        assert_eq!(world.resource::<HullEdges>().open_gaps(), 0);
    }

    /// SYNC EVENTS ARE COUNTED, and everything past the handshake's one is steady-state. An
    /// unwired observer reds the count; miscounting the handshake allowance reds the steady half.
    #[test]
    fn interpolation_sync_events_are_counted() {
        let mut app = App::new();
        app.init_resource::<FrontierDiag>();
        app.add_observer(count_interp_sync_events);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(SyncEvent::<InterpolationConfig>::new(entity, 3));
        app.world_mut()
            .trigger(SyncEvent::<InterpolationConfig>::new(entity, -2));
        let diag = app.world().resource::<FrontierDiag>();
        assert_eq!(diag.sync_events, 2);
        assert_eq!(
            diag.steady_sync_events(),
            1,
            "the handshake resync is the one allowed event",
        );
    }
}
