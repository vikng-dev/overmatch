//! The **kinematic wrap** track view (architecture §3) — the shared, promoted implementation the
//! game's `view` plugin and the sandbox both run.
//!
//! The belt path is *fitted* around the articulated running-gear circles every frame as a pure
//! function of pose, terrain, and belt phase: a taut convex wrap of the circles, a terrain conform
//! on a dense resample of the bottom run, and a budgeted sag on the top run — then the closed loop
//! resampled at the material link pitch. Nothing about the drawn track is simulated: wrong-side
//! capture, compression zigzag, teleport transients and solver divergence are unrepresentable
//! because there is no solver state to capture, buckle, stale, or diverge.
//!
//! Two "feel" tiers layer a self-healing FILTER over that stateless core, ALWAYS on: a hull-frame
//! temporal ease on the conform depth ([`ConformEase`]) and a 1-DOF spring on the sag budget
//! ([`SlackSpring`]). Both are parameter-free derived laws (a ballistic fall at gravity, a pendulum
//! frequency) — there are no dials and no toggles. They are pure VIEW state: cosmetic client-local
//! memory that never touches the sim. A remote tank's drawn belt is therefore not a pure function of
//! replicated pose + phase (it carries the client's own filter history) — which is fine, because it
//! is view-layer juice; the pins, wheels and gear spin still derive from replicated state alone.
//!
//! Pure math + small state types, no ECS and no assets — the same shape as [`super::wheels`]. The
//! caller passes the articulated circles, a terrain oracle, the hull affine, belt phase, material
//! loop length and the per-side lateral stations; owns one [`WrapState`] per side; and gets back the
//! drawn pin joints (and a taut reference loop for diagnostics).

use bevy::math::{Affine3A, Vec2, Vec3};

use super::forces::phase_decompose;
use super::oracle::TerrainOracle;
use super::route::{external_tangent, polyline_len, resample, sag_span, taut_lower_run};

/// Gravity (m/s²) for the wrap-feel BALLISTIC laws — the SAME value
/// [`wheel_lift_step`](super::wheels::wheel_lift_step) falls the view wheels at, so the belt memory
/// can never settle at a different rate than the wheels it spans. There are no per-tier time
/// constants: every fall is this one gravity, which makes the feel parameter-free and scale-correct
/// (a deep drop takes longer than a lip, by g·t² alone).
const WRAP_FEEL_G: f32 = 9.81;
/// Damping ratio for the slack spring — the project-canonical `ζ = 0.5` (the same value the static
/// grip bristle uses). Deliberately a global constant, NOT a per-tank dial: a free damping knob is
/// not a physical input.
const WRAP_FEEL_ZETA: f32 = 0.5;
/// Sag-depth floor (m) for the slack spring's DERIVED frequency `ω = √(g / sag_depth)` — a hanging
/// span's pendulum rate. That rate blows up as the span goes taut (sag → 0), so the depth is floored
/// here: a near-taut return run springs at a fast-but-finite rate instead of an infinitely stiff one.
const SLACK_SAG_FLOOR: f32 = 0.02;
/// Hull-frame z-grid cell size (m) for the belly-memory ease. Coarser than [`BELT_DRAW_SPACING`]
/// on purpose: every interior cell is then guaranteed at least one dense station each frame, so a
/// cell that receives no station simply HOLDS its value (it never falls toward a phantom zero).
const CONFORM_EASE_CELL: f32 = 0.15;
/// Arc-length spacing (m) of the dense bottom-run resample the terrain conform probes on: fine
/// enough that a bump between two wheels is sampled so the conform can raise the line onto it (a
/// tangent segment between wheels is one long edge — conforming only its endpoints would let a
/// board mid-segment go unsampled and the belt cut through it).
const BELT_DRAW_SPACING: f32 = 0.1;
/// A station carries z-keyed belly memory only if its hull-frame outward normal points genuinely
/// DOWN — `out.y < −this`. The belly bottom run is the only z-MONOTONE span; the sprocket/idler end
/// arcs fold z back on themselves, so a lower-branch board depth keyed by z would otherwise leak
/// onto the upper branch (a different normal, a different budget). Excluding the arcs' sideways/up
/// faces (`out.y ≥ −0.2`) keeps memory on the monotone belly; the arcs pass their raw depth through.
const GROUND_FACING_EPS: f32 = 0.2;

