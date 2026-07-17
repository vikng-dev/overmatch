//! Field-belt model: model-3's pin-line chain with terrain contact read from a **deterministic
//! analytic field** instead of narrow-phase queries.
//!
//! The terrain oracle is a rounded-box SDF union over the course's authored blocks
//! ([`TerrainField`], filled by `spawn_environment`). Per link, penetration is evaluated at
//! **fixed link-local collocation stations** (the two pins + the midpoint, on the outer face) and
//! fed to the same closed-form pressure profile as models 2/3. There is no witness point, no
//! tie-breaking, and no collision engine anywhere in the loop: depth is a pure fixed-order
//! arithmetic function of pose — pose-continuous (C0) and bit-deterministic by construction (the
//! contact-oracle research verdict; see
//! `.agents/docs/design/track-model/contact-oracle-research.md`).
//!
//! The field is **rounded** ([`FIELD_ROUNDING`]): box edges in the SDF turn instead of snapping,
//! so normals and depths stay smooth as links cross bump corners — the "round the field, not the
//! mesh" hardening (Drake margin / Jolt active-edge lesson), and the candidate cure for the
//! washboard slap-down. Wheel/terrain face offsets and drivetrain: model 3's unchanged.
//!
//! **Width** ([`TRACK_WIDTH`]) enters as three lateral **columns** (the true shoe edges at
//! ±[`COLUMN_OFFSET`] + the centerline, Simpson-weighted — see [`COLUMNS`]): each column samples
//! its own three stations, owns its share of the per-metre coefficients, and applies its
//! resultant at its own point — curb-under-one-edge roll torque, cross-slope contact, and
//! half-off-a-ledge support emerge from the application points.
//!
//! The track **view** is a stateless kinematic wrap (step 22): the road wheels read the field
//! directly ([`articulate_wheels_field`]), the belt path is *fitted* around the articulated
//! wheels every frame ([`conform_belts_field`]) — tangent wrap + terrain conform + budgeted sag —
//! and nothing about the drawn track is simulated or remembered. The step-21 Verlet chain remains
//! behind the `V` toggle as the frozen A/B partner ([`conform_belts_field_chain`]).

use super::model2::{ChainSideMemory, clipped_linear_piece};
use super::model3::{PinBelt, TRACK_THICKNESS, pin_circles};
use super::*;

/// Edge rounding radius (m) of the terrain field: every authored box is evaluated as a rounded
/// box (core shrunk by this, surface pushed back out), so the union's surface is C1 across box
/// edges at the cost of visually-invisible 3 cm corner rounding. Must stay below the smallest
/// authored half-extent (washboard bump half-height 0.06).
const FIELD_ROUNDING: f32 = 0.03;

/// Shoe (link) width (m): the T-34's 500 mm plate.
const TRACK_WIDTH: f32 = 0.5;

/// Lateral offset of the edge columns from the track centerline: the TRUE shoe edges (±w/2).
/// These are point samples — there is no lateral query radius — so anything short of ±0.25
/// leaves a blind rim at the shoe edge (the step-20 value (w − t)/2 = ±0.23 borrowed the pill's
/// cast radius as reach the field oracle doesn't have; codex finding, step 21c).
const COLUMN_OFFSET: f32 = TRACK_WIDTH / 2.0;

/// Edge-column weight, solved so the edge pair reproduces a laterally-UNIFORM pressure strip's
/// second moment exactly: `2·w_e·off² = w²/12` → with the columns at ±w/2 this is exactly 1/6
/// (Simpson weights, fittingly). Three columns give: exact total load, exact uniform-strip roll
/// stiffness, detection to the true shoe edges, and a mid-track detection row — the lateral
/// sampling gap is 0.25 m.
const EDGE_WEIGHT: f32 = (TRACK_WIDTH * TRACK_WIDTH / 12.0) / (2.0 * COLUMN_OFFSET * COLUMN_OFFSET);

/// The lateral columns: (offset along the link's lateral axis, share of the per-metre
/// coefficients). Weights sum to 1 — flat-ground totals are exactly the single-column model's.
const COLUMNS: [(f32, f32); 3] = [
    (-COLUMN_OFFSET, EDGE_WEIGHT),
    (0.0, 1.0 - 2.0 * EDGE_WEIGHT),
    (COLUMN_OFFSET, EDGE_WEIGHT),
];

// Route-chain solve knobs (step 23, from the codex chain deep dive — every knob has physical
// units; per-frame damping factors, per-frame pass counts, and stiffness-by-iteration are gone).
/// Fixed internal solve step (s): the chain advances on its OWN clock via a frame-time
/// accumulator, so feel is identical at 30/60/144 fps (the old 0.88-per-frame damping + 20
/// passes-per-frame was "three different chains" across render rates).
const CHAIN_SUBSTEP: f32 = 1.0 / 120.0;
/// Catch-up budget: at most this many substeps per rendered frame; longer hitches drop debt
/// instead of integrating a monster step.
const CHAIN_MAX_SUBSTEPS: usize = 8;
/// Constraint sweeps per substep (many small steps beat many sweeps in one big step — XPBD
/// "small steps" result).
const CHAIN_SWEEPS: usize = 4;
/// Damping as real-time half-lives (s), ANISOTROPIC in the route frame (step 24, codex T-34
/// review): isotropic drag is rope physics — it kills the longitudinal yank along with the
/// flutter. Tangential motion (yank, slack migration) barely decays; route-normal motion
/// (transverse flutter) dies fast. The other half of transverse deadness is the pin friction.
const CHAIN_HALF_LIFE_TAN: f32 = 0.60;
const CHAIN_HALF_LIFE_NORM: f32 = 0.060;
/// Node mass (kg): one T-34 link assembly — ~16 kg cast shoe + its share of pin hardware
/// (~1.15 t per 72-link track). Enters the XPBD denominators (w = 1/m), which makes the bending
/// compliance and the friction torque REAL units instead of normalized view parameters.
const CHAIN_NODE_MASS: f32 = 16.0;
/// Pin dry-friction torque (N·m): μ≈0.15 on a ~12 mm pin under 10–50 kN tension gives ~18–90;
/// 25 is the unloaded starting point. Implemented as a torque-LIMITED XPBD hinge constraint
/// toward the joint's previous material angle, multiplier accumulated across sweeps and clamped
/// once per substep (|λ| ≤ τ·h²). This is the physical rope-vs-track differentiator: real track
/// pins are heavily-loaded dry steel bearings — flutter dies within a link or two and slack
/// settles near-polygonal, while bulk yank passes through because it doesn't articulate joints.
const CHAIN_HINGE_TORQUE: f32 = 25.0;
/// Belt length trimmed off the chain view's loop (m) — the tensioner PRELOAD. The T-34 manual
/// spec is ~30–50 mm of return-run sag between wheel tops when correctly tensioned; of the
/// authored 0.13 m TRACK_SLACK this trim leaves ~0.02 m, which drapes to ~40 mm over the ~0.8 m
/// top spans. (Strictly a tensioner consumes ROUTE length, not material length — this shortens
/// links by a cosmetically-nil 0.8%; the honest idler-shift version is parked.)
const CHAIN_SLACK_TRIM: f32 = 0.11;
/// Sprocket motor response time (s): how fast joints engaged on the drive wheel converge to the
/// belt's surface speed. Drive is applied ONLY there — the old all-joint advected anchor
/// injected compression around the whole loop and was itself a zigzag cause (codex, step 22b);
/// the length constraints now transmit drive, so tight and slack sides emerge.
const CHAIN_MOTOR_TAU: f32 = 0.05;
/// Bending stiffness (N·m², REAL units now that node mass is real) of the XPBD turning-angle
/// constraint relative to the route's own curvature. Small on purpose: a pinned track has no
/// bending spring away from its stops — the old normalized B=10 with unit masses was secretly
/// ~160 N·m² of route-shaped spring (part of the rubber-band read). This is a numerical
/// regularizer; the anti-zigzag/anti-flutter duty moved to the pin friction + the route tube.
const CHAIN_BEND_STIFFNESS: f32 = 2.0;
/// Max articulation between consecutive links (rad): must clear the T-34 sprocket's wrap demand
/// of ~31°/joint (see model 2). A hard link-geometry stop, distinct from the bending energy.
const MAX_LINK_ANGLE: f32 = 35.0 * std::f32::consts::PI / 180.0;
/// Post-solve velocity guardrails (m/s), decomposed in the route frame: route-normal speed caps
/// hard (whip is real but bounded); tangential caps at max(8, |belt| + 5) computed inline. These
/// clamp the STORED velocity after reconstruction — containment, not the root fix (that's the
/// no-restitution reconstruction below).
const CHAIN_MAX_NORMAL_SPEED: f32 = 4.0;
/// Route-tube half-widths (m): how far a joint may sit OUTSIDE the loop (whip overshoot) and
/// INSIDE it (terrain holds the belly a board-stack in off the taut line; slack droops under
/// spans). Both stay below half the belly↔top-run route gap (~0.85 m) so the tube atlas never
/// overlaps — one 2D point, one (s,u). A joint clamped to the tube can never be "off the tank"
/// no matter what the solve did — and on wheel arcs the inner bound is zero, which is what makes
/// wrong-side capture UNREPRESENTABLE (codex Priority B): a node on a wheel sector can only move
/// radially off the rim.
const CHAIN_TUBE_OUT: f32 = 0.30;
const CHAIN_TUBE_IN: f32 = 0.40;
/// Half-width (m) of the windowed route-projection search around a joint's previous route
/// coordinate — ±2 pitches: comfortably above the largest legal per-substep motion (~0.17 m),
/// far below the distance to any other route branch. A window (not a global nearest-point
/// query) is what keeps the rebase from tunneling `s` across overlapping parts of the loop.
const CHAIN_REBASE_WINDOW: f32 = 0.35;

