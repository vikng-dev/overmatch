//! The simulated-chain view tier (architecture §1): the step-24 XPBD chain solved inside the
//! route tube, on its own fixed clock. Pure state + stepper — no ECS; the caller owns the
//! adapters (the sandbox's `V` view today, the game's near-tank tier in phase A).
//!
//! Design provenance (sandbox steps 21–24, HQ.md): drive only at the sprocket sector, exact
//! immutable link pitch, XPBD bending relative to the route's own curvature, torque-limited pin
//! DRY friction toward the previous material angle (the rope-vs-track differentiator),
//! anisotropic route-frame damping, per-link terrain contacts at the physics collocation
//! stations, wheel-circle + link-chord exclusion, a monotone route-coordinate tube per joint
//! (wrong-side capture unrepresentable), no-restitution velocity reconstruction, and canonical
//! terrain-conformed reseeds on tear/overrun.

use bevy::math::{Affine3A, Vec2, Vec3};

use super::forces::phase_decompose;
use super::oracle::TerrainOracle;
use super::route::{Route, RouteTag, build_route};

/// The chain solver's quality + material parameters. Solver-quality fields (substep, sweeps,
/// guardrails) are global policy; material fields (mass, torque, articulation, thickness) come
/// from the vehicle's TrackSpec. Nothing here is a per-vehicle feel knob.
pub struct ChainParams {
    /// Fixed internal solve step (s) — feel is render-rate independent.
    pub substep: f32,
    /// Catch-up budget: at most this many substeps per rendered frame; a longer hitch reseeds.
    pub max_substeps: usize,
    /// Constraint sweeps per substep.
    pub sweeps: usize,
    /// Damping half-lives (s), anisotropic in the ROUTE frame: tangential motion (yank, slack
    /// migration) barely decays; route-normal motion (flutter) dies fast.
    pub half_life_tan: f32,
    pub half_life_norm: f32,
    /// One link assembly's mass (kg) — real inverse masses in every compliant/limited
    /// constraint, so compliance and friction torque are physical units.
    pub node_mass: f32,
    /// Pin dry-friction torque (N·m): a torque-LIMITED XPBD hinge constraint toward the joint's
    /// previous material angle; multiplier accumulated across sweeps, clamped once per substep.
    pub hinge_torque: f32,
    /// Sprocket motor response time (s).
    pub motor_tau: f32,
    /// Bending regularizer stiffness (N·m²) relative to the route's own curvature — SMALL: a
    /// pinned track has no bending spring away from its stops.
    pub bend_stiffness: f32,
    /// Hard articulation stop between consecutive links (rad).
    pub max_link_angle: f32,
    /// Route-normal stored-velocity guardrail (m/s); tangential caps at max(8, |belt| + 5).
    pub max_normal_speed: f32,
    /// Route-tube half-widths (m): outside the loop / inside it. Both must stay below half the
    /// belly↔return-run route gap so the (s, u) atlas never overlaps.
    pub tube_out: f32,
    pub tube_in: f32,
    /// Windowed route-projection half-width (m) — above the largest legal per-substep motion,
    /// below the distance to any other route branch.
    pub rebase_window: f32,
    /// Track plate thickness (m): pin line → outer face offset for terrain probes.
    pub thickness: f32,
    /// Lateral probe stations across the shoe (m from centerline) — the physics collocation.
    pub lateral_stations: [f32; 3],
    /// Terrain probe reach (m).
    pub probe_reach: f32,
}

/// One side's per-frame inputs.
pub struct ChainSideInput<'a> {
    /// Pin-line circles, front→rear, at their CURRENT (articulated) positions, stably ordered.
    pub circles: &'a [(Vec2, f32)],
    pub belt_speed: f32,
    pub phase: f32,
    /// Lateral offset (m) of this side's plane in hull-local space.
    pub plane_x: f32,
}

