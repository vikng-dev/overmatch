//! Kinematic track view shared by the game and sandbox; decisions: `.agents/docs/design/track-model/architecture.md` §1a/§3.
//!
//! The belt path is fitted around articulated running-gear circles: a taut convex wrap, terrain
//! conform on a dense lower run, and budgeted sag on the return run. [`step`] reads exact contacts
//! immediately; [`step_cached`] retains and interpolates raw directional contacts between samples.
//! Both apply the same filters and running-gear clearance clamp before sampling material joints.
//!
//! Unconditional cosmetic filters add client-local history: [`ConformEase`] tracks hull-frame
//! depths with ballistic fall; [`SlackSpring`] tracks sag with pendulum frequency. Neither has a
//! tuning input or toggle. Drawn belts depend on that history as well as replicated pose/phase;
//! simulation pins and gear spin are unaffected.
//!
//! Pure math and caller-owned state, without ECS or assets, like [`super::wheels`]. Inputs:
//! articulated circles, oracle, hull affine, phase, material length and per-side lateral stations.
//! Keep one [`WrapState`] per side; output is drawn joints and an optional taut diagnostic loop.

use bevy::math::{Affine3A, Vec2, Vec3};

use super::derive::G;
use super::forces::phase_decompose;
use super::oracle::DepthOracle;
use super::route::{
    SagClip, external_tangent, max_admissible_depth, polyline_len, resample, sag_depth, sag_span,
    slack, taut_lower_run,
};

/// Resolution: half-critical damping controls ringing in the cosmetic slack spring.
const WRAP_FEEL_ZETA: f32 = 0.5;
/// Resolution: sag-depth floor (m) keeps DERIVED pendulum frequency `ω = √(g / sag_depth)` finite
/// as a return span goes taut (sag → 0).
const SLACK_SAG_FLOOR: f32 = 0.02;
/// Resolution: hull-z memory cells (m), coarser than [`BELT_DRAW_SPACING`] so interior cells receive
/// stations each frame. Unsampled cells hold their values, never falling toward a phantom zero.
const CONFORM_EASE_CELL: f32 = 0.15;
/// Resolution: bottom-run probe spacing (m) samples bumps between wheels; probing only tangent
/// endpoints would miss a board mid-segment and let the belt cut through it.
const BELT_DRAW_SPACING: f32 = 0.1;
/// Resolution: ground-facing cutoff; memory requires `out.y < −0.2`; sideways/upward arcs pass raw depth.
/// [`ConformEase`] owns the z-monotonicity requirement.
const GROUND_FACING_EPS: f32 = 0.2;

/// Caller-owned cosmetic memory per side, retained across frames. On teleport, respawn or snap
/// correction, [`Self::reset`] seeds from current raw targets rather than falling from a stale pose.
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

    /// The belly memory's eased conform depth at side-plane `z`.
    #[cfg(test)]
    pub(crate) fn eased_depth(&self, z: f32) -> f32 {
        self.ease.sample(z)
    }
}

/// One side's per-frame geometry inputs.
pub struct WrapSideInput<'a> {
    /// Articulated `(z, y)` centres and pin-line radii, front→rear: `[sprocket, road wheels…, idler]`.
    /// End-arc construction requires that endpoint order.
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
    /// Plate thickness (m); probes use `thickness/2` as the surface offset, assuming centred pins.
    pub thickness: f32,
    /// Downward probe reach (m) for the terrain oracle.
    pub probe_reach: f32,
    /// Build [`WrapSideOutput::reference`] for the sandbox's `-` diagnostic layer. The game passes
    /// `false` to avoid an unused `sag_span` and `Vec` per side.
    pub reference: bool,
    pub sides: [WrapSideInput<'a>; 2],
}