/// Per-side filter state (the two feel tiers). The caller owns one per side and keeps it across
/// frames; it is pure cosmetic memory, reseedable from data at any instant. On a teleport / respawn
/// / snap-correction, [`Self::reset`] drops the memory so it re-inits from the current frame's raw
/// targets instead of settling in from a stale pose over one fall period.
#[derive(Default)]
pub struct WrapState {
    ease: ConformEase,
    spring: SlackSpring,
}

impl WrapState {
    /// Drop all filter memory — the next [`step`] re-inits from the current frame's raw targets.
    pub fn reset(&mut self) {
        self.ease.reset();
        self.spring.reset();
    }
}

/// One side's per-frame geometry inputs.
pub struct WrapSideInput<'a> {
    /// The articulated pin-line circles, front→rear: `[sprocket, road wheels…, idler]`, each a
    /// side-plane `(z, y)` centre + pin-line radius. The sprocket MUST be first and the idler last
    /// (the taut wrap keys its end arcs off that order).
    pub circles: &'a [(Vec2, f32)],
    /// Signed track-centreline x (left −, right +) — where this side's belt plane sits.
    pub plane_x: f32,
    /// The lateral terrain stations (signed hull-x offsets from `plane_x`): the measured shoe faces,
    /// per-side because the shoe is not centred on its pins. The conform takes the deepest of them.
    pub lateral_stations: [f32; 3],
    /// Total belt travel (m) along the loop — the resample offset and the material-pitch registration.
    pub phase: f64,
}

/// The whole-frame inputs shared by both sides.
pub struct WrapInput<'a> {
    pub dt: f32,
    /// Hull→world affine (the presented pose).
    pub affine: Affine3A,
    /// The immutable material loop length: `pitch × count`, exact.
    pub belt_len: f32,
    /// Material link count.
    pub count: usize,
    /// Material link pitch (m).
    pub pitch: f32,
    /// Plate thickness (m) — the conform pushes the pin line to the outer face at `thickness/2`.
    pub thickness: f32,
    /// Downward probe reach (m) for the terrain oracle.
    pub probe_reach: f32,
    /// Whether to build the taut [`WrapSideOutput::reference`] loop — an extra `sag_span` + `Vec`
    /// per side. The sandbox's `-` diagnostic layer wants it; the game passes `false` (it draws no
    /// reference) so it never pays for the throwaway.
    pub reference: bool,
    pub sides: [WrapSideInput<'a>; 2],
}

/// One side's drawn output.
pub struct WrapSideOutput {
    /// The drawn pin joints (link centres), material-pitch spaced along the closed loop — `count`
    /// of them, in loop order. Link `i` spans joint `i` → `i+1` (wrapping).
    pub joints: Vec<Vec2>,
    /// The taut (unconformed) reference loop — a diagnostic layer (belt-vs-reference deviation shows
    /// where terrain holds the belt off its rest path). EMPTY unless [`WrapInput::reference`] asked
    /// for it; the game ignores it and never requests it.
    pub reference: Vec<Vec2>,
}

/// Fit both sides' belts around the articulated circles for one frame. Advances each side's filter
/// state in place; returns the drawn joints (and, on request, the reference loop) per side.
///
/// Allocation: each call builds a handful of transient `Vec`s per side (the resampled stations, the
/// per-station normals/depths, the drawn joints). At the measured ~56 µs/tank/frame this is fine;
/// if a future many-tank tier ever wants it, these are the obvious buffer-reuse candidates (thread a
/// scratch pool through the caller). Deliberately NOT done now — YAGNI.
pub fn step<O: TerrainOracle>(
    input: &WrapInput,
    oracle: &O,
    state: &mut [WrapState; 2],
) -> [WrapSideOutput; 2] {
    let [st0, st1] = state;
    [
        side_step(input, 0, oracle, st0),
        side_step(input, 1, oracle, st1),
    ]
}