/// How far (m) every field box's bottom is extended below its authored extent. A raised block
/// resting on the ground (washboard board, step) would otherwise carry an interior union seam:
/// past mid-height the `min()`-union's nearest surface flips from the block's top face to its
/// BURIED bottom face, depth shrinks as the belt sinks further, force collapses, and the belt
/// swallows the obstacle (the step-19 "washboard ignored" bug). With the bottom pushed far below
/// any reachable depth, depth below a top face grows monotonically until it plateaus at the
/// box's side-face distance (a bounded softness on thin features — parked; see HQ).
const FIELD_BURY: f32 = 2.0;

/// One authored terrain block in the analytic field (world-space oriented box).
pub(super) struct FieldBox {
    center: Vec3,
    /// World→box rotation (the block's rotation inverted; identity for everything but the ramp).
    inv_rot: Quat,
    half: Vec3,
}

impl FieldBox {
    /// Build from an authored block transform (unit cube scaled by `transform.scale`), the bottom
    /// extended by [`FIELD_BURY`] along the block's local −Y (the top surface is untouched).
    pub(super) fn from_block(transform: &Transform) -> Self {
        Self {
            center: transform.translation - transform.rotation * Vec3::Y * (FIELD_BURY / 2.0),
            inv_rot: transform.rotation.inverse(),
            half: transform.scale / 2.0 + Vec3::Y * (FIELD_BURY / 2.0),
        }
    }

    /// Exact first-hit distance (t ≥ 0) of a ray with this ROUNDED box, or `None` on a miss. The
    /// rounded box is the Minkowski sum of the shrunken core and a [`FIELD_ROUNDING`] sphere, so
    /// its exact surface decomposes into: 3 face slabs (the core expanded by the rounding along
    /// one axis each), 12 edge cylinders, and 8 corner spheres — the union's entry is the min of
    /// the primitive entries. Assumes the origin is outside the box (the caller checks the union's
    /// SDF once); closed-form quadratics only, so grazing rays get the exact answer the
    /// sphere-trace march could stall on.
    fn ray_hit(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let r = FIELD_ROUNDING;
        let core = (self.half - Vec3::splat(r)).max(Vec3::splat(1e-3));
        let o = self.inv_rot * (origin - self.center);
        let d = self.inv_rot * dir;

        // Cheap reject: the box inflated by the rounding bounds the whole rounded shape.
        ray_box(o, d, core + Vec3::splat(r))?;

        let mut best = f32::INFINITY;
        // (a) The three face slabs.
        for axis in 0..3 {
            let mut ext = core;
            ext[axis] += r;
            if let Some(t) = ray_box(o, d, ext) {
                best = best.min(t);
            }
        }
        // (b) The twelve edge cylinders: radius r around each core edge, hits accepted only
        // within the edge's axial extent (entries through a cylinder's end cap are inside the
        // corner sphere that covers it, so caps need no test of their own).
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for su in [-1.0_f32, 1.0] {
                for sv in [-1.0_f32, 1.0] {
                    let oc = Vec2::new(o[u] - su * core[u], o[v] - sv * core[v]);
                    let dc = Vec2::new(d[u], d[v]);
                    if let Some(t) = ray_circle(oc, dc, r)
                        && (o[axis] + d[axis] * t).abs() <= core[axis]
                    {
                        best = best.min(t);
                    }
                }
            }
        }
        // (c) The eight corner spheres.
        for sx in [-1.0_f32, 1.0] {
            for sy in [-1.0_f32, 1.0] {
                for sz in [-1.0_f32, 1.0] {
                    let c = Vec3::new(sx * core.x, sy * core.y, sz * core.z);
                    if let Some(t) = ray_sphere(o - c, d, r) {
                        best = best.min(t);
                    }
                }
            }
        }
        (best < f32::INFINITY).then_some(best)
    }
}

