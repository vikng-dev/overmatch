//! MODEL 2 — link-belt: iteration on model 1 where the belt is a ring of **virtual track links**
//! that travel with the belt (per-side arc-phase advanced by belt speed) and contact the terrain as
//! **rigid plates** (segment casts, not point rays). Steps 1 (advected stations) and 2 (plate
//! contact) built; next: link rendering.

use avian3d::prelude::ShapeCastConfig;

use super::*;

/// MODEL 2: each side's **total** belt travel (m) along the reference loop — advanced by belt speed
/// each tick so the sampling stations travel with the belt like real links. Kept unwrapped: users
/// wrap it mod the link pitch for the sampling offset, and its quotient is the **link-identity
/// shift** the chain warm-start needs (how many whole links have passed).
#[derive(Resource, Default)]
pub(super) struct BeltPhase {
    left: f32,
    right: f32,
}

impl BeltPhase {
    fn get(&self, side: Side) -> f32 {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }
    fn advance(&mut self, side: Side, ds: f32) {
        match side {
            Side::Left => self.left += ds,
            Side::Right => self.right += ds,
        }
    }
}

/// How much of last frame's displacement seeds the next solve (the warm-start decay). The
/// projection constraints (lengths, non-penetration, circles) are satisfied by *infinitely many*
/// shapes — a full-strength warm start made every feasible configuration a **fixed point** (a
/// deformed chain floated in place forever; at speed, mangled seeds compounded frame-over-frame
/// into the "flung outward" balloon). Decaying the seed toward the taut reference is the missing
/// **tension**: deformations relax away in ~1/(1−α) frames unless terrain actively holds them,
/// while enough memory survives to keep bistable tents from flipping.
const CHAIN_MEMORY: f32 = 0.8;

/// Last frame's solved chain per side — the warm start. The projection solve is quasi-static and
/// tent configurations over corners are often bistable; solving fresh each frame let tiny input
/// changes flip between them (the config-snapping). Seeding from the previous solution (decayed —
/// see [`CHAIN_MEMORY`]) keeps the solver in the basin it settled in — **hysteresis, the cheap
/// stable form of chain inertia** (actual chain dynamics — mass, momentum, wobble — stay
/// deliberately out at this tier).
#[derive(Resource, Default)]
pub(super) struct ChainMemory {
    left: ChainSideMemory,
    right: ChainSideMemory,
}

#[derive(Default)]
struct ChainSideMemory {
    /// Solved displacement from the reference joint, per link index (hull-local side plane).
    disp: Vec<Vec2>,
    /// The link-identity shift (total travel / pitch) the stored solution corresponds to.
    shift: i64,
    /// Extra path length (m) the terrain currently demands of the belly (smoothed) — subtracted
    /// from the top run's sag budget when building the next reference ring, so the **top half of
    /// the chain lends its slack** when the belly tents over terrain (and sags back when it
    /// doesn't). Without this the surplus had nowhere to go and parked as belly squiggles.
    belly_extra: f32,
}

impl ChainMemory {
    fn get_mut(&mut self, side: Side) -> &mut ChainSideMemory {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }
}