fn side_step<O: TerrainOracle>(
    input: &WrapInput,
    si: usize,
    oracle: &O,
    st: &mut WrapState,
) -> WrapSideOutput {
    let side = &input.sides[si];
    let circles = side.circles;
    let rb = raw_belly(
        oracle,
        &input.affine,
        side.plane_x,
        input.thickness,
        input.probe_reach,
        side.lateral_stations,
        circles,
    );
    let chord = rb.idler_up.distance(rb.sprocket_up);
    // The road wheels are the middle circles — the only sag-promotion candidates (the belt drapes
    // onto a road wheel that pokes into the return run, never onto the sprocket or idler).
    let roads = &circles[1..circles.len().saturating_sub(1)];

    // The taut (unconformed) reference loop — a diagnostic layer, unaffected by the filters. Opt-in
    // ([`WrapInput::reference`]): the game draws no reference and never pays for the throwaway.
    let reference = if input.reference {
        close_loop(&rb.taut, rb.idler_up, rb.sprocket_up, input.belt_len, roads)
    } else {
        Vec::new()
    };

    // FEEL TIER 1 — belly memory: ease the widened conform depths in the HULL FRAME, so the belt
    // line can't step frame-to-frame (rise instant, fall ballistic). Only genuinely ground-facing
    // belly stations carry memory (the z-monotone span); the end arcs pass their raw depth through.
    // The eased depths replace the raw ones both in the drawn belly AND in the length budget below,
    // so the top run stays consistent with what is drawn.
    let zs: Vec<f32> = rb.stations.iter().map(|p| p.x).collect();
    let eased = st.ease.ease(&zs, &rb.widened, &rb.ground, input.dt);
    let conformed = conformed_pts(&rb, &eased);

    // Close with the budgeted sag. FEEL TIER 2 — slack spring: the sag budget is the leftover belt
    // length (from the EASED belly, so the tiers compose), spring-tracked so the return run eases
    // between drapes. The `max(0)` is the explicit length-budget clamp; the spring clamps ≥ 0 too.
    let raw_excess = (input.belt_len - polyline_len(&conformed) - chord).max(0.0);
    let drawn_excess = st.spring.step(raw_excess, input.dt, chord);
    let mut loop_pts = conformed;
    sag_span(
        rb.idler_up,
        rb.sprocket_up,
        drawn_excess,
        roads,
        0,
        &mut loop_pts,
    );
    if let Some(&first) = loop_pts.first() {
        loop_pts.push(first);
    }

    // Space the pins at the MATERIAL pitch, not the drawn one (see [`station_params`]): the links
    // are rigid, so the conformed polyline is read as a uniform-strain image of the material loop
    // and sampled in material arc-length — otherwise the drawn belt walks out from under the
    // sprocket tooth lock at ~one tooth per 160 m.
    let (spacing, offset) = station_params(
        side.phase,
        input.pitch,
        polyline_len(&loop_pts),
        input.count,
    );
    let mut joints = resample(&loop_pts, spacing, offset);
    joints.truncate(input.count);

    WrapSideOutput { joints, reference }
}