/// Entry distance of a ray into an axis-aligned box of half-extents `ext` (slab test), if it hits
/// at t ≥ 0. An origin inside returns 0.
fn ray_box(o: Vec3, d: Vec3, ext: Vec3) -> Option<f32> {
    let (mut t0, mut t1) = (0.0_f32, f32::INFINITY);
    for axis in 0..3 {
        if d[axis].abs() < 1e-9 {
            if o[axis].abs() > ext[axis] {
                return None;
            }
        } else {
            let inv = 1.0 / d[axis];
            let (ta, tb) = ((-ext[axis] - o[axis]) * inv, (ext[axis] - o[axis]) * inv);
            t0 = t0.max(ta.min(tb));
            t1 = t1.min(ta.max(tb));
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

/// Entry distance of a 2D ray into a circle of radius `r` at the origin, if it enters from
/// OUTSIDE at t ≥ 0. An origin already inside returns `None` — for the edge-cylinder use, such a
/// ray can only enter the finite cylinder through an end cap, which the corner spheres cover.
fn ray_circle(o: Vec2, d: Vec2, r: f32) -> Option<f32> {
    let a = d.length_squared();
    if a < 1e-12 {
        return None;
    }
    let b = o.dot(d);
    let c = o.length_squared() - r * r;
    if c <= 0.0 {
        return None;
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / a;
    (t >= 0.0).then_some(t)
}

/// Entry distance of a ray into a sphere of radius `r` at the origin (`o` = ray origin relative to
/// the sphere center), if it enters from outside at t ≥ 0.
fn ray_sphere(o: Vec3, d: Vec3, r: f32) -> Option<f32> {
    let a = d.length_squared();
    if a < 1e-12 {
        return None;
    }
    let b = o.dot(d);
    let c = o.length_squared() - r * r;
    if c <= 0.0 {
        return None;
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / a;
    (t >= 0.0).then_some(t)
}

/// The analytic terrain oracle: every block `spawn_environment` lays down, as data. The course's
/// physics colliders and this field are built from the same transforms, so the two
/// representations cannot drift.
#[derive(Resource, Default)]
pub(super) struct TerrainField(pub(super) Vec<FieldBox>);

impl TerrainField {
    /// Signed distance (m) from `p` to the terrain surface: negative inside. Union = min over
    /// blocks; C0 everywhere, C1 except on inter-block Voronoi seams.
    fn sdf(&self, p: Vec3) -> f32 {
        self.0
            .iter()
            .map(|b| box_sdf(p, b))
            .fold(f32::INFINITY, f32::min)
    }

    /// Signed EUCLIDEAN penetration of `p` (nearest-surface distance): positive inside. Kept for
    /// the harness's field scans and the viz stations; the physics reads [`Self::depth_along`] —
    /// Euclidean depth under a raised block plateaus at the block's side-face distance (the
    /// 19b/19c "fine washboard too soft" defect).
    pub(super) fn signed_depth(&self, p: Vec3) -> f32 {
        (-self.sdf(p)).min(CONTACT_PROBE)
    }

    /// Signed DIRECTIONAL penetration of `station` past the first terrain surface along `out` —
    /// the cast models' semantics, evaluated against the field: an **exact analytic first hit** of
    /// the ray (from `CONTACT_PROBE` behind the station) against each rounded box; depth =
    /// probe − nearest hit. The union's entry is the min over per-box entries — closed form, no
    /// iteration (the step-21c sphere-trace march needed an exhaustion fallback on grazing rays
    /// whose hybrid answer was discontinuous at the convergence boundary; the exact hit deletes
    /// the whole failure class — step-22 review). Unbounded through stacked geometry (the ray
    /// enters via the TOP face — no side-face plateau), positive = past the surface (buried
    /// origin = fully saturated, like the casts), negative = clearance. Lateral roll-off at block
    /// edges comes from the field's rounding, and the tangent-graze branch jump of any first-hit
    /// query happens at zero depth on a rounded surface — the same reason the pill cast was
    /// smooth. Deterministic: fixed evaluation order, pure arithmetic.
    pub(super) fn depth_along(&self, station: Vec3, out: Vec3) -> f32 {
        // Anything past one probe beyond the station is deep clearance — the profile only needs
        // the sign + slope there.
        let t_max = 2.0 * CONTACT_PROBE;
        let origin = station - out * CONTACT_PROBE;
        if self.sdf(origin) <= 0.0 {
            return CONTACT_PROBE;
        }
        let t = self
            .0
            .iter()
            .filter_map(|b| b.ray_hit(origin, out))
            .fold(t_max, f32::min);
        CONTACT_PROBE - t
    }
}

/// Quilez rounded-box SDF: exact distance on faces (the flat-ground answer is identical to the
/// cast models'), rounded by [`FIELD_ROUNDING`] at edges/corners.
fn box_sdf(p: Vec3, b: &FieldBox) -> f32 {
    let core = (b.half - Vec3::splat(FIELD_ROUNDING)).max(Vec3::splat(1e-3));
    let q = (b.inv_rot * (p - b.center)).abs() - core;
    q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - FIELD_ROUNDING
}

/// MODEL 4 belt contact — model 3's advected pin-line ring, penetration from the field at three
/// fixed stations per link (pin a, midpoint, pin b — on the outer face), profile and force
/// machinery unchanged:
///
/// - the two-piece linear profile between the stations replaces the cast's (pen_max, x_c) apex —
///   the interior is interpolated instead of searched, so there is nothing to tie-break;
/// - stations are signed (clearance below zero), so the profile's closed-form clipping still
///   finds the lift-off point between stations;
/// - support + traction applied at the profile centroid on the terrain surface, exactly as
///   model 3 (`+ out·(t/2 − pen_c)`).
pub(super) fn apply_belt_support_field(
    mut hull: Query<(&GlobalTransform, Forces), With<Hull>>,
    field: Res<TerrainField>,
    input: Res<DriveInput>,
    time: Res<Time>,
    pin_belt: Res<PinBelt>,
    mut belt: ResMut<BeltSpeed>,
    mut phase: ResMut<BeltPhase>,
    mut contacts: ResMut<BeltContacts>,
) {
    let Ok((hull_gt, mut forces)) = hull.single_mut() else {
        return;
    };
    let affine = hull_gt.affine();
    let to_local = affine.inverse();
    contacts.0.clear(); // the sole contact system this tick
    let dt = time.delta_secs();

    for side in [Side::Left, Side::Right] {
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        let command = match side {
            Side::Left => input.throttle + input.steer,
            Side::Right => input.throttle - input.steer,
        }
        .clamp(-1.0, 1.0);
        let belt_speed = belt.get(side);
        let mut belt_reaction = 0.0;

        // The fixed advected ring on the pin line (see model 3).
        let mut loop_pts = belt_loop(&pin_circles(), None);
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / pin_belt.count.max(1) as f32;
        let mut stations = resample(&loop_pts, pitch, phase.get(side).rem_euclid(pitch));
        stations.truncate(pin_belt.count);
        let n = stations.len();
        if n < 3 {
            continue;
        }

        for i in 0..n {
            let a = stations[i];
            let b = stations[(i + 1) % n];
            let seg = b - a;
            let len = seg.length();
            if len < 1e-4 {
                continue;
            }
            let tan2 = seg / len;
            let out2 = Vec2::new(tan2.y, -tan2.x);

            let wa = affine.transform_point3(Vec3::new(track_x, a.y, a.x));
            let wb = affine.transform_point3(Vec3::new(track_x, b.y, b.x));
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let axis = (wb - wa) / len;
            let lat = out.cross(axis);
            let face = out * (TRACK_THICKNESS / 2.0);

            // WIDTH: the 500 mm shoe is sampled as three lateral COLUMNS (edges + center; see
            // [`COLUMNS`] — positions set the detection Nyquist, weights match the uniform
            // strip's load AND roll moments exactly). Each column runs the full profile
            // machinery on its own three stations with its weight of the per-metre
            // coefficients and applies its resultant at its own point — roll torque from a
            // curb under one track edge, cross-slope contact, and half-off-a-ledge support
            // all emerge from the application points.
            for (offset, weight) in COLUMNS {
                let shift = lat * offset;
                let ca = wa + shift;
                let cb = wb + shift;

                // The three collocation stations, on the outer face; depth along the link's own
                // outward normal (the cast semantics — see `depth_along`).
                let pen_a = field.depth_along(ca + face, out);
                let pen_m = field.depth_along((ca + cb) / 2.0 + face, out);
                let pen_b = field.depth_along(cb + face, out);
                let pen_max = pen_a.max(pen_m).max(pen_b);
                if pen_max <= 0.0 {
                    continue;
                }

                let (a1, m1, l1) = clipped_linear_piece(0.0, len / 2.0, pen_a, pen_m);
                let (a2, m2, l2) = clipped_linear_piece(len / 2.0, len, pen_m, pen_b);
                let (area, moment, contact_len) = (a1 + a2, m1 + m2, l1 + l2);
                if area <= 0.0 {
                    continue;
                }
                // Resultant at the terrain surface, on this column: the profile's own value at
                // the centroid position. (Model 3's `(pen_a+pen_max)/2` ignored pen_b and moved
                // the traction point ±5 cm under mirroring — codex finding, step 21c. The normal
                // force is offset-invariant along its own line; the traction lever is not.)
                let x_c = moment / area;
                let pen_c = if x_c <= len / 2.0 {
                    pen_a + (pen_m - pen_a) * (x_c / (len / 2.0))
                } else {
                    pen_m + (pen_b - pen_m) * ((x_c - len / 2.0) / (len / 2.0))
                }
                .max(0.0);
                let p = ca + axis * x_c + out * (TRACK_THICKNESS / 2.0 - pen_c);

                // (1) Support: penalty spring along the belt's own inward normal (see model
                // 1/2), at the column's share of the per-metre coefficients.
                let normal = -out;
                let vel = forces.velocity_at_point(p);
                let engage = (pen_max / CONTACT_ENGAGE).clamp(0.0, 1.0);
                let load = weight
                    * (SUPPORT_STIFFNESS_PER_M * area
                        - SUPPORT_DAMPING_PER_M * contact_len * vel.dot(normal))
                    .max(0.0)
                    * engage;
                if load <= 0.0 {
                    continue;
                }
                forces.apply_force_at_point(normal * load, p);

                // (2) Traction: slip-saturated friction on the ellipse (see model 1/2); grip
                // scales with the column's (halved) load.
                let mut slip_long = 0.0;
                let mut traction = Vec3::ZERO;
                let drive = -affine.transform_vector3(Vec3::new(0.0, tan2.y, tan2.x));
                let long_plane = drive - drive.dot(normal) * normal;
                if long_plane.length() > 1e-4 {
                    let long_dir = long_plane.normalize();
                    let lat_dir = normal.cross(long_dir).normalize_or_zero();
                    slip_long = belt_speed - vel.dot(long_dir);
                    let s_lat = vel.dot(lat_dir);
                    let grip = MU * load;
                    let grip_lat = grip * LATERAL_GRIP_RATIO;
                    let mut f_long = grip * (slip_long / SLIP_SATURATION).clamp(-1.0, 1.0);
                    let mut f_lat = -grip_lat * (s_lat / SLIP_SATURATION).clamp(-1.0, 1.0);
                    let e = (f_long / grip).powi(2) + (f_lat / grip_lat).powi(2);
                    if e > 1.0 {
                        let s = e.sqrt().recip();
                        f_long *= s;
                        f_lat *= s;
                    }
                    traction = long_dir * f_long + lat_dir * f_lat;
                    forces.apply_force_at_point(traction, p);
                    belt_reaction += f_long;
                }

                // Displayed load = the **elastic** component only (see model 2), at the
                // column's weight like the physics.
                contacts.0.push(Contact {
                    local: to_local.transform_point3(p),
                    load: weight * SUPPORT_STIFFNESS_PER_M * area * engage,
                    normal,
                    slip: slip_long,
                    traction,
                });
            }
        }

        // Belt dynamics + advection, identical to models 2/3.
        let target = command * MAX_BELT_SPEED;
        let avail = engine_available(belt_speed);
        let engine = (BELT_GOVERNOR_GAIN * (target - belt_speed)).clamp(-avail, avail);
        let next = belt_speed + (engine - belt_reaction) / BELT_INERTIA * dt;
        belt.set(side, next.clamp(-MAX_BELT_SPEED, MAX_BELT_SPEED));
        phase.advance(side, belt_speed * dt);
    }
}

/// One sector of the guide route: which primitive a segment lies on. The tags are what make the
/// route a CHART, not just a polyline — motor membership, bending rest angles, and the tube's
/// inner bound are all sector questions.
#[derive(Clone, Copy, PartialEq)]
enum RouteTag {
    /// Wrap arc of circle `k` in the side's front→rear circle list (0 = sprocket).
    Arc(usize),
    /// Free span: a lower tangent segment or the sagging return run.
    Span,
}

/// The tagged taut guide route (step 24 / slice 3): the wrap view's envelope machinery run on the
/// CURRENT articulated wheel circles every substep, kept as an arc-length table. The chain solve
/// never leaves this route's tube — each joint carries a monotone route coordinate `s`, so
/// material order can't reshuffle and wrong-side wheel capture is unrepresentable (codex
/// Priority B), and the tube clamp doubles as the hard "the chain is ON the tank" guarantee.
struct Route {
    pts: Vec<Vec2>,
    /// Cumulative arc length at each vertex; last = total loop length.
    cum: Vec<f32>,
    /// Per-SEGMENT sector tag (`len == pts.len() − 1`).
    tags: Vec<RouteTag>,
}

impl Route {
    fn total(&self) -> f32 {
        *self.cum.last().unwrap()
    }

    fn wrap(&self, s: f32) -> f32 {
        s.rem_euclid(self.total().max(1e-4))
    }

    /// Segment index containing WRAPPED arc position `s`.
    fn seg(&self, s: f32) -> usize {
        self.cum
            .partition_point(|&c| c <= s)
            .saturating_sub(1)
            .min(self.tags.len() - 1)
    }

    fn point(&self, s: f32) -> Vec2 {
        let s = self.wrap(s);
        let i = self.seg(s);
        let len = (self.cum[i + 1] - self.cum[i]).max(1e-9);
        self.pts[i].lerp(self.pts[i + 1], (s - self.cum[i]) / len)
    }

    fn tangent(&self, s: f32) -> Vec2 {
        let i = self.seg(self.wrap(s));
        (self.pts[i + 1] - self.pts[i]).normalize_or_zero()
    }

    fn tag(&self, s: f32) -> RouteTag {
        self.tags[self.seg(self.wrap(s))]
    }

    /// The route's own turning angle (rad) over one link pitch centred at `s` — the bending rest
    /// angle θ0 at a joint's OWN route coordinate (not its array index — codex step-23 #2: index
    /// anchoring assigned wrap curvature to drifted joints and vice versa). Wheel wraps and the
    /// authored sag are free; deviation from the route's shape costs energy.
    fn turning(&self, s: f32, pitch: f32) -> f32 {
        // Discrete chords at the actual neighbour coordinates (matching the chain's own θ), not
        // point curvature — point sampling at arc/tangent tessellation seams concentrates
        // curvature into single stations.
        let a = self.point(s - pitch);
        let b = self.point(s);
        let c = self.point(s + pitch);
        let e0 = b - a;
        let e1 = c - b;
        e0.perp_dot(e1).atan2(e0.dot(e1))
    }

    /// Windowed projection of `p` onto the route near `hint`: (s, u) with `u` signed along the
    /// route's OUTWARD normal (positive = outside the loop). Only segments within the window are
    /// candidates — a global nearest-point query could tunnel `s` across overlapping parts of
    /// the loop (top run over belly); the window makes the rebase topology-safe.
    fn project(&self, p: Vec2, hint: f32, window: f32) -> (f32, f32) {
        let mut best = (self.wrap(hint), 0.0, f32::INFINITY);
        let mut s0 = hint - window;
        let hi = hint + window;
        while s0 < hi {
            let sw = self.wrap(s0);
            let i = self.seg(sw);
            let a = self.pts[i];
            let b = self.pts[i + 1];
            let ab = b - a;
            let len2 = ab.length_squared();
            if len2 > 1e-12 {
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                let q = a + ab * t;
                let d2 = p.distance_squared(q);
                if d2 < best.2 {
                    let len = len2.sqrt();
                    let tan = ab / len;
                    let out = Vec2::new(tan.y, -tan.x);
                    best = (self.cum[i] + t * len, (p - q).dot(out), d2);
                }
            }
            // Advance to the segment's end (in unwrapped window coordinates).
            s0 += (self.cum[i + 1] - sw).max(1e-6);
        }
        (best.0, best.1)
    }
}

/// Build the tagged guide route from one side's CURRENT circles (front→rear, pin-line radii):
/// the wrap view's lower convex envelope + external tangents + budgeted top-run sag, with every
/// segment tagged by the primitive it lies on. Closed: last point == first point.
fn build_route(circles: &[(Vec2, f32)], belt_len: f32) -> Route {
    fn push(pts: &mut Vec<Vec2>, tags: &mut Vec<RouteTag>, p: Vec2, tag: RouteTag) {
        if pts.last().is_none_or(|l| l.distance_squared(p) > 1e-10) {
            pts.push(p);
            tags.push(tag);
        }
    }

    // Lower convex envelope over the ordered circles (Graham-style scan, as the wrap view).
    let mut active: Vec<usize> = vec![0];
    for k in 1..circles.len() {
        while active.len() >= 2 {
            let (p, a) = (active[active.len() - 2], active[active.len() - 1]);
            let (t0, _) =
                external_tangent(circles[p].0, circles[p].1, circles[k].0, circles[k].1, -1.0);
            let n = (t0 - circles[p].0) / circles[p].1;
            if (circles[a].0 - t0).dot(n) + circles[a].1 > 1e-4 {
                break;
            }
            active.pop();
        }
        active.push(k);
    }

    let (sprocket_c, sprocket_r) = circles[0];
    let (idler_c, idler_r) = *circles.last().unwrap();
    let (idler_up, sprocket_up) = external_tangent(idler_c, idler_r, sprocket_c, sprocket_r, 1.0);

    let mut pts: Vec<Vec2> = vec![sprocket_up];
    let mut tags: Vec<RouteTag> = Vec::new();
    let mut cursor = sprocket_up;
    for w in active.windows(2) {
        let (i, j) = (w[0], w[1]);
        let (t0, t1) =
            external_tangent(circles[i].0, circles[i].1, circles[j].0, circles[j].1, -1.0);
        let toward = if i == 0 {
            Vec2::new(-1.0, 0.0) // the sprocket wraps around its front
        } else {
            Vec2::new(0.0, -1.0) // road wheels wrap under
        };
        for p in arc(circles[i].0, circles[i].1, cursor, t0, toward) {
            push(&mut pts, &mut tags, p, RouteTag::Arc(i));
        }
        push(&mut pts, &mut tags, t1, RouteTag::Span);
        cursor = t1;
    }
    let last = circles.len() - 1;
    for p in arc(idler_c, idler_r, cursor, idler_up, Vec2::new(1.0, 0.0)) {
        push(&mut pts, &mut tags, p, RouteTag::Arc(last));
    }

    // Top run: the leftover belt length as budgeted sag over the road wheels (wrap view's drape).
    let chord = idler_up.distance(sprocket_up);
    let excess = (belt_len - polyline_len(&pts) - chord).max(0.0);
    let roads = &circles[1..circles.len() - 1];
    let mut top: Vec<Vec2> = Vec::new();
    sag_span(idler_up, sprocket_up, excess, roads, 0, &mut top);
    for p in top {
        push(&mut pts, &mut tags, p, RouteTag::Span);
    }
    let first = pts[0];
    push(&mut pts, &mut tags, first, RouteTag::Span);

    let mut cum = Vec::with_capacity(pts.len());
    let mut s = 0.0;
    cum.push(0.0);
    for w in pts.windows(2) {
        s += w[0].distance(w[1]);
        cum.push(s);
    }
    Route { pts, cum, tags }
}

/// MODEL 4's **route-chain view** (`V` toggle) — step 24: the step-23 XPBD chain rehoused into a
/// tagged route tube, on a fully-owned fixed clock, with T-34 pin friction:
/// - every joint carries a **monotone route coordinate** `s` on the current articulated route;
///   the per-sweep tube clamp bounds its normal offset (zero inner bound on wheel arcs), so
///   wrong-side capture and "chain off the tank" are unrepresentable, whatever the solve did;
/// - **θ0 and the sprocket motor read the route at each joint's own `s`** (tag membership +
///   analytic route tangent), not its array index;
/// - **fixed 1/120 s clock owns its inputs**: wheel circles interpolate across the frame per
///   substep, solved output interpolates to render time, and an over-budget hitch reseeds
///   canonically instead of silently dropping debt;
/// - **hinge DRY friction** ([`CHAIN_HINGE_FRICTION`]): Coulomb resistance at every pin — the
///   physical reason a real track moves like a track and not a rope (transverse flutter dies in
///   a link or two, drape goes near-polygonal, bulk yank passes through), with isotropic drag
///   demoted to a residual bleed;
/// - **pinch fuses**: per-joint speed cap, clamped terrain corrections, and a torn-link/NaN
///   detector that reseeds from the route.
pub(super) fn conform_belts_field_chain(
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Transform)>,
    field: Res<TerrainField>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    belt: Res<BeltSpeed>,
    time: Res<Time>,
    mut acc: Local<f32>,
    mut memory: ResMut<ChainMemory>,
    mut belts: ResMut<ConformedBelts>,
    mut reference: ResMut<ChainReference>,
    // Perf probe: (busy seconds, substep-sides, frames) — the promotion-budget number.
    mut perf: Local<(f64, u64, u64)>,
) {
    let t_perf = std::time::Instant::now();
    let hull = *hull;
    let affine = hull.affine();
    let to_local = affine.inverse();
    // Fixed-clock accumulator. A frame that outruns the whole catch-up budget no longer drops
    // debt silently: the chain state is stale by more than the budget, so it reseeds canonically
    // from current inputs instead (codex step-23 #5).
    let dt = time.delta_secs();
    let budget = CHAIN_SUBSTEP * CHAIN_MAX_SUBSTEPS as f32;
    let overrun = *acc + dt > budget + CHAIN_SUBSTEP;
    *acc = (*acc + dt).min(budget);
    let steps = (*acc / CHAIN_SUBSTEP) as usize;
    *acc -= steps as f32 * CHAIN_SUBSTEP;
    // Render-time interpolation factor between the last two solved substeps.
    let alpha = (*acc / CHAIN_SUBSTEP).clamp(0.0, 1.0);
    let g3 = to_local.transform_vector3(Vec3::NEG_Y * 9.81);
    let g2 = Vec2::new(g3.z, g3.y);

    for side in [Side::Left, Side::Right] {
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        // This side's circles, front→rear, inflated to the pin line: fixed drive circles + the
        // ARTICULATED road wheels, sorted so the envelope scan and the frame-to-frame
        // interpolation both see a stable order.
        let (sprocket, idler) = drive_circles_local();
        let mut roads: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _)| w.side == side && w.kind == WheelKind::Road)
            .map(|(_, t)| {
                (
                    Vec2::new(t.translation.z, t.translation.y),
                    ROAD_RADIUS + TRACK_THICKNESS / 2.0,
                )
            })
            .collect();
        roads.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        let mut circles = vec![(sprocket.0, sprocket.1 + TRACK_THICKNESS / 2.0)];
        circles.extend(roads);
        circles.push((idler.0, idler.1 + TRACK_THICKNESS / 2.0));

        // The IMMUTABLE material pitch, straight from the authored belt length minus the
        // tensioner preload — never a polyline measurement (links must not breathe with phase).
        let n = pin_belt.count;
        if n < 3 {
            continue;
        }
        let chain_len = pin_belt.length - CHAIN_SLACK_TRIM;
        let pitch = chain_len / n as f32;
        let belt_speed = belt.get(side);
        let phase_frac = phase.get(side).rem_euclid(pitch);
        let mem = memory.get_mut(side);

        // Canonical reseed, entirely data-derived: joints in material order at exact pitch along
        // the route (phase-aligned), each lifted out of terrain (a taut-route seed through a
        // board would be torn apart by the terrain pass next substep — the reseed must land
        // FEASIBLE or it loops), zero velocity, fresh route coordinates.
        let field = &field;
        let seed = move |route: &Route, mem: &mut ChainSideMemory| {
            mem.s = (0..n)
                .map(|i| route.wrap(phase_frac + i as f32 * pitch))
                .collect();
            mem.pos = (0..n)
                .map(|i| {
                    let s = mem.s[i];
                    let q = route.point(s);
                    let tan = route.tangent(s);
                    let out2 = Vec2::new(tan.y, -tan.x);
                    let w = affine.transform_point3(Vec3::new(
                        track_x,
                        q.y + out2.y * (TRACK_THICKNESS / 2.0),
                        q.x + out2.x * (TRACK_THICKNESS / 2.0),
                    ));
                    let out = affine
                        .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                        .normalize_or_zero();
                    let d = field.depth_along(w, out).max(0.0);
                    q - out2 * d
                })
                .collect();
            mem.prev = mem.pos.clone();
        };

        // Wheels move across the frame: each substep sees circles interpolated between last
        // frame's and this frame's positions (the fixed clock owns its inputs — no substep sees
        // one big end-of-frame jump; codex step-23 #5).
        let prev_circles = if mem.prev_circles.len() == circles.len() {
            mem.prev_circles.clone()
        } else {
            circles.clone()
        };

        // Material identity: rotate the stored ring (and its route coordinates) when the phase
        // crosses whole pitches.
        let shift = (phase.get(side) / pitch).floor() as i64;
        if overrun || mem.pos.len() != n || mem.s.len() != n {
            seed(&build_route(&circles, chain_len), mem);
        } else {
            let rot = (shift - mem.shift).rem_euclid(n as i64) as usize;
            mem.pos.rotate_right(rot);
            mem.prev.rotate_right(rot);
            mem.s.rotate_right(rot);
        }
        mem.shift = shift;

        let ret_t = (-(std::f32::consts::LN_2 / CHAIN_HALF_LIFE_TAN) * CHAIN_SUBSTEP).exp();
        let ret_n = (-(std::f32::consts::LN_2 / CHAIN_HALF_LIFE_NORM) * CHAIN_SUBSTEP).exp();
        let motor_gain =
            (CHAIN_SUBSTEP / CHAIN_MOTOR_TAU) / (1.0 + CHAIN_SUBSTEP / CHAIN_MOTOR_TAU);
        // Real inverse mass in every compliant/limited constraint: compliance and friction
        // torque are physical units, not normalized view parameters.
        let w_inv = 1.0 / CHAIN_NODE_MASS;
        let alpha_tilde = (pitch / CHAIN_BEND_STIFFNESS) / (CHAIN_SUBSTEP * CHAIN_SUBSTEP);
        // Per-substep friction multiplier bound: |λ| ≤ τ·h², accumulated across sweeps and
        // clamped as a TOTAL (clamping per sweep would quadruple the requested torque).
        let friction_cap = CHAIN_HINGE_TORQUE * CHAIN_SUBSTEP * CHAIN_SUBSTEP;
        // Post-solve velocity guardrails (per substep, as displacements).
        let cap_t = 8.0_f32.max(belt_speed.abs() + 5.0) * CHAIN_SUBSTEP;
        let cap_n = CHAIN_MAX_NORMAL_SPEED * CHAIN_SUBSTEP;

        for k in 0..steps {
            let f = (k + 1) as f32 / steps as f32;
            let circ: Vec<(Vec2, f32)> = circles
                .iter()
                .zip(&prev_circles)
                .map(|(c, pr)| (pr.0.lerp(c.0, f), c.1))
                .collect();
            let route = build_route(&circ, chain_len);
            let old_pos = mem.pos.clone();
            // Bending rest angles at each joint's own route coordinate, this substep's route.
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
                    // Anisotropic damping in the ROUTE frame: tangential motion (yank, slack
                    // migration) barely decays, route-normal motion (flutter) dies fast.
                    let tan = route.tangent(mem.s[i]);
                    let nrm = Vec2::new(tan.y, -tan.x);
                    let v = mem.pos[i] - mem.prev[i];
                    let mut vel = tan * (v.dot(tan) * ret_t) + nrm * (v.dot(nrm) * ret_n);
                    // Sprocket motor: membership by ROUTE SECTOR (never the disk interior or a
                    // folded node), tangent from the route (analytic, oriented), rim-distance
                    // ramp so entering the sector is impulse-free.
                    if route.tag(mem.s[i]) == RouteTag::Arc(0) {
                        let rim = (mem.pos[i] - circ[0].0).length() - circ[0].1;
                        let engage = (1.0 - rim.abs() / pitch).clamp(0.0, 1.0);
                        if engage > 0.0 {
                            let v_t = vel.dot(tan);
                            vel += tan * ((belt_speed * CHAIN_SUBSTEP - v_t) * motor_gain * engage);
                        }
                    }
                    mem.pos[i] + vel + g2 * (CHAIN_SUBSTEP * CHAIN_SUBSTEP)
                })
                .collect();

            // Terrain contact planes at the CHAIN's OWN predicted positions, refreshed every
            // substep. Per LINK: the SAME pin/mid/pin longitudinal stations × 3 lateral columns
            // the physics collocates, deepest value, linearized along the link's outward normal.
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
                    let out = affine
                        .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                        .normalize_or_zero();
                    let axis = affine
                        .transform_vector3(Vec3::new(0.0, tan.y, tan.x))
                        .normalize_or_zero();
                    let lat = out.cross(axis);
                    let face2 = out2 * (TRACK_THICKNESS / 2.0);
                    let mut d = f32::NEG_INFINITY;
                    for s2 in [a + face2, (a + b) / 2.0 + face2, b + face2] {
                        let w = affine.transform_point3(Vec3::new(track_x, s2.y, s2.x));
                        for (offset, _) in COLUMNS {
                            d = d.max(field.depth_along(w + lat * offset, out));
                        }
                    }
                    // Keep nearly-clear planes too: sweeps move joints and must not tunnel past
                    // a boundary probed as barely clear.
                    (d > -0.08).then_some((out2, d))
                })
                .collect();
            let p_start = p.clone();
            // XPBD multipliers — scratch per substep. `fired` records which contact planes
            // actually pushed, for the no-restitution velocity reconstruction below.
            let mut lambda = vec![0.0_f32; n];
            let mut lambda_f = vec![0.0_f32; n];
            let mut fired = vec![false; n];
            for _ in 0..CHAIN_SWEEPS {
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
                // (b) XPBD bending regularizer: C = θ − θ0 with the analytic turning-angle
                // gradients, REAL compliance (α = pitch / B, inverse masses in the denominator).
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
                // (b2) Pin DRY friction: a torque-LIMITED constraint toward the joint's
                // PREVIOUS material angle. Zero compliance inside the torque bound — stiction:
                // flutter-scale articulation is simply held; anything needing more torque than
                // a dry pin provides (sprocket wrap, tension, gravity) slips through at
                // bounded resistance. The multiplier accumulates across sweeps and the TOTAL is
                // clamped to ±τ·h² (per-sweep clamping would quadruple the torque).
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
                // (c) Signed hinge stop — the hard link-geometry limit as a zero-compliance
                // projection with the SAME turning-angle gradients as the bending pass.
                for i in 0..n {
                    let (im, ip) = ((i + n - 1) % n, (i + 1) % n);
                    let e0 = p[i] - p[im];
                    let e1 = p[ip] - p[i];
                    let (l0, l1) = (e0.length_squared(), e1.length_squared());
                    if l0 < 1e-9 || l1 < 1e-9 {
                        continue;
                    }
                    let theta = e0.perp_dot(e1).atan2(e0.dot(e1));
                    let c = theta - theta.clamp(-MAX_LINK_ANGLE, MAX_LINK_ANGLE);
                    if c == 0.0 {
                        continue;
                    }
                    let g_prev = e0.perp() / l0;
                    let g_next = e1.perp() / l1;
                    let g_mid = -(g_prev + g_next);
                    let denom =
                        g_prev.length_squared() + g_mid.length_squared() + g_next.length_squared();
                    let dl = -c / denom;
                    p[im] += g_prev * dl;
                    p[i] += g_mid * dl;
                    p[ip] += g_next * dl;
                }
                // (d) Terrain: each joint stays out of its own contact plane. Violation = the
                // probed depth plus however far the sweeps have moved the joint along its
                // outward normal since the probe. Pinch discipline (codex step-24): a SATURATED
                // probe (buried origin) is a bounded contact signal, not a 0.5 m positional
                // correction — skip it and let the reseed detector judge the state; corrections
                // cap at half a pitch per application; and if the corrected point would land
                // inside a wheel, the terrain constraint YIELDS (wheel hard, terrain compliant
                // — the two projectors must never alternate on an empty feasible set).
                for (i, c) in contact.iter().enumerate() {
                    let Some((out2, d)) = *c else {
                        continue;
                    };
                    if d >= CONTACT_PROBE - 1e-3 {
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
                // (e) Wheel circles: nearest-exit for joints, plus LINK-CHORD exclusion — two
                // pins can both clear a wheel while their connecting link cuts through it
                // (arc/tangent handoffs); the midpoint test catches the chord.
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
                // (f) Route tube (slice 3): rebase every joint to its windowed route coordinate
                // — monotone, so material order can't reshuffle — and clamp its normal offset
                // to the tube. On a wheel arc the inner bound is ZERO: radially off the rim is
                // the only way out, so wrong-side capture is unrepresentable. Also pinch fuse
                // #3: whatever the projections above did, a joint ends the sweep within
                // [`CHAIN_TUBE_OUT`] of the route — never "off the tank".
                let total = route.total();
                let mut prev_s = f32::NAN;
                #[allow(clippy::needless_range_loop)] // i indexes p, mem.s, AND the order state
                for i in 0..n {
                    let (mut s_i, mut u) = route.project(p[i], mem.s[i], CHAIN_REBASE_WINDOW);
                    if i > 0 {
                        let gap = (s_i - prev_s).rem_euclid(total);
                        if !(0.2 * pitch..=2.0 * pitch).contains(&gap) {
                            let clamped = if gap > total * 0.5 {
                                0.2 * pitch // projected behind its predecessor: order violation
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
                        RouteTag::Arc(_) => (0.0, CHAIN_TUBE_OUT),
                        RouteTag::Span => (-CHAIN_TUBE_IN, CHAIN_TUBE_OUT),
                    };
                    if u < u_min || u > u_max {
                        let q = route.point(s_i);
                        let tan = route.tangent(s_i);
                        let out = Vec2::new(tan.y, -tan.x);
                        p[i] = q + out * u.clamp(u_min, u_max);
                    }
                    mem.s[i] = s_i;
                    prev_s = s_i;
                }
                // (g) Closing length pass: the projections above must not leave accumulated
                // pitch error in the loop (exact total length is the tension model).
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
            // Pinch fuse: a substep that still ends with NaNs or torn links reseeds
            // canonically — the chain may visibly pop back onto the route once, but it can
            // never leave the tank or poison the next frame.
            let torn = p.iter().any(|q| !q.is_finite())
                || (0..n).any(|i| ((p[(i + 1) % n] - p[i]).length() - pitch).abs() > 0.25 * pitch);
            if torn {
                warn!("route-chain reseed: torn links after substep (pinch fuse)");
                seed(&route, mem);
                continue;
            }
            // Velocity reconstruction — THE pinch fix (codex step-24 #1): `prev = old_pos`
            // would turn every unilateral depenetration into Verlet restitution velocity (a
            // 0.5 m correction = 60 m/s next substep — the "shoots off the tank" energy
            // source). Instead: bilateral responses (lengths, motor, bending) keep their
            // velocity; an ACTIVE terrain contact keeps only its pre-projection escape
            // velocity and never gains inward motion; wheels zero inward radial motion; then
            // anisotropic route-frame guardrails cap what's stored.
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
                dp =
                    tan * dp.dot(tan).clamp(-cap_t, cap_t) + nrm * dp.dot(nrm).clamp(-cap_n, cap_n);
                mem.prev[i] = p[i] - dp;
            }
            mem.pos = p;
        }
        if steps > 0 {
            mem.prev_circles = circles.clone();
        }

        // The current route is the `-` viz layer: chain-vs-route deviation shows exactly where
        // terrain, slack, and whip hold the belt off its taut path.
        let route_now = build_route(&circles, chain_len);
        let ref_world: Vec<Vec3> = route_now
            .pts
            .iter()
            .map(|p| affine.transform_point3(Vec3::new(track_x, p.y, p.x)))
            .collect();
        match side {
            Side::Left => reference.left = ref_world,
            Side::Right => reference.right = ref_world,
        }

        // Render output: interpolate between the last two solved substeps — the accumulator
        // remainder says exactly how far render time sits past the last solve (rendering above
        // 120 Hz no longer repeats states; codex step-23 #5).
        let interp: Vec<Vec2> = if mem.prev.len() == n {
            (0..n)
                .map(|i| mem.prev[i].lerp(mem.pos[i], alpha))
                .collect()
        } else {
            mem.pos.clone()
        };
        let samples: Vec<BeltSample> = interp
            .iter()
            .map(|&p| BeltSample {
                local: p,
                world: affine.transform_point3(Vec3::new(track_x, p.y, p.x)),
            })
            .collect();
        match side {
            Side::Left => belts.left = samples,
            Side::Right => belts.right = samples,
        }
    }
    perf.0 += t_perf.elapsed().as_secs_f64();
    perf.1 += steps as u64 * 2;
    perf.2 += 1;
    if perf.2.is_multiple_of(512) {
        info!(
            "route-chain perf: {:.0} µs/frame avg | {:.1} µs/substep-side ({} substep-sides / {} frames)",
            perf.0 / perf.2 as f64 * 1e6,
            perf.0 / (perf.1 as f64).max(1.0) * 1e6,
            perf.1,
            perf.2
        );
    }
}

/// Probe stations along a road wheel's lower arc as (sin θ, cos θ) from straight down, every 5°
/// to ±50° — fixed samples, so the wheel's terrain read is deterministic like every other field
/// consumer. Density matters: the wheel's lift target is FROZEN between a board edge crossing two
/// adjacent probes, then catches up in one step — at 25° spacing that step measured ~55 mm/tick
/// (double the true circle-on-edge ramp at crawl speed); at 5° it stays under the ~25 mm/tick the
/// real geometry moves, so the quantization hides inside the honest motion.
const WHEEL_ARC: [(f32, f32); 21] = [
    (-0.766, 0.643),
    (-0.707, 0.707),
    (-0.643, 0.766),
    (-0.574, 0.819),
    (-0.500, 0.866),
    (-0.423, 0.906),
    (-0.342, 0.940),
    (-0.259, 0.966),
    (-0.174, 0.985),
    (-0.087, 0.996),
    (0.0, 1.0),
    (0.087, 0.996),
    (0.174, 0.985),
    (0.259, 0.966),
    (0.342, 0.940),
    (0.423, 0.906),
    (0.500, 0.866),
    (0.574, 0.819),
    (0.643, 0.766),
    (0.707, 0.707),
    (0.766, 0.643),
];

/// Critically-damped ease frequency (rad/s) of a wrap-view wheel's RISE (settle ≈ 4.7/ω ≈
/// 100 ms). Integrated implicitly — see [`articulate_wheels_field`].
const WHEEL_EASE_OMEGA: f32 = 45.0;

/// MODEL 4's road wheels, placed directly from the terrain FIELD — wheels first, then the belt
/// wraps them (`ground → wheels → belt`, acyclic). The step-21 order was circular: the chain
/// wrapped the wheels' current circles while the wheels rode the solved chain, stabilized only by
/// a one-frame lag — the root of the teleport/settle wrong-side captures (step-22 review).
///
/// The wheel's ground surface (its circle inflated by the track thickness beneath it) is probed
/// at [`WHEEL_ARC`] stations along the SAME 3 lateral columns the physics reads; the lift target
/// is the deepest directional penetration. Smoothing is asymmetric: a **rise is a fast
/// critically-damped ease** ([`WHEEL_EASE_OMEGA`], integrated IMPLICITLY so it is
/// unconditionally stable at any frame rate — the step-21b spring diverged at 60 fps because its
/// damping was explicit, not because springs are wrong; and truly instant rise read robotic,
/// Yan's step-22 verdict), a **fall is ballistic** (the wheel drops at gravity, not at a tuned
/// rate — a 0.18 m board edge takes ~190 ms because g says so). One signed velocity scalar of
/// cosmetic state, shared by both branches.
pub(super) fn articulate_wheels_field(
    hull: Single<&GlobalTransform, With<Hull>>,
    field: Res<TerrainField>,
    time: Res<Time>,
    mut wheels: Query<(&RigWheel, &mut Suspension, &mut Transform)>,
) {
    let affine = hull.affine();
    let down = affine.transform_vector3(Vec3::NEG_Y).normalize_or_zero();
    let dt = time.delta_secs();
    // Wheel surface + the track plate riding between it and the ground.
    let reach = ROAD_RADIUS + TRACK_THICKNESS;
    for (wheel, mut susp, mut transform) in &mut wheels {
        if wheel.kind != WheelKind::Road {
            continue;
        }
        let mut target = 0.0_f32;
        for (s, c) in WHEEL_ARC {
            for (offset, _) in COLUMNS {
                let local = susp.pivot_local + Vec3::new(offset, -reach * c, reach * s);
                target = target.max(field.depth_along(affine.transform_point3(local), down));
            }
        }
        let target = target.min(SUSP_MAX_LIFT);
        susp.target = target;
        let err = target - susp.dy;
        if err >= 0.0 {
            // Implicit critically-damped step: v' = (v + ω²·e·Δt) / (1 + ωΔt)². Stable for any
            // ωΔt; settles ≈ 4.7/ω (~100 ms) — the ease Yan liked in the chain view, without its
            // solver. Entering a rise from a fall, the (negative) fall speed carries in and the
            // spring absorbs it.
            let wdt = WHEEL_EASE_OMEGA * dt;
            susp.dvel = (susp.dvel + WHEEL_EASE_OMEGA * WHEEL_EASE_OMEGA * err * dt)
                / (1.0 + 2.0 * wdt + wdt * wdt);
            susp.dy = (susp.dy + susp.dvel * dt).min(target);
        } else {
            // Ballistic fall; an upward launch (dvel > 0 from a rise) decelerates first.
            susp.dvel -= 9.81 * dt;
            susp.dy = (susp.dy + susp.dvel * dt).clamp(target, SUSP_MAX_LIFT);
            if susp.dy <= target {
                susp.dy = target;
                susp.dvel = 0.0;
            }
        }
        transform.translation.y = susp.pivot_local.y + susp.dy;
    }
}

/// MODEL 4's track view — a **stateless kinematic wrap** (step 22): no integration, no
/// constraints, no per-frame memory. The path is recomputed from scratch every frame as a pure
/// function of the articulated wheels, the terrain field, and the belt phase:
///
/// 1. **taut wrap** — the lower convex envelope of the pin-line circles (tangent segments + wheel
///    arcs, front→rear; a wheel above the taut line between its neighbours simply drops out);
/// 2. **terrain conform** — every ground-facing station displaced along its outward normal by the
///    directional field depth, max over the SAME 3 lateral columns the physics samples (the
///    visual≡physics invariant, kept);
/// 3. **top run** — the leftover belt length as a sag parabola between the drive wheels' upper
///    tangents, clipped from above onto the wheel circles (the loose T-34 return run rides its
///    road wheels); the conform feeds the length budget FORWARD, so belly lift shortens the sag
///    the same frame (no smoothed `belly_extra` feedback);
/// 4. **links** — the closed path resampled at link pitch with the belt phase.
///
/// Wrong-side wheel capture, compression zigzag, teleport transients, and solver stability are
/// not tuned away here — they are unrepresentable: there is no state to capture, buckle, stale,
/// or diverge. Remote tanks render identically on every client as a pure function of replicated
/// pose + phase (ADR-0014 satisfied by construction).
pub(super) fn conform_belts_field(
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Suspension)>,
    field: Res<TerrainField>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    mut belts: ResMut<ConformedBelts>,
    mut reference: ResMut<ChainReference>,
    // Perf probe: (busy seconds, frames) — the wrap's side of the promotion budget.
    mut perf: Local<(f64, u64)>,
) {
    let t_perf = std::time::Instant::now();
    let affine = hull.affine();
    for side in [Side::Left, Side::Right] {
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        // Pin-line circles, front→rear: sprocket, the ARTICULATED road wheels, idler.
        let (sprocket, idler) = drive_circles_local();
        let mut roads: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _)| w.side == side && w.kind == WheelKind::Road)
            .map(|(_, s)| {
                (
                    Vec2::new(s.pivot_local.z, s.pivot_local.y + s.dy),
                    ROAD_RADIUS + TRACK_THICKNESS / 2.0,
                )
            })
            .collect();
        roads.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
        let mut circles = vec![(sprocket.0, sprocket.1 + TRACK_THICKNESS / 2.0)];
        circles.extend(roads.iter().copied());
        circles.push((idler.0, idler.1 + TRACK_THICKNESS / 2.0));

        // 1. Lower convex envelope over the ordered circles (Graham-style scan): a circle whose
        // body stays above its neighbours' lower tangent is not part of the taut run and drops
        // out — a lifted wheel is skipped, never wrapped from the wrong side (the route-selection
        // rule; fixed logical order, no per-frame hull search).
        let mut active: Vec<usize> = vec![0];
        for k in 1..circles.len() {
            while active.len() >= 2 {
                let (p, a) = (active[active.len() - 2], active[active.len() - 1]);
                let (t0, _) =
                    external_tangent(circles[p].0, circles[p].1, circles[k].0, circles[k].1, -1.0);
                // Unit lower normal of the p→k tangent line (t0 sits on circle p by construction).
                let n = (t0 - circles[p].0) / circles[p].1;
                // Keep `a` only if it protrudes below that line.
                if (circles[a].0 - t0).dot(n) + circles[a].1 > 1e-4 {
                    break;
                }
                active.pop();
            }
            active.push(k);
        }

        // The taut bottom polyline, sprocket_up → front arc → tangents/arcs → idler_up.
        let (sprocket_c, sprocket_r) = circles[0];
        let (idler_c, idler_r) = *circles.last().unwrap();
        let (idler_up, sprocket_up) =
            external_tangent(idler_c, idler_r, sprocket_c, sprocket_r, 1.0);
        let mut bottom: Vec<Vec2> = Vec::new();
        let mut cursor = sprocket_up;
        for w in active.windows(2) {
            let (i, j) = (w[0], w[1]);
            let (t0, t1) =
                external_tangent(circles[i].0, circles[i].1, circles[j].0, circles[j].1, -1.0);
            let toward = if i == 0 {
                Vec2::new(-1.0, 0.0) // the sprocket wraps around its front
            } else {
                Vec2::new(0.0, -1.0) // road wheels wrap under
            };
            bottom.extend(arc(circles[i].0, circles[i].1, cursor, t0, toward));
            bottom.push(t1);
            cursor = t1;
        }
        bottom.extend(arc(idler_c, idler_r, cursor, idler_up, Vec2::new(1.0, 0.0)));

        // The taut (unconformed) loop is the `-` reference layer: chain-vs-reference deviation
        // shows exactly where terrain holds the belt off its rest path.
        let ref_loop = close_loop(&bottom, idler_up, sprocket_up, pin_belt.length, &roads);
        let ref_world: Vec<Vec3> = ref_loop
            .iter()
            .map(|p| affine.transform_point3(Vec3::new(track_x, p.y, p.x)))
            .collect();
        match side {
            Side::Left => reference.left = ref_world,
            Side::Right => reference.right = ref_world,
        }

        // 2. Terrain conform: displace each ground-facing station AGAINST its outward normal by
        // the directional field depth — a buried station is lifted back INSIDE the loop until its
        // outer face sits on the terrain surface (belly rises onto boards, nose backs off a
        // wall). The step-22 first cut had this sign inverted, pushing the belly INTO boards and
        // the nose off the sprocket — Yan's wall/phase-through findings. Deepest of the physics'
        // 3 lateral columns; C0 because the field is rounded.
        //
        // Conform on a DENSE resample, not the wrap's vertices: a tangent segment between two
        // wheels is one long edge — with only its endpoints conformed, a board mid-segment goes
        // unsampled and the belt cuts through it (the second half of the phase-through finding).
        let mut bottom = resample(&bottom, BELT_DRAW_SPACING, 0.0);
        bottom.push(idler_up);
        let m = bottom.len();
        let outs: Vec<Vec2> = (0..m)
            .map(|i| {
                let tan =
                    (bottom[(i + 1).min(m - 1)] - bottom[i.saturating_sub(1)]).normalize_or_zero();
                Vec2::new(tan.y, -tan.x)
            })
            .collect();
        let depths: Vec<f32> = (0..m)
            .map(|i| {
                let out2 = outs[i];
                if out2 == Vec2::ZERO {
                    return 0.0;
                }
                let s2 = bottom[i] + out2 * (TRACK_THICKNESS / 2.0);
                let w = affine.transform_point3(Vec3::new(track_x, s2.y, s2.x));
                let out = affine
                    .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                    .normalize_or_zero();
                let tan = Vec2::new(-out2.y, out2.x);
                let axis = affine
                    .transform_vector3(Vec3::new(0.0, tan.y, tan.x))
                    .normalize_or_zero();
                let lat = out.cross(axis);
                let mut d = 0.0_f32;
                for (offset, _) in COLUMNS {
                    d = d.max(field.depth_along(w + lat * offset, out));
                }
                d.max(0.0)
            })
            .collect();
        // A rigid link OVERHANGS a board edge: the line stays high for about half a pitch before
        // the pin clears the edge, then articulates down over the next — the chain got this from
        // its per-link constraint. Reproduce it on the displacement field: a ±1-station max
        // filter (the overhang; never sinks a lift) followed by a 3-tap triangular smooth (the
        // articulation rounding). Without it, the pointwise ramp starts AT the edge and the belt
        // shaves the corner (~100 mm transients at the 0.18 m boards).
        let widened: Vec<f32> = (0..m)
            .map(|i| {
                depths[i.saturating_sub(1)]
                    .max(depths[i])
                    .max(depths[(i + 1).min(m - 1)])
            })
            .collect();
        let conformed: Vec<Vec2> = (0..m)
            .map(|i| {
                let d = 0.25 * widened[i.saturating_sub(1)]
                    + 0.5 * widened[i]
                    + 0.25 * widened[(i + 1).min(m - 1)];
                if d > 0.0 {
                    bottom[i] - outs[i] * d
                } else {
                    bottom[i]
                }
            })
            .collect();

        // 3 + 4. Close with the budgeted sag and scroll the links along the loop.
        let mut loop_pts = close_loop(&conformed, idler_up, sprocket_up, pin_belt.length, &roads);
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / pin_belt.count.max(1) as f32;
        let mut joints = resample(&loop_pts, pitch, phase.get(side).rem_euclid(pitch));
        joints.truncate(pin_belt.count);
        if joints.len() < 3 {
            continue;
        }
        let samples: Vec<BeltSample> = joints
            .iter()
            .map(|&p| BeltSample {
                local: p,
                world: affine.transform_point3(Vec3::new(track_x, p.y, p.x)),
            })
            .collect();
        match side {
            Side::Left => belts.left = samples,
            Side::Right => belts.right = samples,
        }
    }
    perf.0 += t_perf.elapsed().as_secs_f64();
    perf.1 += 1;
    if perf.1.is_multiple_of(512) {
        info!(
            "kinematic-wrap perf: {:.0} µs/frame avg ({} frames)",
            perf.0 / perf.1 as f64 * 1e6,
            perf.1
        );
    }
}