/// One side's drawn output.
pub struct WrapSideOutput {
    /// The drawn pin joints (link centres), material-pitch spaced along the closed loop — `count`
    /// of them, in loop order. Link `i` spans joint `i` → `i+1` (wrapping).
    pub joints: Vec<Vec2>,
    /// Unconformed loop: deviation shows terrain lifting the belt off its rest path.
    /// Empty unless requested by [`WrapInput::reference`].
    pub reference: Vec<Vec2>,
    /// Drawn spacing (m): [`WrapInput::pitch`] × uniform strain, as resampled by [`station_params`].
    /// It equals material pitch only when drawn and material lengths match. Polygon bounds must
    /// charge this spacing. MEASURED reference geometry (25.56 mm pin offset): strain 1.0002 at full droop
    /// to 1.0020 at the bump stop; substituting material pitch moved the tightest budget by 0.03 mm
    /// against a 0.35 mm margin, flipping no assertion. Current fixture: MEASURED strain 1.0013 at
    /// full droop, 1.0000 at bump, 0.9999..1.0024 over the whole sweep. The choice follows geometry.
    pub spacing: f32,
}

/// Fit both sides' belts around the articulated circles for one frame. Advances each side's filter
/// state in place; returns the drawn joints (and, on request, the reference loop) per side.
///
/// This uncached entry point is the sandbox/reference path. The game's view uses [`step_cached`]
/// to separate terrain probes from render-rate filters and phase sampling.
pub fn step<O: DepthOracle>(
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

fn side_step<O: DepthOracle>(
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
    finish_side(input, si, &rb, &rb.widened, st)
}

fn finish_side(
    input: &WrapInput,
    si: usize,
    rb: &RawBelly,
    depths: &[f32],
    st: &mut WrapState,
) -> WrapSideOutput {
    let side = &input.sides[si];
    let circles = side.circles;
    let chord = rb.idler_up.distance(rb.sprocket_up);

    // Diagnostic reference bypasses both filters; [`WrapInput::reference`] owns the opt-in.
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

    // Ease hull-frame depths (instant rise, ballistic fall) on ground-facing belly stations only;
    // arcs pass raw depths. Both drawn belly and length budget use the eased values.
    let zs: Vec<f32> = rb.stations.iter().map(|p| p.x).collect();
    let eased = st.ease.ease(&zs, depths, &rb.ground, input.dt);
    let conformed = conformed_pts(rb, &eased, circles);

    // Spring-track leftover length from the eased belly, so filters compose. `route::slack`
    // enforces the length budget; the spring also clamps slack ≥ 0.
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

    // Material-registered sampling; [`station_params`] owns the strain map and tooth-drift bound.
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

/// Cosmetic depth memory on a fixed hull-local z-grid. Reinitialise from current raw depths when
/// uninitialised, out of range or reset by a teleport; zero-seeding would put the belt inside
/// terrain for a release period. There is no solver reseed or accumulated constraint error.
///
/// Material-link or resample-index memory would smear features sideways as station counts change.
/// Hull-local z stays stable while wheel Y articulates, so features advect through the grid with
/// terrain. Memory requires a z-monotone belly: end arcs fold z onto two branches with different
/// normals/budgets. [`GROUND_FACING_EPS`] excludes sideways/upward faces; they pass raw depths in
/// both drawing and budget and never write cells.
///
/// Snap upward to clear obstacles immediately; fall with per-cell ballistic velocity onto the raw
/// target. No release-time dial: g·t² makes deep drops take longer than shallow lips.
/// Shares gravity with the wheels, though their discrete integrators differ. Wheels
/// rise over a MEASURED playtested ~100 ms ([`wheel_lift_step`](super::wheels::wheel_lift_step)); the belt
/// snaps, reaching crests a few frames earlier. [`conformed_pts`] bounds that asymmetry rather
/// than unifying rates: MEASURED reference crest-sweep margin changes 0.23 mm with the clamp, versus
/// 112.78 mm for removing the clamp.
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
    /// After teleport/respawn/snap correction, seed the next [`Self::ease`] from raw depths.
    fn reset(&mut self) {
        self.dz = 0.0;
        self.nodes.clear();
        self.vel.clear();
    }

    /// Ease `raw[i]` at hull-z `zs[i]` only where `ground[i]`; pass other depths unchanged.
    /// Floor at raw depth to keep terrain clear; rise instantly and fall with exact constant-g kinematics.
    fn ease(&mut self, zs: &[f32], raw: &[f32], ground: &[bool], dt: f32) -> Vec<f32> {
        // Range only participating belly stations; see the type's z-monotonicity invariant.
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
        // The hull-fixed span rarely leaves coverage; seed from current raw depth when it does.
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
                // Exact constant-g step: position uses old velocity before velocity advances.
                self.nodes[j] += self.vel[j] * dt - 0.5 * G * dt * dt;
                self.vel[j] -= G * dt;
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

/// One-DOF damped spring on `belt_len − eased_bottom_run − chord`, avoiding shape snaps while
/// composing with [`ConformEase`]. DERIVED pendulum frequency `ω = √(g / sag_depth)` slows deep
/// drapes and accelerates shallow ones; [`SLACK_SAG_FLOOR`] keeps it finite near taut. Damping is
/// [`WRAP_FEEL_ZETA`]'s canonical `ζ = 0.5`, a fixed choice rather than a frequency derivation.
///
/// [`Self::step`] solves the underdamped equation exactly for each step's fixed target and rate;
/// the rate is recomputed from drawn sag each step, so the whole filter need not be frame-rate
/// invariant. DERIVED Euler counterexample: `dt < ~2/ω` is required; at `ω ≈ 22 rad/s`, a 100 ms
/// hitch can launch slack to several times its target. Nonfinite state resets to rest.
///
/// A scalar spring cannot conserve loop length: overshoot temporarily draws extra belt.
/// `ζ = 0.5` bounds the fixed-rate response's energy, keeping that accepted cosmetic artifact
/// modest. Simulation's rigid pin loop is unaffected.
#[derive(Default)]
struct SlackSpring {
    value: f32,
    velocity: f32,
    /// Whether the spring holds a live value; `false` ⇒ the next step snaps to the target.
    primed: bool,
}

impl SlackSpring {
    /// Teleport/respawn/snap correction: the next [`Self::step`] snaps to target.
    fn reset(&mut self) {
        self.primed = false;
    }

    /// Pendulum rate `ω = √(g / h)` at the drape's own [`sag_depth`], floored by [`SLACK_SAG_FLOOR`].
    fn omega(chord: f32, excess: f32) -> f32 {
        (G / sag_depth(chord, excess).max(SLACK_SAG_FLOOR)).sqrt()
    }

    /// DERIVED settle window: five damping time constants `1/(ζω)` at [`Self::omega`] for <1 %
    /// step error, in `dt` frames. Sweeps use this owner rather than copy the rate derivation.
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
    /// For fixed target and `ω`, `A ∈ (0, 1]` gives a stable, frame-rate-invariant step.
    fn step(&mut self, target: f32, dt: f32, chord: f32) -> f32 {
        // Filter reset: first frame, nonpositive dt or nonfinite state/target snaps to rest.
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
        // Reset nonfinite output from a pathological chord/target.
        if !self.value.is_finite() || !self.velocity.is_finite() {
            self.value = target.max(0.0);
            self.velocity = 0.0;
        }
        self.value
    }
}

/// View-local contact memory. The caller resets it when terrain, print response or immutable
/// rig parameters change. Phase is deliberately absent: it scrolls over the fitted route.
#[derive(Default)]
pub(crate) struct ContactCache {
    geometry: Option<RawBelly>,
    circles: Vec<(Vec2, f32)>,
    sampled_circles: Vec<(Vec2, f32)>,
    affine: Option<Affine3A>,
    previous: DepthProfile,
    current: DepthProfile,
}

#[derive(Default, Clone)]
struct DepthProfile {
    fractions: Vec<f32>,
    depths: Vec<f32>,
}

impl DepthProfile {
    fn at(&self, fraction: f32) -> f32 {
        let hi = self.fractions.partition_point(|s| *s < fraction);
        if hi == 0 {
            return self.depths[0];
        }
        if hi == self.fractions.len() {
            return *self.depths.last().unwrap();
        }
        let lo = hi - 1;
        let t = (fraction - self.fractions[lo])
            / (self.fractions[hi] - self.fractions[lo]).max(f32::EPSILON);
        self.depths[lo] + (self.depths[hi] - self.depths[lo]) * t
    }
}

/// Refresh exact directional contacts at most once per presented sim tick; interpolate raw
/// depths, then evolve filters, enforce current wheel clearance and sample phase every frame.
/// `refresh` advances sample history even if the inputs are unchanged, completing the last blend.
/// `alpha` weights the previous sample's depths toward the current sample's.
pub(crate) fn step_cached<O: DepthOracle>(
    input: &WrapInput,
    oracle: &O,
    state: &mut [WrapState; 2],
    caches: &mut [ContactCache; 2],
    refresh: bool,
    alpha: f32,
) -> [WrapSideOutput; 2] {
    std::array::from_fn(|si| {
        let side = &input.sides[si];
        let cache = &mut caches[si];
        if cache.geometry.is_none() || cache.circles != side.circles {
            cache.geometry = Some(belly_geometry(side.circles));
            cache.circles.clear();
            cache.circles.extend_from_slice(side.circles);
        }
        let rb = cache.geometry.as_ref().unwrap();
        let cold = cache.affine.is_none();
        if refresh || cold {
            cache
                .previous
                .fractions
                .clone_from(&cache.current.fractions);
            cache.previous.depths.clone_from(&cache.current.depths);
            if cold || cache.affine != Some(input.affine) || cache.sampled_circles != side.circles {
                cache.current = DepthProfile {
                    fractions: rb.fractions.clone(),
                    depths: probe_belly(
                        oracle,
                        &input.affine,
                        side.plane_x,
                        input.thickness,
                        input.probe_reach,
                        side.lateral_stations,
                        rb,
                    ),
                };
                cache.affine = Some(input.affine);
                cache.sampled_circles.clear();
                cache.sampled_circles.extend_from_slice(side.circles);
            }
            if cold {
                cache.previous = cache.current.clone();
            }
        }
        let previous_matches = cache.previous.fractions == rb.fractions;
        let current_matches = cache.current.fractions == rb.fractions;
        let depths: Vec<f32> = rb
            .fractions
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                // Equal geometry takes the exact raw value, without a resampling round trip.
                let sample = |p: &DepthProfile, matches| {
                    if matches { p.depths[i] } else { p.at(s) }
                };
                let from = sample(&cache.previous, previous_matches);
                let to = sample(&cache.current, current_matches);
                from + (to - from) * alpha
            })
            .collect();
        finish_side(input, si, rb, &depths, &mut state[si])
    })
}

/// Geometry and raw directional contacts for one side, without temporal state; the caller eases depths.
struct RawBelly {
    /// The pre-resample taut bottom polyline — the reference/rest path.
    taut: Vec<Vec2>,
    /// The dense resample of the bottom run (+ `idler_up`) — the conform stations.
    stations: Vec<Vec2>,
    /// Per-station outward normal.
    outs: Vec<Vec2>,
    /// Per-station raw conform depth after the ±1-station overhang max filter.
    widened: Vec<f32>,
    /// Z-memory eligibility: `outs[i].y < −`[`GROUND_FACING_EPS`]; sideways/upward arcs are excluded.
    ground: Vec<bool>,
    /// Normalized distance along the lower route, including the end arcs. This is geometric
    /// correspondence across articulated routes, independent of their station counts.
    fractions: Vec<f32>,
    idler_up: Vec2,
    sprocket_up: Vec2,
}

/// [`taut_lower_run`] over articulated front→rear `[sprocket, road wheels…, idler]` pin circles,
/// densely resampled and probed along outward normals.
fn raw_belly<O: DepthOracle>(
    oracle: &O,
    affine: &Affine3A,
    plane_x: f32,
    thickness: f32,
    probe_reach: f32,
    lateral_stations: [f32; 3],
    circles: &[(Vec2, f32)],
) -> RawBelly {
    let mut rb = belly_geometry(circles);
    rb.widened = probe_belly(
        oracle,
        affine,
        plane_x,
        thickness,
        probe_reach,
        lateral_stations,
        &rb,
    );
    rb
}

fn belly_geometry(circles: &[(Vec2, f32)]) -> RawBelly {
    // Sprocket-up → front arc → tangents/arcs → idler-up, without dedupe: resample uses walk spacing.
    let (sprocket_c, sprocket_r) = circles[0];
    let (idler_c, idler_r) = *circles.last().unwrap();
    let (idler_up, sprocket_up) = external_tangent(idler_c, idler_r, sprocket_c, sprocket_r, 1.0);
    let mut taut: Vec<Vec2> = Vec::new();
    taut_lower_run(circles, sprocket_up, idler_up, |p| taut.push(p));

    // Dense stations catch mid-tangent boards; [`probe_belly`] owns the directional lift rule.
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
    let ground = outs.iter().map(|o| o.y < -GROUND_FACING_EPS).collect();
    let mut distance = 0.0;
    let mut fractions = vec![0.0];
    for pair in stations.windows(2) {
        distance += pair[0].distance(pair[1]);
        fractions.push(distance);
    }
    for s in &mut fractions {
        *s /= distance.max(f32::EPSILON);
    }
    RawBelly {
        taut,
        stations,
        outs,
        ground,
        fractions,
        widened: Vec::new(),
        idler_up,
        sprocket_up,
    }
}

/// Probe the half-thickness surface offset for inward lift, taking the deepest of physics' three
/// lateral columns. Offsets are measured shoe faces relative to the pin plane, along signed hull x.
fn probe_belly<O: DepthOracle>(
    oracle: &O,
    affine: &Affine3A,
    plane_x: f32,
    thickness: f32,
    probe_reach: f32,
    lateral_stations: [f32; 3],
    rb: &RawBelly,
) -> Vec<f32> {
    let stations = &rb.stations;
    let outs = &rb.outs;
    let m = stations.len();
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
            let mut d = 0.0_f32;
            for offset in lateral_stations {
                d = d.max(oracle.depth_along(w + lat_axis * offset, out, probe_reach));
            }
            d.max(0.0)
        })
        .collect();
    // Rigid-link overhang holds the line high for about half a pitch past a board edge. The
    // ±1-station max filter preserves lifts; [`conformed_pts`] rounds articulation after easing.
    (0..m)
        .map(|i| {
            depths[i.saturating_sub(1)]
                .max(depths[i])
                .max(depths[(i + 1).min(m - 1)])
        })
        .collect()
}

