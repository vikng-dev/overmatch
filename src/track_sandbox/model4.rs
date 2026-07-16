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

use super::model2::clipped_linear_piece;
use super::model3::{PinBelt, TRACK_THICKNESS, pin_circles};
use super::*;

/// Edge rounding radius (m) of the terrain field: every authored box is evaluated as a rounded
/// box (core shrunk by this, surface pushed back out), so the union's surface is C1 across box
/// edges at the cost of visually-invisible 3 cm corner rounding. Must stay below the smallest
/// authored half-extent (washboard bump half-height 0.06).
const FIELD_ROUNDING: f32 = 0.03;

/// Finite-difference step (m) for the field gradient (conform planes only).
const GRAD_EPS: f32 = 1e-3;

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

// Chain-solve knobs: model 3's verified values, owned per model so the field-belt tunes
// independently.
/// Per-frame velocity retention of the chain's Verlet integration — the swing knob (see model 2).
const CHAIN_DAMPING: f32 = 0.88;
/// Drive-anchor stiffness (s⁻²) toward the advected reference, ALONG the reference tangent: the
/// advection/drive-yank coupling — full strength, so the chain still snaps to throttle.
const CHAIN_DRIVE: f32 = 400.0;
/// Drive-anchor stiffness ACROSS the reference tangent: deliberately weak, so gravity owns the
/// transverse shape. An isotropic anchor pinned the chain to the reference line transversely —
/// under compression the length surplus then buckled into many small alternating kinks around
/// the reference (each kink costs the anchor little; one honest deep sag costs it a lot) — the
/// step-18 ZIGZAG. Weak transverse anchoring inverts that preference.
const CHAIN_DRIVE_ACROSS: f32 = 60.0;
/// Per-iteration bending relaxation toward each joint's neighbour midpoint, at ALL articulation
/// angles (the 35° cap only clips sharp folds — it cannot see a zigzag made of sub-cap angles).
/// Alternating kinks are pure high curvature and get erased; a gravity sag is low curvature and
/// survives; wheel wraps are re-asserted by the circle constraints each iteration.
const CHAIN_BEND: f32 = 0.02;
/// Gauss–Seidel passes of the constraint projections (see model 2).
const CHAIN_ITERATIONS: usize = 20;
/// Max articulation between consecutive links (rad): must clear the T-34 sprocket's wrap demand
/// of ~31°/joint (see model 2).
const MAX_LINK_ANGLE: f32 = 35.0 * std::f32::consts::PI / 180.0;

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

    /// Field gradient (unit, points out of terrain) by central differences — conform planes only.
    fn gradient(&self, p: Vec3) -> Vec3 {
        let h = GRAD_EPS;
        Vec3::new(
            self.sdf(p + Vec3::X * h) - self.sdf(p - Vec3::X * h),
            self.sdf(p + Vec3::Y * h) - self.sdf(p - Vec3::Y * h),
            self.sdf(p + Vec3::Z * h) - self.sdf(p - Vec3::Z * h),
        )
        .normalize_or_zero()
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

/// MODEL 4's **frozen A/B chain view** (`V` toggle; step-21 state) — model 3's Verlet chain solve
/// on the pin line, with the per-link contact planes read from the field instead of casts: at each
/// link's midpoint outer-face station, a negative SDF yields a plane anchored at the projected
/// surface point (`p − ∇·depth`) with the field gradient as its normal. Kept verbatim so the
/// kinematic wrap can be felt against it on identical terrain (the step-22 review's one honest
/// loss is this chain's emergent slack migration); deleted once the wrap wins the feel check.
pub(super) fn conform_belts_field_chain(
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Transform)>,
    field: Res<TerrainField>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
    time: Res<Time>,
    mut memory: ResMut<ChainMemory>,
    mut belts: ResMut<ConformedBelts>,
    mut reference: ResMut<ChainReference>,
) {
    let hull = *hull;
    let affine = hull.affine();
    let to_local = affine.inverse();
    let dt = time.delta_secs().min(1.0 / 30.0);
    let dt2 = dt * dt;
    let g3 = to_local.transform_vector3(Vec3::NEG_Y * 9.81);
    let g2 = Vec2::new(g3.z, g3.y);
    for side in [Side::Left, Side::Right] {
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        // Wheel circles at the wheels' CURRENT (articulated) positions, inflated to the pin line.
        let circles: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _)| w.side == side)
            .map(|(w, t)| {
                let r = match w.kind {
                    WheelKind::Road => ROAD_RADIUS,
                    WheelKind::Sprocket | WheelKind::Idler => DRIVE_RADIUS,
                };
                (
                    Vec2::new(t.translation.z, t.translation.y),
                    r + TRACK_THICKNESS / 2.0,
                )
            })
            .collect();
        let mem = memory.get_mut(side);
        // The reference ring on the pin line, slack budget as model 2/3.
        let mut loop_pts = belt_loop(&pin_circles(), Some(pin_belt.length - mem.belly_extra));
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / pin_belt.count.max(1) as f32;
        let mut joints = resample(&loop_pts, pitch, phase.get(side).rem_euclid(pitch));
        joints.truncate(pin_belt.count);
        let n = joints.len();
        if n < 3 {
            continue;
        }

        // The advected reference ring in world space, for the `-` viz layer.
        let ref_world: Vec<Vec3> = joints
            .iter()
            .map(|j| affine.transform_point3(Vec3::new(track_x, j.y, j.x)))
            .collect();
        match side {
            Side::Left => reference.left = ref_world,
            Side::Right => reference.right = ref_world,
        }

        let ref_len: Vec<f32> = (0..n)
            .map(|i| (joints[(i + 1) % n] - joints[i]).length())
            .collect();

        // Per contacting link, its terrain plane (surface point + inward normal, hull-local 2D)
        // from the field at the link's midpoint outer-face station.
        let mut planes: Vec<Option<(Vec2, Vec2)>> = vec![None; n];
        let mut lifts = vec![0.0_f32; n];
        for i in 0..n {
            let a = joints[i];
            let b = joints[(i + 1) % n];
            let seg = b - a;
            let len = seg.length();
            if len < 1e-4 {
                continue;
            }
            let tan2 = seg / len;
            let out2 = Vec2::new(tan2.y, -tan2.x);
            let mid = (a + b) / 2.0;
            let w_mid = affine.transform_point3(Vec3::new(track_x, mid.y, mid.x));
            let wa3 = affine.transform_point3(Vec3::new(track_x, a.y, a.x));
            let wb3 = affine.transform_point3(Vec3::new(track_x, b.y, b.x));
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let axis3 = (wb3 - wa3).normalize_or_zero();
            let lat = out.cross(axis3);
            // The visual chain conforms to what the physics samples — ALL of it: the SAME
            // 3 lateral columns × 3 longitudinal stations the forces use. Laterally this closes
            // the view/physics inversion (a rigid 500 mm shoe over an off-center ridge is held
            // up across its full width); longitudinally the pins are SHARED with the
            // neighbouring links, so terrain-edge hand-offs spread over a full pitch. The lift
            // is the deepest VALUE (continuous); the plane anchor/normal are DEPTH-WEIGHTED
            // BLENDS over the penetrating stations — never the winning station's position,
            // which jumps up to the full sample spread when near-tied stations swap (the one
            // argmax consumption left in model 4; codex finding, step 21c).
            let face = out * (TRACK_THICKNESS / 2.0);
            let mut depth = 0.0_f32;
            let mut q_acc = Vec3::ZERO;
            let mut g_acc = Vec3::ZERO;
            let mut w_acc = 0.0_f32;
            for (offset, _) in COLUMNS {
                let shift = lat * offset;
                for base in [wa3, w_mid, wb3] {
                    let s = base + shift + face;
                    let d = field.depth_along(s, out);
                    if d > 0.0 {
                        depth = depth.max(d);
                        let surf = s - out * d;
                        q_acc += surf * d;
                        g_acc += field.gradient(surf) * d;
                        w_acc += d;
                    }
                }
            }
            if depth <= 0.0 || w_acc <= 0.0 {
                continue;
            }
            let surface = q_acc / w_acc;
            let grad = g_acc.normalize_or_zero();
            let q = to_local.transform_point3(surface);
            let m3 = to_local.transform_vector3(grad);
            let m2 = Vec2::new(m3.z, m3.y).normalize_or_zero();
            if m2 == Vec2::ZERO {
                continue;
            }
            planes[i] = Some((Vec2::new(q.z, q.y), m2));
            lifts[i] = depth;
        }

        // Slack bookkeeping for next frame (see model 2).
        let joint_lift = |i: usize| lifts[(i + n - 1) % n].max(lifts[i]);
        let extra: f32 = (0..n)
            .map(|i| {
                let d = joint_lift((i + 1) % n) - joint_lift(i);
                d * d / (2.0 * ref_len[i].max(1e-3))
            })
            .sum();
        mem.belly_extra = (mem.belly_extra * 0.8 + extra * 0.2).clamp(0.0, 0.5);

        // Verlet step + projections, identical to model 3.
        let shift = (phase.get(side) / pitch).floor() as i64;
        if mem.pos.len() == n {
            let rot = (shift - mem.shift).rem_euclid(n as i64) as usize;
            mem.pos.rotate_right(rot);
            mem.prev.rotate_right(rot);
        } else {
            mem.pos = joints.clone();
            mem.prev = joints.clone();
        }
        mem.shift = shift;
        let old_pos = mem.pos.clone();
        let mut p: Vec<Vec2> = (0..n)
            .map(|i| {
                let vel = (mem.pos[i] - mem.prev[i]) * CHAIN_DAMPING;
                // Anchor split into along/across the reference tangent (see the knob docs):
                // advection + drive stay violent, the transverse shape belongs to gravity.
                let tan = (joints[(i + 1) % n] - joints[(i + n - 1) % n]).normalize_or_zero();
                let delta = joints[i] - mem.pos[i];
                let along = tan * delta.dot(tan);
                let across = delta - along;
                mem.pos[i] + vel + (g2 + along * CHAIN_DRIVE + across * CHAIN_DRIVE_ACROSS) * dt2
            })
            .collect();
        for _ in 0..CHAIN_ITERATIONS {
            // (a) Rigid link lengths.
            for i in 0..n {
                let j = (i + 1) % n;
                let d = p[j] - p[i];
                let l = d.length();
                if l < 1e-6 {
                    continue;
                }
                let shift = d * ((l - ref_len[i]) / l * 0.5);
                p[i] += shift;
                p[j] -= shift;
            }
            // (b) Joint articulation cap.
            for i in 0..n {
                let prev = p[(i + n - 1) % n];
                let next = p[(i + 1) % n];
                let u = p[i] - prev;
                let v = next - p[i];
                let (lu, lv) = (u.length(), v.length());
                if lu < 1e-6 || lv < 1e-6 {
                    continue;
                }
                let ang = (u.dot(v) / (lu * lv)).clamp(-1.0, 1.0).acos();
                if ang > MAX_LINK_ANGLE {
                    let mid = (prev + next) / 2.0;
                    let t = ((ang - MAX_LINK_ANGLE) / ang).min(1.0) * 0.5;
                    p[i] = p[i].lerp(mid, t);
                }
            }
            // (b2) Weak bending stiffness at all angles (see [`CHAIN_BEND`]): erases the
            // compression zigzag, leaves sags and wraps.
            for i in 0..n {
                let mid = (p[(i + n - 1) % n] + p[(i + 1) % n]) / 2.0;
                p[i] = p[i].lerp(mid, CHAIN_BEND);
            }
            // (c) Terrain: pins stay t/2 inside each contacting link's plane.
            for (i, plane) in planes.iter().enumerate() {
                let Some((q, m)) = *plane else {
                    continue;
                };
                for idx in [i, (i + 1) % n] {
                    let v = (p[idx] - q).dot(m) - TRACK_THICKNESS / 2.0;
                    if v < 0.0 {
                        p[idx] -= m * v;
                    }
                }
            }
            // (d) Wheel circles (inflated to the pin line above).
            for pt in p.iter_mut() {
                for &(c, r) in &circles {
                    let d = *pt - c;
                    let l = d.length();
                    if l < r && l > 1e-6 {
                        *pt = c + d * (r / l);
                    }
                }
            }
        }

        mem.prev = old_pos;
        mem.pos = p.clone();

        let samples: Vec<BeltSample> = (0..n)
            .map(|i| BeltSample {
                local: p[i],
                world: affine.transform_point3(Vec3::new(track_x, p[i].y, p[i].x)),
            })
            .collect();
        match side {
            Side::Left => belts.left = samples,
            Side::Right => belts.right = samples,
        }
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
) {
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