/// Close a bottom polyline (sprocket_up → … → idler_up) into the full belt loop: the belt length
/// left over after the bottom run becomes the return run's drape ([`sag_span`]). The
/// `max(0)` on the excess is the explicit length-budget clamp: a conform-lengthened bottom run
/// beyond the total belt length runs the top taut instead of laundering the deficit into the
/// shape (the step-22 infeasibility rule).
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

/// Drape one return-run span with `excess` metres of slack as a parabola — and if the curve dips
/// into a road wheel, PROMOTE that wheel to a support: split the span at the wheel's top and
/// drape each side with its share of the remaining slack (the loose T-34 return run riding its
/// wheels, hanging in short spans between them — the chain view's drape, computed instead of
/// solved). The step-22 first cut used ONE global parabola clipped onto the circles, which dumped
/// the whole budget into the long end spans (Yan: "slack more substantial than chain mode").
/// Points arrive from above by construction, so which side of a wheel the belt is on is given,
/// never discovered.
fn sag_span(
    from: Vec2,
    to: Vec2,
    excess: f32,
    wheels: &[(Vec2, f32)],
    depth: usize,
    out: &mut Vec<Vec2>,
) {
    const SEGMENTS: usize = 16;
    let chord = from.distance(to);
    let h = (3.0 * chord * excess / 8.0).sqrt();
    // The deepest wheel the sag would enter, tested at the wheel's own abscissa.
    let mut worst: Option<(Vec2, f32)> = None;
    if depth < 4 {
        for &(c, r) in wheels {
            let (lo, hi) = (from.x.min(to.x), from.x.max(to.x));
            if c.x <= lo || c.x >= hi || (to.x - from.x).abs() < 1e-4 {
                continue;
            }
            let t = (c.x - from.x) / (to.x - from.x);
            let sag_y = from.lerp(to, t).y - 4.0 * h * t * (1.0 - t);
            let pen = (c.y + r) - sag_y;
            if pen > 1e-3 && worst.is_none_or(|(_, w)| pen > w) {
                worst = Some((Vec2::new(c.x, c.y + r), pen));
            }
        }
    }
    if let Some((split, _)) = worst {
        let (l, r) = (from.distance(split), split.distance(to));
        // The detour over the wheel top consumes slack; the remainder splits by chord share.
        let remaining = (excess - (l + r - chord)).max(0.0);
        sag_span(from, split, remaining * l / (l + r), wheels, depth + 1, out);
        sag_span(split, to, remaining * r / (l + r), wheels, depth + 1, out);
        return;
    }
    for i in 0..=SEGMENTS {
        let t = i as f32 / SEGMENTS as f32;
        let base = from.lerp(to, t);
        let mut q = Vec2::new(base.x, base.y - 4.0 * h * t * (1.0 - t));
        // Safety clip (mm-scale grazes near tangency that promotion's point-split leaves).
        for &(c, r) in wheels {
            let dz = q.x - c.x;
            if dz.abs() < r {
                q.y = q.y.max(c.y + (r * r - dz * dz).sqrt());
            }
        }
        out.push(q);
    }
}