/// **Belly memory** (feel tier 1) — a temporal ease on the terrain-conform depth, keyed to
/// HULL-FRAME position.
///
/// This is a FILTER, not physics: it holds a running per-position estimate of how far terrain has
/// lifted the belt off its taut line, and each frame relaxes it toward this frame's raw field
/// depth. It is self-healing — it cannot tear, buckle, or accumulate error, and it has NO reseed
/// concept: if its grid is uninitialised, its span moves out from under it, or a teleport drops it,
/// it re-inits from the CURRENT frame's raw depths (never zeros — a zero-init would draw the belt
/// INSIDE terrain for one release period).
///
/// The memory is keyed to a fixed hull-local **z-grid**, NOT to material links and NOT to raw
/// resample indices. The dense bottom-run resample changes its station COUNT as the bottom path
/// lengthens and shortens; index-keyed memory would smear a belly feature sideways as the count
/// drifts. Hull-local z is stable (the running gear is hull-fixed; only wheel Y articulates), so
/// terrain features ADVECT through the grid exactly as they advect under the tank — the memory of
/// a bump sits where the bump is, in the tank's own frame.
///
/// Only the **belly** carries this memory: a z-grid is only single-valued over a z-MONOTONE span,
/// and the belly bottom run is the only one. The sprocket/idler end arcs fold z back on themselves
/// (two branches share a z), so a `ground`-facing gate ([`GROUND_FACING_EPS`]) restricts memory to
/// stations whose hull-frame outward normal points genuinely down; the arcs and top run pass their
/// RAW conform depth through untouched (drawn positions AND budget) and never write a cell.
///
/// The relaxation is ASYMMETRIC and the asymmetry is load-bearing: when the raw depth is GREATER
/// than the eased value (the belt must lift MORE to clear an obstacle) it snaps up INSTANTLY —
/// easing upward would draw the belt inside the obstacle. When the raw depth is lower (the obstacle
/// has passed) it FALLS BALLISTICALLY at gravity toward the target — a velocity per cell that
/// accelerates the drop and lands on the raw depth. This is exactly
/// [`wheel_lift_step`](super::wheels::wheel_lift_step)'s own fall policy, ported to the belt line:
/// it is PARAMETER-FREE (no release time constant to tune) and SCALE-CORRECT (a deep belly takes
/// longer to settle than a shallow lip, by g·t² alone), and — because the belt falls at the same
/// gravity as the wheels it spans — the two can never settle at different rates.
#[derive(Default)]
struct ConformEase {
    z_lo: f32,
    /// Cell size (m); `0.0` ⇒ uninitialised / reset.
    dz: f32,
    /// Eased depth per hull-local z-cell.
    nodes: Vec<f32>,
    /// Ballistic fall velocity per cell (m/s, ≤ 0 while dropping) — the accelerating fall's state.
    vel: Vec<f32>,
}

impl ConformEase {
    /// Drop the memory (a teleport / respawn / snap correction): the next [`Self::ease`] re-inits
    /// from raw.
    fn reset(&mut self) {
        self.dz = 0.0;
        self.nodes.clear();
        self.vel.clear();
    }