/// Per-frame inputs shared by both sides.
pub struct ChainInput<'a> {
    pub dt: f32,
    /// Hull-local → world (the PRESENTED pose in the game; the render transform in the lab).
    pub affine: Affine3A,
    /// Gravity in the hull-local side plane (z, y).
    pub gravity_local: Vec2,
    /// Material loop length (m) and link count — the immutable pitch is `belt_len / count`.
    pub belt_len: f32,
    pub count: usize,
    pub sides: [ChainSideInput<'a>; 2],
}

#[derive(Default)]
struct ChainSide {
    /// Joint positions (side plane), the solved state.
    pos: Vec<Vec2>,
    /// Previous-substep positions (implicit velocity, post reconstruction).
    prev: Vec<Vec2>,
    /// Route coordinate per joint (wrapped) — the tube's warm start and order ledger.
    s: Vec<f32>,
    /// Link-identity shift (total travel / pitch) the stored state corresponds to.
    shift: i64,
    /// Last solved frame's circles, for per-substep interpolation on the fixed clock.
    prev_circles: Vec<(Vec2, f32)>,
}

/// The chain tier's whole state: reset to `Default` for a canonical cold start (view toggle,
/// tier promotion, respawn).
#[derive(Default)]
pub struct ChainState {
    acc: f32,
    sides: [ChainSide; 2],
}

/// What one `step` did — the caller's log/telemetry hook.
#[derive(Default)]
pub struct StepReport {
    /// Fixed substeps executed this frame (per side).
    pub substeps: u32,
    /// Tear-fuse reseeds (NaN / torn links after a solve) — a solver anomaly, worth a warning.
    pub tears: u32,
    /// Clock-overrun reseeds (a frame hitch outlasted the whole catch-up budget) — graceful
    /// degradation, not an anomaly; expected on load/shader-compile hitches.
    pub overruns: u32,
}