/// The `9` viz layer for MODEL 4: the collocation stations at the **physics** ring (pins + mids
/// on the outer face) — grey when clear of terrain, orange when penetrating. The whole oracle,
/// visible.
pub(super) fn draw_sample_points(
    mut gizmos: Gizmos,
    viz: Res<VizLayers>,
    hull: Single<&GlobalTransform, With<Hull>>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    field: Res<TerrainField>,
) {
    if !viz.casts {
        return;
    }
    let affine = hull.affine();
    for side in [Side::Left, Side::Right] {
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        let mut loop_pts = belt_loop(&pin_circles(), None);
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / pin_belt.count.max(1) as f32;
        let mut stations = resample(&loop_pts, pitch, phase.get(side).rem_euclid(pitch));
        stations.truncate(pin_belt.count);
        let n = stations.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = stations[i];
            let b = stations[(i + 1) % n];
            let seg = b - a;
            let len = seg.length();
            if len < 1e-4 {
                continue;
            }
            let tan2 = seg / len;
            let out2 = Vec2::new(tan2.y, -tan2.x);
            let wa = affine.transform_point3(Vec3::new(track_x, a.y, a.x));
            let wb = affine.transform_point3(Vec3::new(track_x, b.y, b.x));
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let axis = (wb - wa) / len;
            let lat = out.cross(axis);
            let face = out * (TRACK_THICKNESS / 2.0);
            for (offset, _) in COLUMNS {
                let shift = lat * offset;
                let (ca, cb) = (wa + shift, wb + shift);
                for s in [ca + face, (ca + cb) / 2.0 + face, cb + face] {
                    let color = if field.depth_along(s, out) > 0.0 {
                        TRACTION_FORCE_COLOR
                    } else {
                        CAST_COLOR
                    };
                    gizmos.sphere(Isometry3d::from_translation(s), 0.015, color);
                }
            }
        }
    }
}