    /// Ease this frame's raw per-station depths (`raw[i]` at hull-local z `zs[i]`) and return the
    /// eased depth per station. Only `ground[i]` (belly) stations carry memory; the rest pass `raw`
    /// through untouched. Rise is instant; fall is exact constant-g kinematics; the returned value
    /// is floored at the raw depth so the belt is NEVER drawn inside terrain.
    fn ease(&mut self, zs: &[f32], raw: &[f32], ground: &[bool], dt: f32) -> Vec<f32> {
        // Grid range over PARTICIPATING (belly) stations only — the end arcs fold z back on
        // themselves and must never key memory (see the type doc's monotonicity note).
        let (mut z_min, mut z_max) = (f32::INFINITY, f32::NEG_INFINITY);
        for (i, &z) in zs.iter().enumerate() {
            if ground[i] {
                z_min = z_min.min(z);
                z_max = z_max.max(z);
            }
        }
        if !z_min.is_finite() {
            // No belly this frame (fully airborne / no ground-facing span): drop stale memory and
            // pass every station's raw depth through.
            self.reset();
            return raw.to_vec();
        }
        // (Re)initialise when uninitialised or the span moved out of range — rare, since the
        // z-span is hull-fixed. Seed every cell from THIS frame's raw depth, never zero.
        let covered = self.dz > 0.0
            && z_min >= self.z_lo
            && z_max <= self.z_lo + self.dz * self.nodes.len() as f32;
        if !covered {
            self.dz = CONFORM_EASE_CELL;
            self.z_lo = z_min - self.dz;
            let n = (((z_max - z_min) / self.dz).ceil() as usize) + 3;
            self.nodes = vec![0.0; n];
            self.vel = vec![0.0; n];
            for (i, &z) in zs.iter().enumerate() {
                if ground[i] {
                    let j = self.cell(z);
                    self.nodes[j] = self.nodes[j].max(raw[i]);
                }
            }
        }
        let n = self.nodes.len();
        // This frame's per-cell raw target (max of the BELLY stations that land in the cell).
        let mut target = vec![0.0_f32; n];
        let mut hit = vec![false; n];
        for (i, &z) in zs.iter().enumerate() {
            if ground[i] {
                let j = self.cell(z);
                target[j] = target[j].max(raw[i]);
                hit[j] = true;
            }
        }
        for j in 0..n {
            if !hit[j] {
                continue; // no belly station this frame ⇒ hold (never fall toward a phantom zero)
            }
            if target[j] >= self.nodes[j] {
                self.nodes[j] = target[j]; // rise: instant
                self.vel[j] = 0.0;
            } else {
                // fall: EXACT constant-g kinematics toward the lower target (frame-rate invariant) —
                // advance position with the OLD velocity + ½g·dt², then the velocity. Lands on it.
                self.nodes[j] += self.vel[j] * dt - 0.5 * WRAP_FEEL_G * dt * dt;
                self.vel[j] -= WRAP_FEEL_G * dt;
                if self.nodes[j] <= target[j] {
                    self.nodes[j] = target[j];
                    self.vel[j] = 0.0;
                }
            }
        }
        zs.iter()
            .zip(raw)
            .zip(ground)
            .map(|((&z, &r), &g)| if g { self.sample(z).max(r) } else { r })
            .collect()
    }

    /// Cell index for a hull-local z, clamped into range.
    fn cell(&self, z: f32) -> usize {
        (((z - self.z_lo) / self.dz) as isize).clamp(0, self.nodes.len() as isize - 1) as usize
    }

    /// Linear sample of the eased grid at hull-local z (between cell centres).
    fn sample(&self, z: f32) -> f32 {
        let n = self.nodes.len();
        let f = (z - self.z_lo) / self.dz - 0.5;
        if f <= 0.0 {
            return self.nodes[0];
        }
        if f >= (n - 1) as f32 {
            return self.nodes[n - 1];
        }
        let j = f.floor() as usize;
        let frac = f - j as f32;
        self.nodes[j] * (1.0 - frac) + self.nodes[j + 1] * frac
    }
}

/// **Slack spring** (feel tier 2) — a 1-DOF spring on the top-run sag budget.
///
/// The stateless wrap feeds the leftover belt length (`belt_len − bottom_run − chord`) straight
/// into the sag parabola, so the return run SNAPS between shapes the instant the belly budget
/// changes. This tier tracks that budget with a damped spring (position = drawn slack, plus a
/// velocity), so the top run eases between drapes. Its target is the budget computed from the EASED
/// belly (tier 1's output), so the tiers compose.
///
/// Both the spring's frequency and its damping are DERIVED, not tuned. The frequency is a hanging
/// span's PENDULUM rate `ω = √(g / sag_depth)`: a deep drape swings slowly, a shallow one snaps back
/// fast — the physically honest scaling, with the sag depth floored ([`SLACK_SAG_FLOOR`]) so a
/// near-taut run stays finite. The damping is the project-canonical `ζ = 0.5` ([`WRAP_FEEL_ZETA`]).
///
/// The step is the EXACT closed-form underdamped solution (see [`Self::step`]), so it is
/// unconditionally stable and frame-rate invariant: a 100 ms hitch frame relaxes correctly rather
/// than overshooting the way a semi-implicit Euler step would (Euler needs `dt < ~2/ω`, and at
/// `ω ≈ 22 rad/s` a hitch would launch it to several times the target and draw impossible slack).
/// The filter contract also requires it never carry a wound: any non-finite state snaps to rest.
///
/// CAVEAT — length conservation: a single scalar spring cannot conserve the material loop length.
/// On overshoot the drawn slack transiently exceeds the budget, so the top run draws slightly MORE
/// belt than the loop actually has. `ζ = 0.5` keeps that (now energy-bounded) overshoot modest; it
/// is a known, accepted cosmetic artifact of the one-DOF model, not a length leak in the sim (the
/// sim's belt is the rigid pin loop, untouched here).
#[derive(Default)]
struct SlackSpring {
    value: f32,
    velocity: f32,
    /// Whether the spring holds a live value; `false` ⇒ the next step snaps to the target.
    primed: bool,
}