/// MODEL 2 belt contact — a fork of [`apply_belt_support`] where the belt is a ring of **virtual
/// track links**, in two mechanisms:
///
/// 1. **Advected** ([`BeltPhase`]): the ring's arc-phase advances with belt speed, so the links
///    travel around the loop. Real link kinematics fall out — rolling without slip = contact points
///    stationary on the ground while the hull passes over (model 1's fixed-in-hull stations scrub
///    along the ground instead); wheelspin/skid = links visibly sliding.
/// 2. **Plate contact**: each link (the segment between consecutive stations) is cast as a rigid
///    plate along its outward normal (`cast_shape` with a segment collider) instead of probing a
///    point per station. The cast finds where the plate *first touches* — including a terrain corner
///    that pokes up **between** the stations, which point rays are structurally blind to (the clip
///    the user spotted) — and support/traction are applied **at that true contact point**, so a link
///    rests on a corner with its load there, like a real track plate.
///
/// Support/traction/belt-speed dynamics are otherwise model 1's; coefficients are per-metre × the
/// actual link length (the seam link is shorter — it carries proportionally less).
pub(super) fn apply_belt_support_links(
    mut hull: Query<(&GlobalTransform, Forces), With<Hull>>,
    spatial: SpatialQuery,
    input: Res<DriveInput>,
    time: Res<Time>,
    count: Res<LinkCount>,
    mut belt: ResMut<BeltSpeed>,
    mut phase: ResMut<BeltPhase>,
    mut contacts: ResMut<BeltContacts>,
) {
    let Ok((hull_gt, mut forces)) = hull.single_mut() else {
        return;
    };
    let affine = hull_gt.affine();
    let to_local = affine.inverse();
    contacts.0.clear(); // the sole contact system now — nothing ran before us this tick
    let dt = time.delta_secs();

    for side in [Side::Left, Side::Right] {
        // Physics belt = the hull-fixed rigid taut line (`rest_circles`); see model 1.
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        let circles = rest_circles();
        // Additive differential: steer adds to the left track, subtracts from the right.
        let command = match side {
            Side::Left => input.throttle + input.steer,
            Side::Right => input.throttle - input.steer,
        }
        .clamp(-1.0, 1.0);
        let belt_speed = belt.get(side); // this tick's belt surface speed (constant over the loop)
        let mut belt_reaction = 0.0;

        // The full closed belt loop, sampled as the **fixed ring of `LinkCount` links** at its exact
        // pitch (loop length / count — no phase-dependent remainder link), at the *advected*
        // positions: the phase advances with belt speed below, so the links travel around the loop.
        // Loop-traversal direction (CCW in (z, y)) is the direction the belt surface moves when
        // driving forward, so `phase += belt_speed·dt` circulates the ring the right way.
        let mut loop_pts = belt_loop(&circles, None);
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / count.0.max(1) as f32;
        let mut stations = resample(&loop_pts, pitch, phase.get(side).rem_euclid(pitch));
        stations.truncate(count.0);
        let n = stations.len();
        if n < 3 {
            continue;
        }

        // Each link = the segment between consecutive stations (modular: the seam segment closes the
        // ring; a degenerate seam is skipped). The link is a rigid plate on penalty ground: it has a
        // **pressure distribution** along its length, and the resultant force acts at the pressure
        // **centroid**. The profile is reconstructed piecewise-linearly from three probes — the two
        // endpoint penetrations (rays) and the plate cast's deepest point — and integrated in closed
        // form. On flat ground the centroid is the link centre; on a corner it moves to the corner.
        // (Applying the whole load at the *cast's contact point* instead was degenerate on coplanar
        // contact — parry picks arbitrarily among tied points, so the point flipped between the
        // plate's ends tick-to-tick: teleporting dots + flickering torque = the observed jitter.)
        for i in 0..n {
            let a = stations[i];
            let b = stations[(i + 1) % n];
            let seg = b - a;
            let len = seg.length();
            if len < 1e-4 {
                continue;
            }
            // Link tangent (loop-traversal direction) and outward normal, in the side plane.
            // Winding is CCW in (z, y), so the outward normal is the tangent rotated −90°.
            let tan2 = seg / len;
            let out2 = Vec2::new(tan2.y, -tan2.x);

            // Side-plane (z, y) → world: local (x = 0, y = v.y, z = v.x).
            let wa = affine.transform_point3(Vec3::new(track_x, a.y, a.x));
            let wb = affine.transform_point3(Vec3::new(track_x, b.y, b.x));
            let center = (wa + wb) / 2.0;
            let axis = (wb - wa) / len;
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let Ok(out_dir) = Dir3::new(out) else {
                continue;
            };
            let filter = SpatialQueryFilter::from_mask(Layer::Terrain);

            // Cast the plate from just inside the belt surface, outward: its first touch is the
            // deepest terrain feature under the link — including a corner *between* the stations,
            // which endpoint rays are structurally blind to. The segment collider is expressed about
            // the link centre (identity rotation: its local endpoints are already world-oriented).
            let plate = Collider::segment(wa - center, wb - center);
            let Some(hit) = spatial.cast_shape(
                &plate,
                center - out * CONTACT_PROBE,
                Quat::IDENTITY,
                out_dir,
                &ShapeCastConfig {
                    max_distance: CONTACT_PROBE + 0.02,
                    ..default()
                },
                &filter,
            ) else {
                continue;
            };
            let pen_max = CONTACT_PROBE - hit.distance;
            if pen_max <= 0.0 {
                continue;
            }
            // Where along the link the deepest contact sits (the axial coordinate is invariant to
            // the normal offset of the cast).
            let x_c = (hit.point1 - wa).dot(axis).clamp(0.0, len);

            // Endpoint penetrations along the same normal (may be ≤ 0 = that end clear of ground),
            // clamped to the cast's depth (the cast stops at first touch, so nothing exceeds it).
            let end_pen = |w: Vec3| -> f32 {
                spatial
                    .cast_ray(
                        w - out * CONTACT_PROBE,
                        out_dir,
                        CONTACT_PROBE + 0.02,
                        true,
                        &filter,
                    )
                    .map_or(f32::NEG_INFINITY, |h| CONTACT_PROBE - h.distance)
                    .min(pen_max)
            };
            let pen_a = end_pen(wa);
            let pen_b = end_pen(wb);

            // Integrate the clipped piecewise-linear pressure profile (0, pen_a) → (x_c, pen_max) →
            // (len, pen_b): area A (∫pen dx), first moment M (∫x·pen dx), contacting length lc.
            let (a1, m1, l1) = clipped_linear_piece(0.0, x_c, pen_a, pen_max);
            let (a2, m2, l2) = clipped_linear_piece(x_c, len, pen_max, pen_b);
            let (area, moment, contact_len) = (a1 + a2, m1 + m2, l1 + l2);
            if area <= 0.0 {
                continue;
            }
            // Resultant position: the pressure centroid, on the belt line (matching model 1, which
            // applies at the station on the belt surface).
            let p = wa + axis * (moment / area);

            // (1) Support: penalty spring along the belt's own inward normal, soft-engaged (see
            // model 1 for both rationales). Elastic term = stiffness per metre × ∫pen (the profile
            // area); damping over the contacting length; engagement ramps on the deepest point.
            let normal = -out;
            let vel = forces.velocity_at_point(p);
            let engage = (pen_max / CONTACT_ENGAGE).clamp(0.0, 1.0);
            let load = (SUPPORT_STIFFNESS_PER_M * area
                - SUPPORT_DAMPING_PER_M * contact_len * vel.dot(normal))
            .max(0.0)
                * engage;
            if load <= 0.0 {
                continue;
            }
            forces.apply_force_at_point(normal * load, p);

            // (2) Traction: slip-saturated friction on the ellipse, drive axis = belt-travel
            // direction projected into the contact plane (see model 1).
            let mut slip_long = 0.0;
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
                forces.apply_force_at_point(long_dir * f_long + lat_dir * f_lat, p);
                belt_reaction += f_long; // the belt feels the longitudinal friction as a load
            }

            contacts.0.push(Contact {
                local: to_local.transform_point3(p),
                load,
                normal,
                slip: slip_long,
            });
        }

        // Belt dynamics (identical to model 1: governor under the constant-power curve), then
        // advect the link ring by the belt's motion this tick — the step that makes the stations
        // *travel*.
        let target = command * MAX_BELT_SPEED;
        let avail = engine_available(belt_speed);
        let engine = (BELT_GOVERNOR_GAIN * (target - belt_speed)).clamp(-avail, avail);
        let next = belt_speed + (engine - belt_reaction) / BELT_INERTIA * dt;
        belt.set(side, next.clamp(-MAX_BELT_SPEED, MAX_BELT_SPEED));
        phase.advance(side, belt_speed * dt);
    }
}