/// Draw the belly after eased depths receive a 3-tap triangular articulation smooth; [`raw_belly`]
/// already applied ±1-station overhang. It must envelope running gear union ground. Terrain depths
/// alone know nothing of wheels: MEASURED unbounded crest lift +121 mm above a rim, +112.78 mm past
/// budget in `a_swept_crest_never_lifts_the_drawn_belt_above_a_wheel`.
/// Cap each station at [`max_admissible_depth`], its inward-normal distance to the first circle.
/// Clamp AFTER ease and smoothing: neighbour averaging could otherwise re-enter a wheel.
/// The clamp constrains the final drawn stations, not an intermediate field.
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

/// Close diagnostic bottom (sprocket-up → idler-up) with [`sag_span`]. `route::slack` makes the top
/// taut if the bottom exceeds material length, rather than hiding that infeasibility in the drape.
/// The drawn loop passes excess through [`SlackSpring`] instead.
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

/// Drawn spacing and offset for material-pitch registration to `view::tooth_angle` over any travel.
/// Rigid material length is `material_pitch · count`; CLAIMED arc/sag discretisation leaves drawn
/// `poly_len` ~0.08% off, drifting ~one tooth per 160 m if phase advances directly on the drawn loop.
///
/// Map material positions `offset_m + i · material_pitch` through uniform
/// `strain = poly_len / (material_pitch · count)` (drawn m/material m). Material spacing stays exact
/// to float precision; drawn spacing is strained. One material pitch sweeps a full drawn-spacing
/// offset period, then wraps; `phase += count · material_pitch` returns every station to itself.
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
    use crate::track::oracle::{BlockField, DepthOracle};
    use crate::track::rig_geom::{tiger_rig, tiger_spec};
    use crate::track::route::{SAG_CLIP_INSET, chord_inset, deepest_inside, on_circle};
    use crate::track::side::Side;
    use crate::track::wheels::{
        WheelParams, wheel_lift_rise_omega, wheel_lift_step, wheel_lift_target,
    };

    /// Longest chord on a route circle, measured from the walk because fixed `route::arc`
    /// segmentation makes it pose-angle dependent; a skipped/lifted wheel returns zero.
    /// Measure budget on the route circle and cut on the tread rim, so radius-base errors differ.
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

    /// Allowed polygon inset and diagnostic terms. DERIVED chord insets stack at successively
    /// reduced radii; use the looser producer chain:
    /// * Lower: walk arc chord → [`BELT_DRAW_SPACING`] conform chord → [`WrapSideOutput::spacing`]
    ///   link chord. Roadwheels wrap only a few degrees (millimetric chords); DERIVED seed end
    ///   wraps ~165° produce link-sized chords.
    /// * Drape: [`SAG_CLIP_INSET`] → link chord. Read the inset directly, not round-tripped through
    ///   its inverse `route::sag_clip_chord`. No conform chord: drape is appended after resampling.
    ///
    /// Per-link producer classification would tighten drape bounds by MEASURED 0.4 mm on this rig,
    /// but needs a classifier, output provenance and plumbing. The physical ceiling below prevents
    /// producer coarsening from widening this relative budget unnoticed.
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

    /// Straight drawn links inset a circular `tread + pin_to_inner` route; [`pin_polygon_budget`]
    /// bounds that geometry. Reference-geometry wrong radius base (MEASURED ~26 mm), flipped pin offset (MEASURED
    /// ~51 mm), or template scale must exceed the bound; the sweep prints its tightest case.
    ///
    /// Producer-relative budgets alone widen when discretisation coarsens. Require both cut and
    /// budget below `pin_to_inner`, measured from glb `Link_Box` via `marker_model`:
    /// deeper than the shoe's inner face cannot be a polygon inset. Producers are
    /// [`BELT_DRAW_SPACING`], `sag_clip_chord`, `route::ARC_SEGMENTS` and drawn link spacing.
    ///
    /// Sweep full droop → bump stop uniformly and with alternating extremes (widest wheel arcs).
    /// `PHASE_STEPS` spans one material pitch, the full modulo period of [`station_params`]. Carry
    /// one [`WrapState`] so pose changes excite the spring, then wait its own
    /// [`SlackSpring::settle_frames`] window. Empty terrain seeds belly memory at its zero target.
    /// MEASURED reference geometry: tightest settled spare 0.43 mm, wheel 0 at 8.01/8.44 mm; loosest budget 13.89 mm,
    /// deepest cut 13.11 mm, both just over half the 25.56 mm physical ceiling.
    /// Transient overshoot is accepted ([`SlackSpring`]), measured rather than asserted: MEASURED
    /// worst spare 0.42 mm on the same wheel. A positive `cut − budget` means revisit the artifact,
    /// never raise the bound. MEASURED current fixture: 24.64 mm pin offset, 0.46 mm settled spare
    /// (wheel 7, 7.72/8.18 mm), 0.42 mm transient spare; loosest budget 13.89 mm, deepest cut 13.06 mm.
    ///
    /// Sandbox `-` reference deviation requires the reference drape to ride every end circle.
    /// Assert only [`close_loop`]'s appended return tail: a lower-run-dominated whole-loop bound
    /// would miss a clip regression. [`SagClip::RoadWheels`] sinks the drape MEASURED 33 mm into
    /// the idler, 48 mm into the sprocket, well past [`SAG_CLIP_INSET`]. Raw taut geometry and belt
    /// length alone determine the reference, so check once per pose.
    /// Both drawn/reference drapes need [`SagClip::EveryCircle`]: pins ride the sprocket teeth at
    /// its pin radius; tooth tips remain `pin_to_inner` inside it, protruding through the links.
    /// Simulation deliberately omits this clip because its length cost is not free; test drawn joints.
    #[test]
    fn the_drawn_shoe_never_cuts_a_road_wheel_deeper_than_the_pin_polygon() {
        let rig = tiger_rig();
        let params = SuspensionParams::default();
        let field = BlockField::new(vec![]);
        let pti = rig.model.pin_to_inner;
        let droop = rig.droop_travel(&params).effective;
        let lift = tiger_spec().track.suspension.bump_stop;
        let dt = 1.0 / 60.0;

        /// Resolution: evenly spaced phases over the material-pitch period (see test doc).
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

        // Pose-block changes excite the same spring state throughout the sweep.
        let mut state = [WrapState::default(), WrapState::default()];
        // Worst `cut − budget` on ANY frame, transients included, and the block it happened in.
        let mut transient = (f32::NEG_INFINITY, String::new());
        // Print loosest budget/deepest cut so the physical headroom is measured.
        let mut ceiling = (0.0_f32, 0.0_f32);
        // Tightest asserted (settled) `cut − budget`.
        let mut tightest = (f32::NEG_INFINITY, String::new());

        for &phase in &phases {
            println!("  phase {phase:.4} m");
            for (label, dy) in &poses {
                let rest = rig.rest.get(Side::Right);
                let last = rest.len() - 1;
                let circles = posed_circles(rest, dy);
                let input = wrap_input(&rig, &circles, phase, dt, Affine3A::IDENTITY, true);
                let (taut, idler_up, sprocket_up) = taut_walk(&circles);

                // Wheel/idler cuts use declared tread radii independently of the route radius;
                // the sprocket target is its pin circle minus the shoe offset.
                let mut targets: Vec<(String, (Vec2, f32), f32)> = vec![
                    ("the sprocket".into(), circles[0], circles[0].1 - pti),
                    (
                        "the idler".into(),
                        circles[last],
                        tiger_spec().track.idler.radius.right,
                    ),
                ];
                for (i, &circle) in circles.iter().enumerate().take(last).skip(1) {
                    targets.push((
                        format!("wheel {}", i - 1),
                        circle,
                        rig.wheel_treads.get(Side::Right)[i - 1].radius,
                    ));
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
                            // Physical ceiling is independent of producer resolution; see test doc.
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

    /// Analytic ridge over `y = 0`, peak at world `z = at_z`, flanks `s = tan(angle)`. Continuous
    /// exact probes keep sampled-ground resolution out of the measurements.
    /// Solid: `{y ≤ 0} ∪ ({y ≤ peak − s·(z − at_z)} ∩ {y ≤ peak + s·(z − at_z)})`;
    /// first hit is the minimum of convex-piece slab clips, without marching.
    struct Ridge {
        peak: f32,
        slope: f32,
        at_z: f32,
    }

    impl DepthOracle for Ridge {
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
    ) -> WheelParams<'static> {
        WheelParams {
            reach: rig.wheel_pin_radius() + rig.model.pin_to_outer,
            ease_omega: wheel_lift_rise_omega(),
            max_lift: tiger_spec().track.suspension.bump_stop,
            max_droop: rig.droop_travel(params).effective,
            lateral_spans: &[(-0.08, 0.08)],
            probe_reach: 0.5,
        }
    }

    /// Terrain conform must not lift the belt above its wheels beyond [`pin_polygon_budget`];
    /// the empty-field sweep cannot exercise that producer. Cross an analytic [`Ridge`] at driving
    /// speed with persistent [`WrapState`] and wheel-lift history: belt rise is instant while
    /// wheels take MEASURED ~100 ms. A hull pinned at rest height makes the ridge exceed bump travel,
    /// asking for conform the bottomed wheels cannot follow.
    ///
    /// MEASURED reference geometry, worst `belt-above-rim − budget` (mm):
    /// | Clamp | Eased rise | Instant rise |
    /// |---|---:|---:|
    /// | absent | +112.78 | +106.53 |
    /// | present | -0.56 | -0.33 |
    /// The 0.23 mm rate effect with the clamp does not justify discarding the playtested wheel
    /// ease (`wheels::wheel_lift_step`). The unclamped result agrees with MEASURED +121 mm crest
    /// screenshots. MEASURED current fixture: -0.36 mm with clamp/eased rise. This gate covers
    /// temporal lag; the static gate below isolates geometry.
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

    /// Isolate geometry from temporal lag: snapped wheels and fresh [`WrapState`] each frame.
    /// MEASURED reference geometry: worst `belt-above-rim − budget` +35.39 mm unclamped, −1.22 mm clamped; extra swept
    /// error comes from filters. Also compare the same pose over ridge and empty terrain: ridge
    /// conform must raise the belly between wheels. Disabling conform would pass every upper bound
    /// while drawing through the crest. MEASURED current fixture: −1.35 mm clamped.
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