impl SlackSpring {
    /// Drop the state (a teleport / respawn / snap correction): the next [`Self::step`] snaps to the
    /// target.
    fn reset(&mut self) {
        self.primed = false;
    }

    /// Advance the spring toward `target` (m of slack) and return the drawn value, clamped ≥ 0.
    /// `chord` is the return-run span; the frequency is DERIVED from the current drawn sag depth
    /// (`√(g / sag_depth)`, pendulum scaling), the damping is the canonical [`WRAP_FEEL_ZETA`].
    ///
    /// The integrator is the EXACT underdamped discretization of `e″ + 2ζω·e′ + ω²·e = 0` about the
    /// (constant-over-dt) target, `e = value − target`. With `ω_d = ω√(1−ζ²)`, `A = e^(−ζω·dt)`:
    /// `e ← A[e·cos(ω_d·dt) + (v + ζω·e)/ω_d · sin(ω_d·dt)]`,
    /// `v ← A[v·cos(ω_d·dt) − (ω²·e + ζω·v)/ω_d · sin(ω_d·dt)]`.
    /// `A ∈ (0, 1]` bounds it at every `dt` — unconditionally stable, frame-rate invariant.
    fn step(&mut self, target: f32, dt: f32, chord: f32) -> f32 {
        // Self-heal (filter contract): first frame, a degenerate dt, or ANY non-finite state (a
        // prior NaN, a pathological input) snaps to rest at the target rather than propagating.
        if !self.primed
            || dt <= 0.0
            || !self.value.is_finite()
            || !self.velocity.is_finite()
            || !target.is_finite()
        {
            self.value = target.max(0.0);
            self.velocity = 0.0;
            self.primed = true;
            return self.value;
        }
        // Sag depth of the CURRENT drawn slack (`h = √(3·chord·excess / 8)`, the same parabola
        // `sag_span` draws), floored so the pendulum rate stays finite as the run goes taut.
        let sag_depth = (3.0 * chord * self.value / 8.0).sqrt().max(SLACK_SAG_FLOOR);
        let omega = (WRAP_FEEL_G / sag_depth).sqrt();
        let zeta = WRAP_FEEL_ZETA; // ζ = 0.5 ⇒ always underdamped
        let omega_d = omega * (1.0 - zeta * zeta).sqrt();
        let decay = (-zeta * omega * dt).exp();
        let (s, c) = (omega_d * dt).sin_cos();
        let e = self.value - target;
        let v = self.velocity;
        self.value = target + decay * (e * c + (v + zeta * omega * e) / omega_d * s);
        self.velocity = decay * (v * c - (omega * omega * e + zeta * omega * v) / omega_d * s);
        if self.value < 0.0 {
            self.value = 0.0;
            self.velocity = self.velocity.max(0.0);
        }
        // Belt-and-suspenders: a post-step non-finite (pathological chord/target) never leaves here.
        if !self.value.is_finite() || !self.velocity.is_finite() {
            self.value = target.max(0.0);
            self.velocity = 0.0;
        }
        self.value
    }
}