impl ChainState {
    /// Advance the fixed clock by `input.dt` and write each side's RENDER positions (side-plane,
    /// interpolated between the last two solved substeps) into `out`.
    pub fn step<O: TerrainOracle>(
        &mut self,
        input: &ChainInput,
        params: &ChainParams,
        oracle: &O,
        out: &mut [Vec<Vec2>; 2],
    ) -> StepReport {
        let mut report = StepReport::default();
        let affine = input.affine;
        // Fixed-clock accumulator. A frame that outruns the whole catch-up budget doesn't drop
        // debt silently: the chain state is stale by more than the budget, so it reseeds.
        let budget = params.substep * params.max_substeps as f32;
        let overrun = self.acc + input.dt > budget + params.substep;
        self.acc = (self.acc + input.dt).min(budget);
        let steps = (self.acc / params.substep) as usize;
        self.acc -= steps as f32 * params.substep;
        report.substeps = steps as u32;
        // Render-time interpolation factor between the last two solved substeps.
        let alpha = (self.acc / params.substep).clamp(0.0, 1.0);
        let h = params.substep;
        let g2 = input.gravity_local;

        let n = input.count;
        if n < 3 {
            return report;
        }
        let pitch = input.belt_len / n as f32;

        for (si, side) in input.sides.iter().enumerate() {
            let circles = side.circles;
            if circles.len() < 2 {
                continue;
            }
            let belt_speed = side.belt_speed;
            // ONE decomposition (the canonical `phase_decompose`) feeds BOTH the resample
            // offset (the seed below) and the whole-pitch link-identity shift (below) — a
            // separately-rounded floor/rem pair could disagree by a link at a pitch boundary.
            let (shift, phase_frac) = phase_decompose(f64::from(side.phase), pitch);
            let plane_x = side.plane_x;
            let mem = &mut self.sides[si];

            // Canonical reseed, entirely data-derived: joints in material order at exact pitch
            // along the route (phase-aligned), each lifted out of terrain (a taut-route seed
            // through an obstacle would be torn apart by the terrain pass next substep — the
            // reseed must land FEASIBLE or it loops), zero velocity, fresh route coordinates.
            let seed = |route: &Route, mem: &mut ChainSide| {
                mem.s = (0..n)
                    .map(|i| route.wrap(phase_frac + i as f32 * pitch))
                    .collect();
                mem.pos = (0..n)
                    .map(|i| {
                        let s = mem.s[i];
                        let q = route.point(s);
                        let tan = route.tangent(s);
                        let out2 = Vec2::new(tan.y, -tan.x);
                        let face = out2 * (params.thickness / 2.0);
                        let w =
                            affine.transform_point3(Vec3::new(plane_x, q.y + face.y, q.x + face.x));
                        let outw = affine
                            .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                            .normalize_or_zero();
                        let d = oracle.depth_along(w, outw, params.probe_reach).max(0.0);
                        q - out2 * d
                    })
                    .collect();
                mem.prev = mem.pos.clone();
            };

            // Wheels move across the frame: each substep sees circles interpolated between last
            // frame's and this frame's positions (the fixed clock owns its inputs — no substep
            // sees one big end-of-frame jump).
            let prev_circles = if mem.prev_circles.len() == circles.len() {
                mem.prev_circles.clone()
            } else {
                circles.to_vec()
            };

            // Material identity: rotate the stored ring (and its route coordinates) when the
            // phase crosses whole pitches (`shift` from the decomposition above).
            let cold = mem.pos.len() != n || mem.s.len() != n;
            if overrun || cold {
                seed(&build_route(circles, input.belt_len), mem);
                // A cold start (empty/resized state) is a normal spawn, not an anomaly — even
                // when the very first frame's dt also trips the overrun budget (a load hitch).
                if overrun && !cold {
                    report.overruns += 1;
                }
            } else {
                let rot = (shift - mem.shift).rem_euclid(n as i64) as usize;
                mem.pos.rotate_right(rot);
                mem.prev.rotate_right(rot);
                mem.s.rotate_right(rot);
            }
            mem.shift = shift;

            let ret_t = (-(std::f32::consts::LN_2 / params.half_life_tan) * h).exp();
            let ret_n = (-(std::f32::consts::LN_2 / params.half_life_norm) * h).exp();
            let motor_gain = (h / params.motor_tau) / (1.0 + h / params.motor_tau);
            // Real inverse mass in every compliant/limited constraint.
            let w_inv = 1.0 / params.node_mass;
            let alpha_tilde = (pitch / params.bend_stiffness) / (h * h);
            // Per-substep friction multiplier bound: |λ| ≤ τ·h², accumulated across sweeps and
            // clamped as a TOTAL (clamping per sweep would multiply the requested torque).
            let friction_cap = params.hinge_torque * h * h;
            // Post-solve velocity guardrails (per substep, as displacements).
            let cap_t = 8.0_f32.max(belt_speed.abs() + 5.0) * h;
            let cap_n = params.max_normal_speed * h;

            for k in 0..steps {
                let f = (k + 1) as f32 / steps as f32;
                let circ: Vec<(Vec2, f32)> = circles
                    .iter()
                    .zip(&prev_circles)
                    .map(|(c, pr)| (pr.0.lerp(c.0, f), c.1))
                    .collect();
                let route = build_route(&circ, input.belt_len);
                let old_pos = mem.pos.clone();
                // Bending rest angles at each joint's own route coordinate, this substep's
                // route (never its array index — index anchoring assigns wrap curvature to
                // drifted joints).
                let theta0: Vec<f32> = mem.s.iter().map(|&s| route.turning(s, pitch)).collect();

                // Each joint's PREVIOUS material angle — the pin friction's stiction target.
                let theta_start: Vec<f32> = (0..n)
                    .map(|i| {
                        let (im, ip) = ((i + n - 1) % n, (i + 1) % n);
                        let e0 = mem.pos[i] - mem.pos[im];
                        let e1 = mem.pos[ip] - mem.pos[i];
                        e0.perp_dot(e1).atan2(e0.dot(e1))
                    })
                    .collect();

                let mut p: Vec<Vec2> = (0..n)
                    .map(|i| {
                        // Anisotropic damping in the ROUTE frame — tangentially centred on the
                        // COMMANDED circulation, not on zero: in the hull frame the correct
                        // steady state IS the whole loop moving at belt speed (ground links
                        // stationary in the world), and damping absolute velocity was a DC
                        // momentum drain across all n joints that the k-joint sprocket motor
                        // could not repay (measured: the loop crawled at ~0.37× belt; wheels,
                        // spun from phase, ran exact — the step-26 scroll-lag bug). Damping
                        // the deviation leaves wave/flutter decay identical and is a no-op at
                        // belt 0; a blocked joint sees only drive·(1−ret_t) ≈ 1% per substep.
                        let tan = route.tangent(mem.s[i]);
                        let nrm = Vec2::new(tan.y, -tan.x);
                        let v = mem.pos[i] - mem.prev[i];
                        let drive = belt_speed * h;
                        let mut vel = tan * (drive + (v.dot(tan) - drive) * ret_t)
                            + nrm * (v.dot(nrm) * ret_n);
                        // Sprocket motor: membership by ROUTE SECTOR (never the disk interior
                        // or a folded node), tangent from the route, rim-distance ramp so
                        // entering the sector is impulse-free.
                        if route.tag(mem.s[i]) == RouteTag::Arc(0) {
                            let rim = (mem.pos[i] - circ[0].0).length() - circ[0].1;
                            let engage = (1.0 - rim.abs() / pitch).clamp(0.0, 1.0);
                            if engage > 0.0 {
                                let v_t = vel.dot(tan);
                                vel += tan * ((belt_speed * h - v_t) * motor_gain * engage);
                            }
                        }
                        mem.pos[i] + vel + g2 * (h * h)
                    })
                    .collect();

                // Terrain contact planes at the CHAIN's OWN predicted positions, refreshed
                // every substep. Per LINK: pin/mid/pin × the lateral stations (the physics
                // collocation), deepest value, linearized along the link's outward normal.
                let contact: Vec<Option<(Vec2, f32)>> = (0..n)
                    .map(|i| {
                        let a = p[i];
                        let b = p[(i + 1) % n];
                        let seg = b - a;
                        let len = seg.length();
                        if len < 1e-4 {
                            return None;
                        }
                        let tan = seg / len;
                        let out2 = Vec2::new(tan.y, -tan.x);
                        let outw = affine
                            .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                            .normalize_or_zero();
                        let axis = affine
                            .transform_vector3(Vec3::new(0.0, tan.y, tan.x))
                            .normalize_or_zero();
                        let lat = outw.cross(axis);
                        let face2 = out2 * (params.thickness / 2.0);
                        let mut d = f32::NEG_INFINITY;
                        for s2 in [a + face2, (a + b) / 2.0 + face2, b + face2] {
                            let w = affine.transform_point3(Vec3::new(plane_x, s2.y, s2.x));
                            for offset in params.lateral_stations {
                                d = d.max(oracle.depth_along(
                                    w + lat * offset,
                                    outw,
                                    params.probe_reach,
                                ));
                            }
                        }
                        // Keep nearly-clear planes too: sweeps move joints and must not tunnel
                        // past a boundary probed as barely clear.
                        (d > -0.08).then_some((out2, d))
                    })
                    .collect();
                let p_start = p.clone();
                // XPBD multipliers — scratch per substep. `fired` records which contact planes
                // actually pushed, for the no-restitution velocity reconstruction below.
                let mut lambda = vec![0.0_f32; n];
                let mut lambda_f = vec![0.0_f32; n];
                let mut fired = vec![false; n];
                for _ in 0..params.sweeps {
                    // (a) Rigid link lengths (zero compliance).
                    for i in 0..n {
                        let j = (i + 1) % n;
                        let d = p[j] - p[i];
                        let l = d.length();
                        if l < 1e-6 {
                            continue;
                        }
                        let shift = d * ((l - pitch) / l * 0.5);
                        p[i] += shift;
                        p[j] -= shift;
                    }
                    // (a2) Sprocket TOOTH LOCK — positional, the real mechanism: a pin seated
                    // in a tooth gap is kinematically at its material station, it cannot slip.
                    // Joints on the drive arc are pulled TANGENTIALLY toward
                    // `phase + i·pitch` (the seed convention — material identity), engage-
                    // ramped by rim distance and capped per sweep. The velocity motor above
                    // remains as the warm start; without this positional anchor the loop
                    // circulated at only ~0.63× belt post-damping-fix (Gauss-Seidel length
                    // sweeps diffuse the sector's velocity too slowly around the loop, and
                    // pin friction eats the rest — measured step 26).
                    #[allow(clippy::needless_range_loop)] // i is the MATERIAL index (s target)
                    for i in 0..n {
                        if route.tag(mem.s[i]) != RouteTag::Arc(0) {
                            continue;
                        }
                        let rim = (p[i] - circ[0].0).length() - circ[0].1;
                        let engage = (1.0 - rim.abs() / pitch).clamp(0.0, 1.0);
                        if engage <= 0.0 {
                            continue;
                        }
                        let s_t = route.wrap(phase_frac + i as f32 * pitch);
                        // Only lock joints whose material station IS on the drive arc — a
                        // drifted joint radially near the rim but materially past the wrap
                        // must not be yanked back onto it.
                        if route.tag(s_t) != RouteTag::Arc(0) {
                            continue;
                        }
                        let tan = route.tangent(s_t);
                        let err = (route.point(s_t) - p[i]).dot(tan);
                        let corr = err.clamp(-0.25 * pitch, 0.25 * pitch) * engage;
                        p[i] += tan * corr;
                    }
                    // (b) XPBD bending regularizer: C = θ − θ0, real compliance.
                    for i in 0..n {
                        let (im, ip) = ((i + n - 1) % n, (i + 1) % n);
                        let e0 = p[i] - p[im];
                        let e1 = p[ip] - p[i];
                        let (l0, l1) = (e0.length_squared(), e1.length_squared());
                        if l0 < 1e-9 || l1 < 1e-9 {
                            continue;
                        }
                        let theta = e0.perp_dot(e1).atan2(e0.dot(e1));
                        let mut c = theta - theta0[i];
                        if c > std::f32::consts::PI {
                            c -= std::f32::consts::TAU;
                        } else if c < -std::f32::consts::PI {
                            c += std::f32::consts::TAU;
                        }
                        let g_prev = e0.perp() / l0;
                        let g_next = e1.perp() / l1;
                        let g_mid = -(g_prev + g_next);
                        let denom = w_inv
                            * (g_prev.length_squared()
                                + g_mid.length_squared()
                                + g_next.length_squared())
                            + alpha_tilde;
                        let dl = (-c - alpha_tilde * lambda[i]) / denom;
                        lambda[i] += dl;
                        p[im] += g_prev * (w_inv * dl);
                        p[i] += g_mid * (w_inv * dl);
                        p[ip] += g_next * (w_inv * dl);
                    }
                    // (b2) Pin DRY friction: torque-limited stiction toward the previous
                    // material angle — flutter is held, the sprocket wrap and tension slip
                    // through at bounded resistance.
                    for i in 0..n {
                        let (im, ip) = ((i + n - 1) % n, (i + 1) % n);
                        let e0 = p[i] - p[im];
                        let e1 = p[ip] - p[i];
                        let (l0, l1) = (e0.length_squared(), e1.length_squared());
                        if l0 < 1e-9 || l1 < 1e-9 {
                            continue;
                        }
                        let theta = e0.perp_dot(e1).atan2(e0.dot(e1));
                        let mut c = theta - theta_start[i];
                        if c > std::f32::consts::PI {
                            c -= std::f32::consts::TAU;
                        } else if c < -std::f32::consts::PI {
                            c += std::f32::consts::TAU;
                        }
                        let g_prev = e0.perp() / l0;
                        let g_next = e1.perp() / l1;
                        let g_mid = -(g_prev + g_next);
                        let denom = w_inv
                            * (g_prev.length_squared()
                                + g_mid.length_squared()
                                + g_next.length_squared());
                        if denom < 1e-9 {
                            continue;
                        }
                        let want = lambda_f[i] - c / denom;
                        let clamped = want.clamp(-friction_cap, friction_cap);
                        let dl = clamped - lambda_f[i];
                        lambda_f[i] = clamped;
                        if dl == 0.0 {
                            continue;
                        }
                        p[im] += g_prev * (w_inv * dl);
                        p[i] += g_mid * (w_inv * dl);
                        p[ip] += g_next * (w_inv * dl);
                    }
                    // (c) Signed hinge stop — the hard link-geometry limit.
                    for i in 0..n {
                        let (im, ip) = ((i + n - 1) % n, (i + 1) % n);
                        let e0 = p[i] - p[im];
                        let e1 = p[ip] - p[i];
                        let (l0, l1) = (e0.length_squared(), e1.length_squared());
                        if l0 < 1e-9 || l1 < 1e-9 {
                            continue;
                        }
                        let theta = e0.perp_dot(e1).atan2(e0.dot(e1));
                        let c = theta - theta.clamp(-params.max_link_angle, params.max_link_angle);
                        if c == 0.0 {
                            continue;
                        }
                        let g_prev = e0.perp() / l0;
                        let g_next = e1.perp() / l1;
                        let g_mid = -(g_prev + g_next);
                        let denom = g_prev.length_squared()
                            + g_mid.length_squared()
                            + g_next.length_squared();
                        let dl = -c / denom;
                        p[im] += g_prev * dl;
                        p[i] += g_mid * dl;
                        p[ip] += g_next * dl;
                    }
                    // (d) Terrain: pinch discipline — a SATURATED probe is a bounded contact
                    // signal, not a positional correction (skip; the tear fuse judges the
                    // state); corrections cap at half a pitch; and terrain YIELDS to wheels
                    // (never alternate two projectors on an empty feasible set).
                    for (i, c) in contact.iter().enumerate() {
                        let Some((out2, d)) = *c else {
                            continue;
                        };
                        if d >= params.probe_reach - 1e-3 {
                            continue;
                        }
                        let v = (d + (p[i] - p_start[i]).dot(out2)).min(0.5 * pitch);
                        if v <= 0.0 {
                            continue;
                        }
                        let cand = p[i] - out2 * v;
                        if circ.iter().any(|&(c, r)| cand.distance_squared(c) < r * r) {
                            continue;
                        }
                        p[i] = cand;
                        fired[i] = true;
                    }
                    // (e) Wheel circles: nearest-exit for joints, plus LINK-CHORD exclusion —
                    // two clear pins can still chord through a wheel at arc/tangent handoffs.
                    for pt in p.iter_mut() {
                        for &(c, r) in &circ {
                            let d = *pt - c;
                            let l = d.length();
                            if l < r && l > 1e-6 {
                                *pt = c + d * (r / l);
                            }
                        }
                    }
                    for i in 0..n {
                        let j = (i + 1) % n;
                        let mid = (p[i] + p[j]) / 2.0;
                        for &(c, r) in &circ {
                            let d = mid - c;
                            let l = d.length();
                            if l < r && l > 1e-6 {
                                let lift = d * (r / l) + c - mid;
                                p[i] += lift;
                                p[j] += lift;
                            }
                        }
                    }
                    // (f) Route tube: rebase every joint to its windowed route coordinate —
                    // monotone, so material order can't reshuffle — and clamp its normal offset
                    // to the tube. On a wheel arc the inner bound is ZERO: radially off the rim
                    // is the only way out, so wrong-side capture is unrepresentable; and a
                    // joint ends every sweep within `tube_out` of the route — never off the
                    // vehicle, whatever the projections above did.
                    let total = route.total();
                    let mut prev_s = f32::NAN;
                    #[allow(clippy::needless_range_loop)] // i indexes p, s, AND the order state
                    for i in 0..n {
                        let (mut s_i, mut u) = route.project(p[i], mem.s[i], params.rebase_window);
                        if i > 0 {
                            let gap = (s_i - prev_s).rem_euclid(total);
                            if !(0.2 * pitch..=2.0 * pitch).contains(&gap) {
                                let clamped = if gap > total * 0.5 {
                                    0.2 * pitch // projected behind its predecessor
                                } else {
                                    gap.clamp(0.2 * pitch, 2.0 * pitch)
                                };
                                s_i = route.wrap(prev_s + clamped);
                                // The offset is re-measured at the corrected coordinate.
                                let q = route.point(s_i);
                                let tan = route.tangent(s_i);
                                u = (p[i] - q).dot(Vec2::new(tan.y, -tan.x));
                            }
                        }
                        let (u_min, u_max) = match route.tag(s_i) {
                            RouteTag::Arc(_) => (0.0, params.tube_out),
                            RouteTag::Span => (-params.tube_in, params.tube_out),
                        };
                        if u < u_min || u > u_max {
                            let q = route.point(s_i);
                            let tan = route.tangent(s_i);
                            let outn = Vec2::new(tan.y, -tan.x);
                            p[i] = q + outn * u.clamp(u_min, u_max);
                        }
                        mem.s[i] = s_i;
                        prev_s = s_i;
                    }
                    // (g) Closing length pass: the projections above must not bank pitch error
                    // (exact total length IS the tension model).
                    for i in 0..n {
                        let j = (i + 1) % n;
                        let d = p[j] - p[i];
                        let l = d.length();
                        if l < 1e-6 {
                            continue;
                        }
                        let shift = d * ((l - pitch) / l * 0.5);
                        p[i] += shift;
                        p[j] -= shift;
                    }
                }
                // Tear fuse: a substep that still ends with NaNs or torn links reseeds
                // canonically — the chain may visibly pop back onto the route once, but it can
                // never leave the vehicle or poison the next frame.
                let torn = p.iter().any(|q| !q.is_finite())
                    || (0..n)
                        .any(|i| ((p[(i + 1) % n] - p[i]).length() - pitch).abs() > 0.25 * pitch);
                if torn {
                    seed(&route, mem);
                    report.tears += 1;
                    continue;
                }
                // Velocity reconstruction — THE pinch fix: `prev = old_pos` would turn every
                // unilateral depenetration into Verlet restitution velocity. Bilateral
                // responses (lengths, motor, bending) keep their velocity; an ACTIVE terrain
                // contact keeps only its pre-projection escape velocity and never gains inward
                // motion; wheels zero inward radial motion; anisotropic route-frame guardrails
                // cap what's stored.
                for i in 0..n {
                    let mut dp = p[i] - old_pos[i];
                    if fired[i]
                        && let Some((out2, _)) = contact[i]
                    {
                        let away = -out2;
                        let pre = (p_start[i] - old_pos[i]).dot(away);
                        dp += away * (pre.max(0.0) - dp.dot(away));
                    }
                    for &(c, r) in &circ {
                        let rad = p[i] - c;
                        let l = rad.length();
                        if l < r + 0.02 && l > 1e-6 {
                            let r_hat = rad / l;
                            let vr = dp.dot(r_hat);
                            if vr < 0.0 {
                                dp -= r_hat * vr;
                            }
                        }
                    }
                    let tan = route.tangent(mem.s[i]);
                    let nrm = Vec2::new(tan.y, -tan.x);
                    dp = tan * dp.dot(tan).clamp(-cap_t, cap_t)
                        + nrm * dp.dot(nrm).clamp(-cap_n, cap_n);
                    mem.prev[i] = p[i] - dp;
                }
                mem.pos = p;
            }
            if steps > 0 {
                mem.prev_circles = circles.to_vec();
            }

            // Render output: interpolate between the last two solved substeps — the
            // accumulator remainder says exactly how far render time sits past the last solve.
            out[si] = if mem.prev.len() == n {
                (0..n)
                    .map(|i| mem.prev[i].lerp(mem.pos[i], alpha))
                    .collect()
            } else {
                mem.pos.clone()
            };
        }
        report
    }
}
