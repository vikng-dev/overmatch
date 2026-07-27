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
use super::route::{
    SagClip, external_tangent, max_admissible_depth, polyline_len, resample, sag_depth, sag_span,
    slack, taut_lower_run,
};

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
    /// The DRAWN link spacing (m): [`WrapInput::pitch`] times the loop's uniform strain, i.e. what
    /// [`station_params`] actually stepped the joints along the loop by. Equal to the material pitch
    /// only when the drawn path happens to be exactly the material length.
    ///
    /// This is the value the drawn polyline is RESAMPLED at, so it — not the pitch — is what
    /// anything bounding the drawn belt's POLYGON has to charge. That is a correctness choice, and
    /// it is worth saying plainly that it is not currently a load-bearing one: MEASURED 2026-07-27
    /// over the drawn-belt sweep's whole travel band the strain runs 1.0002 (full droop) to 1.0020
    /// (the bump stop, where the running gear asks for the most belt), and re-running that sweep
    /// with the material pitch substituted for this moves the tightest budget by 0.03 mm against a
    /// 0.35 mm margin and flips no assertion. The term uses the drawn spacing because it is the
    /// right number, not because a failure was ever observed to hang on it.
    pub spacing: f32,
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

    // The taut (unconformed) reference loop — a diagnostic layer, unaffected by the filters. Opt-in
    // ([`WrapInput::reference`]): the game draws no reference and never pays for the throwaway.
    let reference = if input.reference {
        close_loop(
            &rb.taut,
            rb.idler_up,
            rb.sprocket_up,
            input.belt_len,
            circles,
        )
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
    let conformed = conformed_pts(&rb, &eased, circles);

    // Close with the budgeted sag. FEEL TIER 2 — slack spring: the sag budget is the leftover belt
    // length (from the EASED belly, so the tiers compose), spring-tracked so the return run eases
    // between drapes. `route::slack` is the explicit length-budget clamp; the spring clamps ≥ 0 too.
    let raw_excess = slack(input.belt_len, polyline_len(&conformed), chord);
    let drawn_excess = st.spring.step(raw_excess, input.dt, chord);
    let mut loop_pts = conformed;
    // [`SagClip::EveryCircle`]: the DRAWN drape is pushed out of the sprocket and idler too, not
    // just the road wheels — and the length that costs is absorbed by `station_params`' strain.
    sag_span(
        rb.idler_up,
        rb.sprocket_up,
        drawn_excess,
        circles,
        SagClip::EveryCircle,
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

    WrapSideOutput {
        joints,
        reference,
        spacing,
    }
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
/// accelerates the drop and lands on the raw depth. It is PARAMETER-FREE (no release time constant
/// to tune) and SCALE-CORRECT (a deep belly takes longer to settle than a shallow lip, by g·t²
/// alone).
///
/// The FALL is exactly [`wheel_lift_step`](super::wheels::wheel_lift_step)'s own — same gravity, so
/// the belt and the wheels it spans can never settle DOWN at different rates. The RISE is not: the
/// wheels ease up over a playtested ~100 ms (Yan's 2026-07-17 A/B; see that function's doc) while
/// the belt snaps, so on a crest the belt reaches terrain a few frames before the wheels do. That
/// asymmetry is bounded on the drawn side by [`conformed_pts`]' running-gear clamp rather than by
/// unifying the two rates — MEASURED 2026-07-27 it is worth 0.23 mm of belt-vs-rim margin with the
/// clamp in place, against the 112.78 mm the missing clamp was worth.
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

    /// The spring's pendulum rate `ω = √(g / h)` for a drape of `chord` carrying `excess` metres of
    /// slack, at the drape's own [`sag_depth`] floored by [`SLACK_SAG_FLOOR`] so a near-taut run
    /// stays finite.
    fn omega(chord: f32, excess: f32) -> f32 {
        (WRAP_FEEL_G / sag_depth(chord, excess).max(SLACK_SAG_FLOOR)).sqrt()
    }

    /// Frames of `dt` after which a step input is settled to under 1 %: five damping time constants
    /// `1/(ζω)` at [`Self::omega`]. The spring's OWN statement of "settled", so a sweep that waits
    /// for it asks the filter instead of copying its rate derivation.
    #[cfg(test)]
    fn settle_frames(chord: f32, excess: f32, dt: f32) -> usize {
        ((5.0 / (WRAP_FEEL_ZETA * Self::omega(chord, excess))) / dt).ceil() as usize
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
        // The pendulum rate of the CURRENT drawn slack — the same parabola `sag_span` draws.
        let omega = Self::omega(chord, self.value);
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
///
/// # The running-gear clamp
///
/// The conform's job is to lift the drawn belly onto TERRAIN; the invariant it must not break is
/// that the drawn belly is the support envelope of the running gear UNION the ground. Nothing in the
/// depth field knows about the wheels — a crest between two stations lifts the line by whatever the
/// oracle read, and on a ridge that is enough to draw the belt straight through the road wheels it
/// is meant to be hanging under — MEASURED +121 mm of belt above a wheel rim on the crest this was
/// diagnosed from, and +112.78 mm past budget on the analytic sweep that reproduces it
/// (`a_swept_crest_never_lifts_the_drawn_belt_above_a_wheel`). So
/// every station's depth is capped at [`max_admissible_depth`] — the closed-form distance along its
/// own inward normal to the first circle it would enter.
///
/// The cap is applied AFTER the ease and the 3-tap smooth, and that ordering is load-bearing: both
/// of those are linear filters over the depth field, so clamping earlier lets them average a clamped
/// station back up with its neighbours and re-enter the wheel. Clamping last is the only place the
/// invariant is stated about the line that is actually drawn.
fn conformed_pts(rb: &RawBelly, eased: &[f32], circles: &[(Vec2, f32)]) -> Vec<Vec2> {
    let m = rb.stations.len();
    (0..m)
        .map(|i| {
            let d = 0.25 * eased[i.saturating_sub(1)]
                + 0.5 * eased[i]
                + 0.25 * eased[(i + 1).min(m - 1)];
            let d = d.min(max_admissible_depth(rb.stations[i], rb.outs[i], circles));
            if d > 0.0 {
                rb.stations[i] - rb.outs[i] * d
            } else {
                rb.stations[i]
            }
        })
        .collect()
}

/// Close a bottom polyline (sprocket_up → … → idler_up) into the full belt loop: the belt length
/// left over after the bottom run becomes the return run's drape ([`sag_span`]), through
/// `route::slack`'s length-budget clamp (a conform-lengthened bottom run beyond the total belt
/// length runs the top taut instead of laundering the deficit into the shape — the infeasibility
/// rule). Used for the diagnostic reference loop; the drawn loop routes its excess through the
/// [`SlackSpring`] instead.
fn close_loop(
    bottom: &[Vec2],
    idler_up: Vec2,
    sprocket_up: Vec2,
    belt_length: f32,
    circles: &[(Vec2, f32)],
) -> Vec<Vec2> {
    let mut pts = bottom.to_vec();
    let chord = idler_up.distance(sprocket_up);
    let excess = slack(belt_length, polyline_len(bottom), chord);
    sag_span(
        idler_up,
        sprocket_up,
        excess,
        circles,
        SagClip::EveryCircle,
        &mut pts,
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;

    use crate::track::derive::SuspensionParams;
    use crate::track::oracle::{BlockField, TerrainOracle};
    use crate::track::rig_geom::{tiger_rig, tiger_spec};
    use crate::track::route::{SAG_CLIP_INSET, chord_inset, deepest_inside, on_circle};
    use crate::track::side::Side;
    use crate::track::wheels::{
        WHEEL_LIFT_RISE_OMEGA, WheelParams, wheel_lift_step, wheel_lift_target,
    };

    /// The coarsest chord the taut walk's own arc discretisation lays on one circle: the longest
    /// step between consecutive walk points that lie ON it. Measured off the walk rather than
    /// assumed, because `route::arc`'s fixed segment count makes the chord a function of the wrap
    /// angle, and the wrap angle moves with the suspension pose. A circle the envelope skipped
    /// (a lifted wheel) contributes nothing and returns 0.
    ///
    /// The reference circle here is the ROUTE circle, never the rendered rim — the cut below is
    /// measured against the rim, so a wrong radius base still moves the two apart.
    fn wrap_arc_chord(taut: &[Vec2], circle: (Vec2, f32)) -> f32 {
        // Every point the walk puts on a given circle is emitted contiguously (its entry tangent
        // point, then its arc), so consecutive pairs of the filtered run are real chords.
        let on: Vec<Vec2> = taut
            .iter()
            .copied()
            .filter(|&p| on_circle(p, circle))
            .collect();
        on.windows(2)
            .map(|w| w[0].distance(w[1]))
            .fold(0.0, f32::max)
    }

    /// **The pin-polygon budget** for one circle: how far inside it the drawn belt is allowed to
    /// sit, with the terms that make it up (for a failure message).
    ///
    /// The chords STACK, all derived and none authored, and each is drawn on the polyline the one
    /// before it already inset — so each is taken about the radius its predecessor left behind. The
    /// loop has two producers and they stack differently, so the budget is the LOOSER of the two
    /// chains:
    ///
    /// * **lower run** — the taut walk's own arc chord (a property of the wrap ANGLE: a road
    ///   wheel's few degrees give a millimetric chord and pay nothing, while the sprocket and the
    ///   idler each wrap ~165° DERIVED and get a chord the size of a link), then the conform
    ///   resample ([`BELT_DRAW_SPACING`]) the joints are picked off, then the drawn link itself
    ///   ([`WrapSideOutput::spacing`] across the pin circle);
    /// * **drape** — the ride inset `route::sag_clip_chord` is DEFINED as the inverse of, i.e.
    ///   [`SAG_CLIP_INSET`] read directly rather than round-tripped through the chord and back, then
    ///   the drawn link. No resample term: [`side_step`] appends the drape AFTER the conform
    ///   resample.
    ///
    /// One number per circle rather than a per-link producer classifier: the drape's own chain is
    /// the tighter of the two on every circle of this rig, so splitting the loop by producer buys
    /// MEASURED 0.4 mm of extra tightness on the links the drape wrote — and costs a classifier, a
    /// provenance field on the production output and the plumbing to carry it. The physical ceiling
    /// below is what actually keeps this honest.
    fn pin_polygon_budget(
        taut: &[Vec2],
        circle: (Vec2, f32),
        link_spacing: f32,
    ) -> (f32, [f32; 3]) {
        let r = circle.1;
        let arc = chord_inset(r, wrap_arc_chord(taut, circle));
        let draw = chord_inset(r - arc, BELT_DRAW_SPACING);
        let lower = (
            arc + draw + chord_inset(r - arc - draw, link_spacing),
            [arc, draw, chord_inset(r - arc - draw, link_spacing)],
        );
        let ride = SAG_CLIP_INSET;
        let drape = (
            ride + chord_inset(r - ride, link_spacing),
            [ride, 0.0, chord_inset(r - ride, link_spacing)],
        );
        if lower.0 >= drape.0 { lower } else { drape }
    }

    /// Deepest the drawn shoe's inner face reaches inside a rendered rim of radius `rim` centred at
    /// `c`. Negative return = every link is clear.
    fn deepest_cut(joints: &[Vec2], c: Vec2, rim: f32, pin_to_inner: f32) -> f32 {
        (0..joints.len())
            .map(|k| {
                let (a, b) = (joints[k], joints[(k + 1) % joints.len()]);
                let ab = b - a;
                let t = ((c - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
                pin_to_inner + rim - c.distance(a + ab * t)
            })
            .fold(f32::NEG_INFINITY, f32::max)
    }

    /// One side's circles at a per-station vertical offset — the pose builder every sweep here
    /// shares. Index `i` is the ROAD WHEEL index (the sprocket and idler are hull-fixed).
    fn posed_circles(rest: &[(Vec2, f32)], dy: impl Fn(usize) -> f32) -> Vec<(Vec2, f32)> {
        let last = rest.len() - 1;
        rest.iter()
            .enumerate()
            .map(|(i, &(c, r))| match i {
                0 => (c, r),
                i if i == last => (c, r),
                i => (Vec2::new(c.x, c.y + dy(i - 1)), r),
            })
            .collect()
    }

    /// The taut walk of a pose — the polyline the lower run's first stacked chord is read off.
    fn taut_walk(circles: &[(Vec2, f32)]) -> (Vec<Vec2>, Vec2, Vec2) {
        let last = circles.len() - 1;
        let (idler_up, sprocket_up) = external_tangent(
            circles[last].0,
            circles[last].1,
            circles[0].0,
            circles[0].1,
            1.0,
        );
        let mut taut: Vec<Vec2> = Vec::new();
        taut_lower_run(circles, sprocket_up, idler_up, |p| taut.push(p));
        (taut, idler_up, sprocket_up)
    }

    /// One frame's [`WrapInput`] for the shipped Tiger at a given pose/phase.
    fn wrap_input<'a>(
        rig: &'a crate::track::rig_geom::RigGeom,
        circles: &'a [(Vec2, f32)],
        phase: f64,
        dt: f32,
        affine: Affine3A,
        reference: bool,
    ) -> WrapInput<'a> {
        WrapInput {
            dt,
            affine,
            belt_len: rig.belt_len(),
            count: rig.link_count,
            pitch: rig.pitch,
            thickness: rig.thickness,
            probe_reach: 0.5,
            reference,
            sides: Side::ALL.map(|s| WrapSideInput {
                circles,
                plane_x: s.plane_x(rig.plane_x),
                lateral_stations: rig.grip_stations(s),
                phase,
            }),
        }
    }

    /// **The drawn shoe against the rendered rim.** The pin route is built at
    /// `tread + pin_to_inner` ([`super::super::derive::pin_line_radius`]), so the shoe's inner face
    /// lands exactly ON the wheel tread — *for a circular belt*. The drawn belt is not circular: it
    /// is a chain of straight links, and every straight span across a wrap arc lies inside the
    /// circle it spans. The links therefore cut into the road wheels by a bounded, purely geometric
    /// amount, and that bound ([`pin_polygon_budget`]) is what this test states.
    ///
    /// Exceeding it means the belt is no longer merely polygonal about the right circle — a wrong
    /// radius base (tread vs pin line is MEASURED ~26 mm), a flipped `pin_to_inner` (MEASURED
    /// ~51 mm), or a template scale (proportional) all land far outside it, which is the point: the
    /// assertion is loose enough to be true of the model we have and tight enough that none of those
    /// can hide under it. The sweep PRINTS its own tightest case rather than asking anyone to
    /// remember one.
    ///
    /// # The ceiling, and why a relative budget alone would not do
    ///
    /// Every term of the budget is DERIVED from the same constants that drive production —
    /// [`BELT_DRAW_SPACING`], `sag_clip_chord`, `route::ARC_SEGMENTS`, the drawn link spacing. That
    /// is what makes it honest, and it is also a loop: coarsen a producer and its own budget widens
    /// in lockstep, so the gate stays green while the belt visibly degrades. The backstop is a
    /// PHYSICAL ceiling that no producer can move — `pin_to_inner`, the MEASURED 25.56 mm from the
    /// pin line to the shoe's inner face (`marker_model`, off the glb's own `Link_Box`). Every
    /// budget AND every cut must stay under it, because a link cutting deeper than the shoe's own
    /// inner face is not an inset polygon at all — it is geometry that cannot be drawn.
    ///
    /// # What is swept, and why each axis is there
    ///
    /// * **Pose** — the whole travel band, full droop to the bump stop, both uniformly and with
    ///   alternating stations at opposite ends (the pattern real terrain produces, and the one where
    ///   a wheel's wrap arc is widest).
    /// * **Belt phase** — `PHASE_STEPS` evenly spaced across ONE material pitch. That is the full
    ///   period, not a slice of one: [`station_params`] takes the phase modulo the pitch, so every
    ///   distinct placement of the joints relative to the geometry occurs inside one pitch.
    /// * **Time** — the sweep carries ONE [`WrapState`] through the whole thing, so every pose
    ///   change lands on the slack spring as a real step input, and each block then runs the
    ///   spring's OWN settle window ([`SlackSpring::settle_frames`]) before the assertion. (Tier 1
    ///   has nothing to settle here: the field is empty, so every raw conform depth is zero and the
    ///   belly memory is seeded at its own target on frame one.)
    ///
    /// MEASURED 2026-07-27: the tightest SETTLED case leaves 0.43 mm spare (road wheel 0 at 8.01 mm
    /// of an 8.44 mm budget), and against the 25.56 mm physical ceiling the loosest budget in the
    /// whole sweep is 13.89 mm and the deepest cut 13.11 mm — both a little over HALF the ceiling:
    /// room for the model to breathe, none for it to stop meaning anything.
    ///
    /// The budget is asserted in the SETTLED regime. The spring's documented overshoot
    /// ([`SlackSpring`]'s length-conservation caveat) is a deliberate, accepted cosmetic artifact,
    /// so it is MEASURED rather than asserted away: the sweep tracks the worst `cut − budget` over
    /// every frame including the transients and prints it beside the tightest settled case (MEASURED
    /// 2026-07-27: 0.42 mm spare, on the same road wheel — the transient comes marginally closer to
    /// the bound than any settled frame but stays inside it). If that print ever goes positive the
    /// artifact has become visible and the caveat needs revisiting, not the number raising.
    ///
    /// # The reference loop rides its end circles too
    ///
    /// [`close_loop`] takes [`SagClip::EveryCircle`] for exactly the reason the drawn loop does, and
    /// nothing else in this module would notice if it stopped: the game never asks for the loop, so
    /// quietly reverting that one call to [`SagClip::RoadWheels`] would be a change no assertion
    /// sees. It is a real regression — the sandbox's `-` layer draws this loop, and the
    /// belt-vs-reference deviation readout is only meaningful if the reference is the shape the belt
    /// is deviating FROM. So this sweep asks for the reference and asserts its RETURN RUN alone
    /// (the tail [`close_loop`] appends to the raw taut bottom, sliced off at the bottom's own
    /// length): measuring the whole loop would state a bound the LOWER run sets, which says nothing
    /// about the clip under test. On the drape the bound is the ride's own [`SAG_CLIP_INSET`].
    /// Under `RoadWheels` the drape sinks MEASURED 33 mm into
    /// the idler and 48 mm into the sprocket — an order of magnitude past it, which is what makes
    /// the reversion loud. The reference is built from the RAW taut walk and the belt length alone,
    /// so it is checked once per pose rather than once per frame.
    ///
    /// The two END circles are also the reason the drawn drape asks for [`SagClip::EveryCircle`] at
    /// all. Clipping the sprocket to its own pin circle is not merely conservative: the pins
    /// physically ride the teeth at exactly that radius, and the teeth still stand proud through the
    /// links because the drawn tooth tip is `pin_to_inner` inside it. The SIM route deliberately
    /// does NOT take that clip — the length it costs is not free there — which is why this test
    /// reads the DRAWN joints and nothing else.
    #[test]
    fn the_drawn_shoe_never_cuts_a_road_wheel_deeper_than_the_pin_polygon() {
        let rig = tiger_rig();
        let params = SuspensionParams::default();
        let field = BlockField::new(vec![]);
        let pti = rig.model.pin_to_inner;
        let droop = rig.droop_travel(&params).effective;
        let lift = tiger_spec().track.suspension.bump_stop;
        let dt = 1.0 / 60.0;

        /// Belt phases per pose, evenly spaced across one material pitch (the station map's full
        /// period — see the test doc).
        const PHASE_STEPS: usize = 5;
        let phases: Vec<f64> = (0..PHASE_STEPS)
            .map(|k| f64::from(rig.pitch) * k as f64 / PHASE_STEPS as f64)
            .collect();

        println!(
            "\ndrawn shoe vs rendered rim — pitch {:.5} m, pin_to_inner {pti:.5} m, \
             travel band -{droop:.3}..+{lift:.3} m, {PHASE_STEPS} phases across one pitch",
            rig.pitch,
        );

        // Uniform poses across the band, plus the two alternating patterns.
        let uniform: Vec<f32> = (0..=8)
            .map(|i| -droop + (droop + lift) * i as f32 / 8.0)
            .collect();
        let mut poses: Vec<(String, Box<dyn Fn(usize) -> f32>)> = uniform
            .iter()
            .map(|&dy| {
                let label = format!("all {dy:+.3}");
                let f: Box<dyn Fn(usize) -> f32> = Box::new(move |_| dy);
                (label, f)
            })
            .collect();
        for (label, odd) in [("alt -/+", true), ("alt +/-", false)] {
            let f: Box<dyn Fn(usize) -> f32> =
                Box::new(move |i| if (i % 2 == 0) == odd { -droop } else { lift });
            poses.push((label.to_string(), f));
        }

        // ONE state for the whole sweep: the pose change at each block boundary IS the spring's
        // step input, and nothing else in this test would ever move it off its target.
        let mut state = [WrapState::default(), WrapState::default()];
        // Worst `cut − budget` on ANY frame, transients included, and the block it happened in.
        let mut transient = (f32::NEG_INFINITY, String::new());
        // The loosest budget and the deepest cut the whole sweep produced, against the physical
        // ceiling — printed so the doc's headroom figure is a reading rather than a memory.
        let mut ceiling = (0.0_f32, 0.0_f32);
        // The same quantity restricted to SETTLED frames: how much of its own budget the tightest
        // asserted case leaves spare.
        let mut tightest = (f32::NEG_INFINITY, String::new());

        for &phase in &phases {
            println!("  phase {phase:.4} m");
            for (label, dy) in &poses {
                let rest = rig.rest.get(Side::Right);
                let last = rest.len() - 1;
                let circles = posed_circles(rest, dy);
                let input = wrap_input(&rig, &circles, phase, dt, Affine3A::IDENTITY, true);
                let (taut, idler_up, sprocket_up) = taut_walk(&circles);

                // The rim every comparison is made against is the MEASURED mesh radius — the thing
                // actually rendered — never the wrap circle's own radius, or the test could not see
                // a wrong radius base at all (both sides would move together).
                let mut targets: Vec<(String, (Vec2, f32), f32)> = vec![
                    ("the sprocket".into(), circles[0], circles[0].1 - pti),
                    ("the idler".into(), circles[last], rig.model.idler_radius),
                ];
                for (i, &circle) in circles.iter().enumerate().take(last).skip(1) {
                    targets.push((format!("wheel {}", i - 1), circle, rig.model.wheel_tread));
                }

                // Settle window: the slack spring's own, at the drape this pose implies.
                let chord = idler_up.distance(sprocket_up);
                let excess = slack(rig.belt_len(), polyline_len(&taut), chord);
                let settle = SlackSpring::settle_frames(chord, excess, dt);

                let mut settled_cuts: Vec<f32> = Vec::new();
                let mut settled_spacing = rig.pitch;
                for frame in 0..settle {
                    let out = &mut step(&input, &field, &mut state)[1];
                    let link_spacing = out.spacing;
                    let joints = std::mem::take(&mut out.joints);
                    let reference = std::mem::take(&mut out.reference);
                    let is_settled = frame + 1 == settle;

                    // The diagnostic reference loop's return run — pose-only, so once per pose.
                    if frame == 0 {
                        assert!(
                            reference.len() > taut.len(),
                            "`WrapInput::reference` asked for the loop and got no return run"
                        );
                        // `close_loop` appends the drape to the raw taut bottom, so the return run
                        // is exactly the tail past it (the joining vertex is shared, hence the −1).
                        let drape = &reference[taut.len() - 1..];
                        for &circle in &circles {
                            let cut = deepest_inside(drape, circle);
                            assert!(
                                cut <= SAG_CLIP_INSET,
                                "{label}: the reference loop's return run cuts the circle at {:?} \
                                 (r {:.3}) {:.2} mm deep, past its {:.3} mm ride budget — the drape \
                                 is not riding the end circles (has `close_loop` reverted to \
                                 `SagClip::RoadWheels`?)",
                                circle.0,
                                circle.1,
                                cut * 1000.0,
                                SAG_CLIP_INSET * 1000.0,
                            );
                        }
                    }

                    for (what, circle, rim) in &targets {
                        let cut = deepest_cut(&joints, circle.0, *rim, pti);
                        let (budget, [first, draw, link]) =
                            pin_polygon_budget(&taut, *circle, link_spacing);
                        if is_settled {
                            assert!(
                                cut <= budget,
                                "{label} @ phase {phase:.4}: {what} is cut {:.2} mm deep, past the \
                                 {:.2} mm pin-polygon budget (producer chord {:.2} mm + conform \
                                 chord {:.2} mm + link chord {:.2} mm at pin radius {:.5} m). The \
                                 belt is no longer polygonal about the right circle.",
                                cut * 1000.0,
                                budget * 1000.0,
                                first * 1000.0,
                                draw * 1000.0,
                                link * 1000.0,
                                circle.1,
                            );
                            // PHYSICAL CEILING — the one bound here that cannot move with the
                            // producers it is bounding. See the test doc.
                            assert!(
                                budget < pti && cut < pti,
                                "{label} @ phase {phase:.4}: the budget is {:.2} mm and the {what} \
                                 cut is {:.2} mm, against a shoe face {:.2} mm inside the pin line. \
                                 A cut past the shoe's own inner face is not a polygon inset — it \
                                 is geometry that cannot be drawn, so a producer has been coarsened \
                                 past physical sense (the relative budget would have tracked it and \
                                 stayed green).",
                                budget * 1000.0,
                                cut * 1000.0,
                                pti * 1000.0,
                            );
                            ceiling = (ceiling.0.max(budget), ceiling.1.max(cut));
                            if cut - budget > tightest.0 {
                                tightest = (
                                    cut - budget,
                                    format!(
                                        "{what} of {label} @ phase {phase:.4} ({:.2} mm cut vs \
                                         {:.2} mm budget)",
                                        cut * 1000.0,
                                        budget * 1000.0,
                                    ),
                                );
                            }
                            settled_cuts.push(cut);
                            settled_spacing = link_spacing;
                        }
                        if cut - budget > transient.0 {
                            transient = (
                                cut - budget,
                                format!(
                                    "{what} on frame {frame}/{settle} of {label} @ phase \
                                     {phase:.4} ({:.2} mm cut vs {:.2} mm budget)",
                                    cut * 1000.0,
                                    budget * 1000.0,
                                ),
                            );
                        }
                    }
                }

                print!(
                    "    {label:<10} {settle:>3} f  strain {:.4}  sprocket {:5.2} mm  \
                     idler {:5.2} mm  road wheels",
                    settled_spacing / rig.pitch,
                    settled_cuts[0] * 1000.0,
                    settled_cuts[1] * 1000.0,
                );
                for cut in &settled_cuts[2..] {
                    print!(" {:5.2}", cut * 1000.0);
                }
                println!(
                    "  (budget mm: road {:.2}, sprocket {:.2}, idler {:.2})",
                    pin_polygon_budget(&taut, circles[1], settled_spacing).0 * 1000.0,
                    pin_polygon_budget(&taut, circles[0], settled_spacing).0 * 1000.0,
                    pin_polygon_budget(&taut, circles[last], settled_spacing).0 * 1000.0,
                );
            }
        }
        println!(
            "  tightest SETTLED margin (the asserted regime): {:+.2} mm of budget — {}",
            tightest.0 * 1000.0,
            tightest.1,
        );
        println!(
            "  worst TRANSIENT frame (not asserted, see the doc): {:+.2} mm of budget — {}",
            transient.0 * 1000.0,
            transient.1,
        );
        println!(
            "  physical ceiling (pin_to_inner {:.2} mm): loosest budget {:.2} mm, deepest cut \
             {:.2} mm",
            pti * 1000.0,
            ceiling.0 * 1000.0,
            ceiling.1 * 1000.0,
        );
    }

    /// An analytic RIDGE: flat ground at `y = 0` with a symmetric tent of height `peak` and flank
    /// slope `tan(angle)`, crested at world `z = at_z`. Exact and pose-continuous, like every other
    /// oracle — the belt view is a per-frame function of this, so a sampled ground would put its own
    /// resolution into the numbers below.
    ///
    /// The solid is `{y ≤ 0} ∪ ({y ≤ peak − s·(z − at_z)} ∩ {y ≤ peak + s·(z − at_z)})`, so a first
    /// hit is a slab clip against each convex piece and a `min` across the union — closed form, no
    /// marching.
    struct Ridge {
        peak: f32,
        slope: f32,
        at_z: f32,
    }

    impl TerrainOracle for Ridge {
        fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
            let origin = station - out * reach;
            // Ray parameters where it is inside the half-plane `y ≤ a·z + b`.
            let slab = |a: f32, b: f32| -> (f32, f32) {
                let f0 = origin.y - a * origin.z - b;
                let df = out.y - a * out.z;
                if df.abs() < 1e-12 {
                    if f0 <= 0.0 {
                        (f32::NEG_INFINITY, f32::INFINITY)
                    } else {
                        (f32::INFINITY, f32::NEG_INFINITY)
                    }
                } else if df < 0.0 {
                    (f0 / -df, f32::INFINITY)
                } else {
                    (f32::NEG_INFINITY, -f0 / df)
                }
            };
            let enter = |slabs: &[(f32, f32)]| {
                let a = slabs.iter().map(|s| s.0).fold(0.0_f32, f32::max);
                let b = slabs.iter().map(|s| s.1).fold(f32::INFINITY, f32::min);
                if a <= b { a } else { f32::INFINITY }
            };
            let ground = enter(&[slab(0.0, 0.0)]);
            let tent = enter(&[
                slab(-self.slope, self.peak + self.slope * self.at_z),
                slab(self.slope, self.peak - self.slope * self.at_z),
            ]);
            (reach - ground.min(tent)).clamp(-reach, reach)
        }
    }

    /// The drawn belly's `y` where it crosses side-plane abscissa `z` — the LOWEST branch, since the
    /// return run crosses the same abscissa far above. `None` if nothing crosses.
    fn belly_y(joints: &[Vec2], z: f32) -> Option<f32> {
        let n = joints.len();
        (0..n)
            .filter_map(|k| {
                let (a, b) = (joints[k], joints[(k + 1) % n]);
                let dz = b.x - a.x;
                if dz.abs() < 1e-9 {
                    return None;
                }
                let t = (z - a.x) / dz;
                (0.0..=1.0).contains(&t).then_some(a.y + (b.y - a.y) * t)
            })
            .fold(None, |lo: Option<f32>, y| Some(lo.map_or(y, |m| m.min(y))))
    }

    /// The Tiger's view wheel params for a crest run — the same shape [`super::super::view`] builds.
    fn crest_wheel_params(
        rig: &crate::track::rig_geom::RigGeom,
        params: &SuspensionParams,
    ) -> WheelParams {
        WheelParams {
            reach: rig.wheel_pin_radius() + rig.model.pin_to_outer,
            ease_omega: WHEEL_LIFT_RISE_OMEGA,
            max_lift: tiger_spec().track.suspension.bump_stop,
            max_droop: rig.droop_travel(params).effective,
            lateral_stations: [-0.08, 0.0, 0.08],
            probe_reach: 0.5,
        }
    }

    /// **The belt may never float above the wheels it hangs under.** The pin-polygon sweep above is
    /// run over an EMPTY field, so it says nothing at all about the terrain conform — and the
    /// conform is the producer that raises the drawn belly. This is that missing half-plane, and the
    /// direction it asserts is the one Yan's crest screenshots failed: the belt line above a road
    /// wheel's rim rather than below it.
    ///
    /// The scenario is a tank driven across an analytic [`Ridge`] at driving speed, carrying ONE
    /// [`WrapState`] AND the wheel-lift filter state through the whole crossing — because both
    /// halves of the bug were temporal: the conform lifts the belt onto terrain the instant the
    /// oracle reads it, and (before this) the wheels rose toward the same terrain over ~100 ms, so
    /// for a few frames the belt hung off nothing. The hull is held at its rest height rather than
    /// climbing, which is the WORST case for the invariant: the ridge intrudes past the bump stop,
    /// the wheels bottom out, and the conform is asked for a lift the running gear cannot follow.
    ///
    /// The margin is the same [`pin_polygon_budget`] the empty-field sweep asserts — the drawn belt
    /// is a chord polygon about the pin circles either way, and there is exactly one statement of
    /// how far inside them a chord may sit.
    ///
    /// MEASURED 2026-07-27, worst case over the whole sweep, as `belt-above-rim − budget`:
    ///
    /// ```text
    ///   no clamp, eased wheel rise (the shipped bug)   +112.78 mm
    ///   no clamp, instant wheel rise                   +106.53 mm
    ///   clamp, eased wheel rise (this)                   -0.56 mm
    ///   clamp, instant wheel rise                        -0.33 mm
    /// ```
    ///
    /// The clamp is the whole fix: the wheel-rise law moves it by 0.23 mm, which is why the
    /// playtested ease stayed (`wheels::wheel_lift_step`) rather than being unified with the belt's
    /// own instant rise on doctrinal grounds. The pre-fix figure lines up with the +121 mm read off
    /// the crest screenshots this was diagnosed from.
    #[test]
    fn a_swept_crest_never_lifts_the_drawn_belt_above_a_wheel() {
        let rig = tiger_rig();
        let params = SuspensionParams::default();
        let dt = 1.0 / 60.0;
        let rest = rig.rest.get(Side::Right);
        let last = rest.len() - 1;
        let wparams = crest_wheel_params(&rig, &params);

        println!("\nswept crest — belt vs wheel rim (hull pinned at rest height)");
        let mut worst = (f32::NEG_INFINITY, String::new());
        for peak in [0.12_f32, 0.20] {
            for angle_deg in [10.0_f32, 20.0, 30.0] {
                for speed in [3.0_f32, 6.0] {
                    let field = Ridge {
                        peak,
                        slope: angle_deg.to_radians().tan(),
                        at_z: 0.0,
                    };
                    let mut state = [WrapState::default(), WrapState::default()];
                    let mut lift = vec![(0.0_f32, 0.0_f32); last - 1];
                    let mut block = f32::NEG_INFINITY;
                    // Enter well clear of the ridge and leave well past it.
                    let frames = ((8.0 / speed) / dt) as usize;
                    for f in 0..frames {
                        let z = -4.0 + speed * f as f32 * dt;
                        let affine = Affine3A::from_translation(Vec3::new(0.0, rig.hull_rest_y, z));
                        for (i, (dy, dvel)) in lift.iter_mut().enumerate() {
                            let c = rest[i + 1].0;
                            let pivot = Vec3::new(rig.plane_x, c.y, c.x);
                            let target =
                                wheel_lift_target(&field, &affine, Vec3::NEG_Y, pivot, &wparams);
                            wheel_lift_step(dy, dvel, target, dt, &wparams);
                        }
                        let circles = posed_circles(rest, |i| lift[i].0);
                        let input = wrap_input(&rig, &circles, 0.0, dt, affine, false);
                        let out = &mut step(&input, &field, &mut state)[1];
                        let link_spacing = out.spacing;
                        let joints = std::mem::take(&mut out.joints);
                        let (taut, _, _) = taut_walk(&circles);
                        for (i, &circle) in circles.iter().enumerate().take(last).skip(1) {
                            let Some(y) = belly_y(&joints, circle.0.x) else {
                                continue;
                            };
                            let float = y - (circle.0.y - circle.1);
                            let budget = pin_polygon_budget(&taut, circle, link_spacing).0;
                            block = block.max(float - budget);
                            assert!(
                                float <= budget,
                                "peak {peak:.2} m / {angle_deg:.0}° / {speed:.0} m/s, frame {f} at \
                                 z {z:+.2}: the drawn belt sits {:.1} mm ABOVE wheel {}'s rim, past \
                                 the {:.2} mm pin-polygon budget — the conform lifted the belly \
                                 through the running gear.",
                                float * 1000.0,
                                i - 1,
                                budget * 1000.0,
                            );
                        }
                    }
                    println!(
                        "  peak {peak:.2} m  {angle_deg:>2.0}°  {speed:.0} m/s  {frames:>3} frames \
                         — worst belt-above-rim margin {:+.2} mm of budget",
                        block * 1000.0,
                    );
                    if block > worst.0 {
                        worst = (
                            block,
                            format!("peak {peak:.2} m / {angle_deg:.0}° / {speed:.0} m/s"),
                        );
                    }
                }
            }
        }
        println!(
            "  worst across the sweep: {:+.2} mm of budget — {}",
            worst.0 * 1000.0,
            worst.1,
        );
    }

    /// The same invariant with every filter removed: wheels SNAPPED to their targets and a fresh
    /// [`WrapState`] each frame, so what is left is pure geometry. Pinned separately from the swept
    /// run above so a failure says which half moved — the conform's residual, or a filter transient.
    /// MEASURED 2026-07-27: worst `belt-above-rim − budget` is +35.39 mm without the clamp and
    /// −1.22 mm with it, so the residual is pure geometry and the swept run's extra millimetres are
    /// the filters.
    ///
    /// It also asserts the OTHER direction, because a clamp is a trivially safe way to break a
    /// conform: the same pose is drawn against the ridge and against an empty field, and the ridge's
    /// belly must still sit measurably HIGHER between the wheels. A clamp that killed the conform
    /// outright would pass every bound above and draw the belt straight through the crest.
    #[test]
    fn a_static_crest_never_lifts_the_drawn_belt_above_a_wheel() {
        let rig = tiger_rig();
        let params = SuspensionParams::default();
        let dt = 1.0 / 60.0;
        let rest = rig.rest.get(Side::Right);
        let last = rest.len() - 1;
        let wparams = crest_wheel_params(&rig, &params);

        println!("\nstatic crest — belt vs wheel rim (wheels snapped, no filter memory)");
        let mut worst = (f32::NEG_INFINITY, String::new());
        for angle_deg in [10.0_f32, 20.0, 30.0] {
            let field = Ridge {
                peak: 0.20,
                slope: angle_deg.to_radians().tan(),
                at_z: 0.0,
            };
            let mut block = f32::NEG_INFINITY;
            let mut conform = 0.0_f32;
            for k in -16..=16 {
                let z = k as f32 * 0.25;
                let affine = Affine3A::from_translation(Vec3::new(0.0, rig.hull_rest_y, z));
                let dys: Vec<f32> = (1..last)
                    .map(|i| {
                        let c = rest[i].0;
                        let pivot = Vec3::new(rig.plane_x, c.y, c.x);
                        wheel_lift_target(&field, &affine, Vec3::NEG_Y, pivot, &wparams)
                    })
                    .collect();
                let circles = posed_circles(rest, |i| dys[i]);
                let input = wrap_input(&rig, &circles, 0.0, dt, affine, false);
                let mut state = [WrapState::default(), WrapState::default()];
                let out = &mut step(&input, &field, &mut state)[1];
                let link_spacing = out.spacing;
                let joints = std::mem::take(&mut out.joints);
                // The SAME pose over an empty field: the difference is the conform alone.
                let mut bare_state = [WrapState::default(), WrapState::default()];
                let bare = std::mem::take(
                    &mut step(&input, &BlockField::new(vec![]), &mut bare_state)[1].joints,
                );
                for k in 0..=40 {
                    let z =
                        circles[last].0.x + (circles[0].0.x - circles[last].0.x) * k as f32 / 40.0;
                    if let (Some(a), Some(b)) = (belly_y(&joints, z), belly_y(&bare, z)) {
                        conform = conform.max(a - b);
                    }
                }
                let (taut, _, _) = taut_walk(&circles);
                for (i, &circle) in circles.iter().enumerate().take(last).skip(1) {
                    let Some(y) = belly_y(&joints, circle.0.x) else {
                        continue;
                    };
                    let float = y - (circle.0.y - circle.1);
                    let budget = pin_polygon_budget(&taut, circle, link_spacing).0;
                    block = block.max(float - budget);
                    assert!(
                        float <= budget,
                        "{angle_deg:.0}° crest at z {z:+.2}: the drawn belt sits {:.1} mm ABOVE \
                         wheel {}'s rim, past the {:.2} mm pin-polygon budget",
                        float * 1000.0,
                        i - 1,
                        budget * 1000.0,
                    );
                }
            }
            println!(
                "  {angle_deg:>2.0}°  worst belt-above-rim margin {:+.2} mm of budget, most the \
                 conform lifted the belly {:.1} mm",
                block * 1000.0,
                conform * 1000.0,
            );
            assert!(
                conform > 0.010,
                "{angle_deg:.0}° crest: the terrain conform lifted the drawn belly by at most \
                 {:.2} mm over a 200 mm ridge — the running-gear clamp has turned the conform off \
                 rather than bounding it",
                conform * 1000.0,
            );
            if block > worst.0 {
                worst = (block, format!("{angle_deg:.0}°"));
            }
        }
        println!(
            "  worst across the sweep: {:+.2} mm of budget — {}",
            worst.0 * 1000.0,
            worst.1,
        );
    }
}