/// The taut wrap + terrain-conform of one side — the reference path, the dense conform stations,
/// their outward normals and raw conform depths. Pure geometry + field probes, no temporal state:
/// the belly-memory ease is applied by the caller.
struct RawBelly {
    /// The pre-resample taut bottom polyline — the reference/rest path.
    taut: Vec<Vec2>,
    /// The dense resample of the bottom run (+ `idler_up`) — the conform stations.
    stations: Vec<Vec2>,
    /// Per-station outward normal.
    outs: Vec<Vec2>,
    /// Per-station raw conform depth after the ±1-station overhang max filter.
    widened: Vec<f32>,
    /// Per-station: is this a genuinely ground-facing BELLY station (may carry z-keyed memory)?
    /// `outs[i].y < −`[`GROUND_FACING_EPS`] — the arcs and top run are `false`.
    ground: Vec<bool>,
    idler_up: Vec2,
    sprocket_up: Vec2,
}

/// Build one side's taut wrap + raw conform: [`taut_lower_run`]'s lower-envelope walk over the
/// ordered pin-line `circles` (`[sprocket, road wheels…, idler]`, already articulated & sorted
/// front→rear), then the bottom run densely resampled and probed against the field along its
/// outward normal.
fn raw_belly<O: TerrainOracle>(
    oracle: &O,
    affine: &Affine3A,
    plane_x: f32,
    thickness: f32,
    probe_reach: f32,
    lateral_stations: [f32; 3],
    circles: &[(Vec2, f32)],
) -> RawBelly {
    // The taut bottom polyline, sprocket_up → front arc → tangents/arcs → idler_up — the shared
    // envelope walk, sunk RAW (no dedupe: the conform resample rides the walk's own spacing).
    let (sprocket_c, sprocket_r) = circles[0];
    let (idler_c, idler_r) = *circles.last().unwrap();
    let (idler_up, sprocket_up) = external_tangent(idler_c, idler_r, sprocket_c, sprocket_r, 1.0);
    let mut taut: Vec<Vec2> = Vec::new();
    taut_lower_run(circles, sprocket_up, idler_up, |p| taut.push(p));

    // The conform stations: a dense resample of the taut bottom, so a board mid-tangent is sampled.
    // Displace each ground-facing station AGAINST its outward normal by the directional field depth
    // — a buried station is lifted back INSIDE the loop until its outer face sits on the surface.
    // Deepest of the physics' 3 lateral columns (the visual≡physics invariant).
    let mut stations = resample(&taut, BELT_DRAW_SPACING, 0.0);
    stations.push(idler_up);
    let m = stations.len();
    let outs: Vec<Vec2> = (0..m)
        .map(|i| {
            let tan =
                (stations[(i + 1).min(m - 1)] - stations[i.saturating_sub(1)]).normalize_or_zero();
            Vec2::new(tan.y, -tan.x)
        })
        .collect();
    let lat_axis = affine.transform_vector3(Vec3::X);
    let depths: Vec<f32> = (0..m)
        .map(|i| {
            let out2 = outs[i];
            if out2 == Vec2::ZERO {
                return 0.0;
            }
            let s2 = stations[i] + out2 * (thickness / 2.0);
            let w = affine.transform_point3(Vec3::new(plane_x, s2.y, s2.x));
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            // Station offsets are hull-x measurements (shoe faces relative to the pin plane) — shift
            // along the hull's lateral axis, per-side signed.
            let mut d = 0.0_f32;
            for offset in lateral_stations {
                d = d.max(oracle.depth_along(w + lat_axis * offset, out, probe_reach));
            }
            d.max(0.0)
        })
        .collect();
    // A rigid link OVERHANGS a board edge: the line stays high for about half a pitch before the
    // pin clears the edge, then articulates down over the next. Reproduce it on the displacement
    // field: a ±1-station max filter (the overhang; never sinks a lift) — the triangular smooth (the
    // articulation rounding) is applied in [`conformed_pts`] after the ease.
    let widened: Vec<f32> = (0..m)
        .map(|i| {
            depths[i.saturating_sub(1)]
                .max(depths[i])
                .max(depths[(i + 1).min(m - 1)])
        })
        .collect();
    // Belly participation: only stations whose hull-frame outward normal points genuinely DOWN
    // carry z-keyed memory. The end arcs fold z, so their sideways/up faces are excluded.
    let ground: Vec<bool> = outs.iter().map(|o| o.y < -GROUND_FACING_EPS).collect();
    RawBelly {
        taut,
        stations,
        outs,
        widened,
        ground,
        idler_up,
        sprocket_up,
    }
}