/// Iterations of the chain projection solve. Each Gauss–Seidel pass projects link lengths, joint
/// articulation, terrain planes, and wheel circles once; enough passes converge to visual tolerance
/// (the solve restarts from the reference ring every frame — no temporal state, no drift; but an
/// under-converged solve reads as snapping between configurations, so err high — it's cheap).
const CHAIN_ITERATIONS: usize = 20;

/// Max articulation between consecutive links (rad): a real track pin can't fold further. Uncapped,
/// a *compressed* span (nosed into a wall, clumped between wheels) buckles into zigzags — length
/// constraints alone permit any fold. Must sit above the tightest wheel-wrap demand (the T-34's
/// small sprocket: pitch/radius ≈ 0.172/0.32 ≈ 31° per joint), so 35°.
const MAX_LINK_ANGLE: f32 = 35.0 * std::f32::consts::PI / 180.0;

/// The fixed number of links in each track's ring. A real track has a fixed link count; resampling
/// the ring per frame instead left a phase-dependent *remainder link* at the loop seam — which sits
/// at the sprocket — whose length snapped as the phase wrapped (the sprocket/idler spikes). Computed
/// once at startup: the belt length at the target pitch, rounded to a whole ring.
#[derive(Resource, Default)]
pub(super) struct LinkCount(usize);