/// Displace one side's conform stations by the eased depths (the ±1 overhang was already applied in
/// [`raw_belly`]; this is the 3-tap triangular articulation smooth). The drawn belly polyline.
fn conformed_pts(rb: &RawBelly, eased: &[f32]) -> Vec<Vec2> {
    let m = rb.stations.len();
    (0..m)
        .map(|i| {
            let d = 0.25 * eased[i.saturating_sub(1)]
                + 0.5 * eased[i]
                + 0.25 * eased[(i + 1).min(m - 1)];
            if d > 0.0 {
                rb.stations[i] - rb.outs[i] * d
            } else {
                rb.stations[i]
            }
        })
        .collect()
}

/// Close a bottom polyline (sprocket_up → … → idler_up) into the full belt loop: the belt length
/// left over after the bottom run becomes the return run's drape ([`sag_span`]). The `max(0)` on the
/// excess is the explicit length-budget clamp: a conform-lengthened bottom run beyond the total belt
/// length runs the top taut instead of laundering the deficit into the shape (the infeasibility
/// rule). Used for the diagnostic reference loop; the drawn loop routes its excess through the
/// [`SlackSpring`] instead.
fn close_loop(
    bottom: &[Vec2],
    idler_up: Vec2,
    sprocket_up: Vec2,
    belt_length: f32,
    wheels: &[(Vec2, f32)],
) -> Vec<Vec2> {
    let mut pts = bottom.to_vec();
    let chord = idler_up.distance(sprocket_up);
    let excess = (belt_length - polyline_len(bottom) - chord).max(0.0);
    sag_span(idler_up, sprocket_up, excess, wheels, 0, &mut pts);
    pts
}

/// Resample spacing and phase offset that place the drawn pin stations exactly `material_pitch`
/// apart along a conformed loop — the pin spacing the sprocket phase lock (`view::tooth_angle`)
/// assumes, and the one that keeps the drawn belt registered to the teeth over any travel.
///
/// The links are RIGID: the material loop is exactly `material_pitch · count`. The conformed
/// polyline only approximates it — arc and sag polyline discretisation leave `poly_len` ~0.08% off
/// — so resampling at the naive `poly_len / count` spaces the pins at the DRAWN pitch, and the drawn
/// belt walks out from under the material-pitch tooth lock at ~one tooth per 160 m.
///
/// The cure is a uniform-strain reparametrisation: treat the polyline as the drawn image of the
/// material loop (`strain = poly_len / (material_pitch · count)` drawn metres per material metre)
/// and sample at material positions `offset_m + i · material_pitch` mapped back through the strain.
/// Pin spacing is then the material pitch to float precision, pin 0 advances exactly one station
/// per material pitch of travel (so the sprocket's one-tooth-per-pitch lock never drifts), and the
/// loop closes: `phase += count · material_pitch` returns `offset_m`, and every station, to itself.
pub(crate) fn station_params(
    phase: f64,
    material_pitch: f32,
    poly_len: f32,
    count: usize,
) -> (f32, f32) {
    let material_len = material_pitch * count.max(1) as f32;
    let strain = if material_len > 1e-6 {
        poly_len / material_len
    } else {
        1.0
    };
    let (_, offset_m) = phase_decompose(phase, material_pitch);
    (material_pitch * strain, offset_m * strain)
}