pub(super) fn init_link_count(mut commands: Commands) {
    let length = polyline_len(&belt_loop(&rest_circles(), None)) + TRACK_SLACK;
    commands.insert_resource(LinkCount((length / CONTACT_SPACING).round() as usize));
    commands.insert_resource(ChainMemory::default());
}

/// MODEL 2's conform — the belt drawn as a true **chain of rigid links** on the same advected ring
/// the physics samples (same pitch, same phase: the drawn segments ARE the physical links,
/// travelling with the belt). A small positional projection solve (PBD-style, in the 2D side plane)
/// enforces what a steel chain is kinematically:
///
/// - **fixed link lengths** (each segment projected back to its reference length — a link that
///   tents over a corner *stays link-sized*, pulling the needed length from its neighbours and
///   ultimately the slack top run, instead of stretching);
/// - **terrain non-penetration** (each contacting link's plate cast yields a contact plane; the
///   link's joints are projected out of it);
/// - **wheel wrap as a constraint, not a drawing** (joints can't enter the wheel circles — tension
///   pulls the chain taut around them and the wrap emerges).
///
/// Writes the shared [`ConformedBelts`] (the drawn spline and the wheels ride it), replacing model
/// 1's per-point `conform_belts` while the link-belt model is active. The physics is untouched — it
/// samples the reference ring; this is the *rendering-integrity* half of the link model.
pub(super) fn conform_belts_links(
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Transform)>,
    spatial: SpatialQuery,
    belt_length: Res<BeltLength>,
    phase: Res<BeltPhase>,
    count: Res<LinkCount>,
    mut memory: ResMut<ChainMemory>,
    mut belts: ResMut<ConformedBelts>,
) {
    let hull = *hull;
    let affine = hull.affine();
    let to_local = affine.inverse();
    for side in [Side::Left, Side::Right] {
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        // Wheel circles at the wheels' CURRENT (articulated) positions — the wheels the chain must
        // wrap are the ones being drawn, not the rest pose (constraining against rest circles let
        // the chain notch through lifted wheels at a ledge). One frame stale (wheels ride last
        // frame's chain); converges visually.
        let circles: Vec<(Vec2, f32)> = wheels
            .iter()
            .filter(|(w, _)| w.side == side)
            .map(|(w, t)| {
                let r = match w.kind {
                    WheelKind::Road => ROAD_RADIUS,
                    WheelKind::Sprocket | WheelKind::Idler => DRIVE_RADIUS,
                };
                (Vec2::new(t.translation.z, t.translation.y), r)
            })
            .collect();
        let mem = memory.get_mut(side);
        // The same fixed advected link ring the physics uses: exactly `LinkCount` joints at the
        // loop's exact pitch (no phase-dependent remainder link — the old resample seam sat at the
        // sprocket and its snapping length spiked the chain there). The top-run sag budget gives up
        // whatever extra path the belly currently demands of the terrain (`belly_extra`) — the top
        // half of the chain lends its slack, instead of the surplus parking as belly squiggles.
        let mut loop_pts = belt_loop(&rest_circles(), Some(belt_length.0 - mem.belly_extra));
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let pitch = polyline_len(&loop_pts) / count.0.max(1) as f32;
        let mut joints = resample(&loop_pts, pitch, phase.get(side).rem_euclid(pitch));
        joints.truncate(count.0);
        let n = joints.len();
        if n < 3 {
            continue;
        }

        // The rigid link lengths: the reference ring's segment lengths, preserved exactly (arc
        // chords and the seam link included).
        let ref_len: Vec<f32> = (0..n)
            .map(|i| (joints[(i + 1) % n] - joints[i]).length())
            .collect();

        // Per contacting link, its terrain contact plane in the local side plane: a point on the
        // terrain surface (the plate cast's hit, taken at reference config) and the link's inward
        // normal. Distance-0 casts (buried origin, extreme clip mid-transient) yield no plane — the
        // surface is unknowable from there; the physics pushes the rig out.
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
            let wa = affine.transform_point3(Vec3::new(track_x, a.y, a.x));
            let wb = affine.transform_point3(Vec3::new(track_x, b.y, b.x));
            let center = (wa + wb) / 2.0;
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let Ok(out_dir) = Dir3::new(out) else {
                continue;
            };
            let plate = Collider::segment(wa - center, wb - center);
            if let Some(hit) = spatial.cast_shape(
                &plate,
                center - out * CONTACT_PROBE,
                Quat::IDENTITY,
                out_dir,
                &ShapeCastConfig {
                    max_distance: CONTACT_PROBE,
                    ..default()
                },
                &SpatialQueryFilter::from_mask(Layer::Terrain),
            ) && hit.distance > 0.0
                && hit.distance < CONTACT_PROBE
            {
                let q = to_local.transform_point3(hit.point1);
                planes[i] = Some((Vec2::new(q.z, q.y), -out2));
                lifts[i] = CONTACT_PROBE - hit.distance;
            }
        }

        // Slack bookkeeping for next frame: the extra path length the lifted belly wants, to first
        // order Σ (Δ joint-lift)² / 2ℓ per link (a uniform raise adds ~nothing; differential lift —
        // a tent — is what consumes length). Smoothed, then lent by the top-run sag above.
        let joint_lift = |i: usize| lifts[(i + n - 1) % n].max(lifts[i]);
        let extra: f32 = (0..n)
            .map(|i| {
                let d = joint_lift((i + 1) % n) - joint_lift(i);
                d * d / (2.0 * ref_len[i].max(1e-3))
            })
            .sum();
        mem.belly_extra = (mem.belly_extra * 0.8 + extra * 0.2).clamp(0.0, 0.5);

        // The projection solve, warm-started from last frame's solution (index-rotated by how many
        // whole links have passed, so each stored displacement seeds the same physical link). This
        // is the hysteresis that stops bistable tent configurations from flipping frame-to-frame.
        let shift = (phase.get(side) / pitch).floor() as i64;
        let mut p = joints.clone();
        if mem.disp.len() == n {
            let rot = (shift - mem.shift).rem_euclid(n as i64) as usize;
            mem.disp.rotate_right(rot);
            for (pt, d) in p.iter_mut().zip(&mem.disp) {
                *pt += *d * CHAIN_MEMORY;
            }
        }
        for _ in 0..CHAIN_ITERATIONS {
            // (a) Rigid link lengths: split each segment's error between its joints.
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
            // (b) Joint articulation cap: consecutive links can't fold sharper than
            // MAX_LINK_ANGLE (real pins can't; uncapped, compressed spans buckle into zigzags).
            // Ease the joint toward its neighbours' midpoint, proportional to the excess fold.
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
            // (c) Terrain: project each contacting link's joints out of its contact plane
            // ((p − q)·m ≥ 0, m = the link's inward normal).
            for (i, plane) in planes.iter().enumerate() {
                let Some((q, m)) = *plane else {
                    continue;
                };
                for idx in [i, (i + 1) % n] {
                    let v = (p[idx] - q).dot(m);
                    if v < 0.0 {
                        p[idx] -= m * v;
                    }
                }
            }
            // (d) Wheel circles: the chain can't enter the running gear; tension wraps it around.
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

        // Remember the solution (as displacements off the reference) for next frame's warm start.
        mem.shift = shift;
        mem.disp = (0..n).map(|i| p[i] - joints[i]).collect();

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

/// Integrate `max(0, pen(x))` over one linear piece of a pressure profile: `pen` runs `p0 → p1`
/// across `[x0, x1]`. Returns `(∫pen dx, ∫x·pen dx, contacting length)`, clipping the sub-range
/// where the profile is negative (that part of the plate is clear of the ground). Closed form, so
/// the plate's resultant force and centroid are smooth functions of pose — no sampling noise.
fn clipped_linear_piece(x0: f32, x1: f32, p0: f32, p1: f32) -> (f32, f32, f32) {
    let w = x1 - x0;
    if w <= 0.0 || (p0 <= 0.0 && p1 <= 0.0) {
        return (0.0, 0.0, 0.0);
    }
    if p0 >= 0.0 && p1 >= 0.0 {
        // Trapezoid: A = w·(p0+p1)/2; M = ∫x·pen dx with pen linear in x.
        let area = w * (p0 + p1) / 2.0;
        let moment = w * (p0 * (2.0 * x0 + x1) + p1 * (x0 + 2.0 * x1)) / 6.0;
        return (area, moment, w);
    }
    // One end negative: clip at the zero crossing and integrate the positive triangle.
    let xc = x0 + w * (p0 / (p0 - p1));
    if p0 > 0.0 {
        clipped_linear_piece(x0, xc, p0, 0.0)
    } else {
        clipped_linear_piece(xc, x1, 0.0, p1)
    }
}
