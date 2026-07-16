//! Box-belt model: model-2 chain dynamics with oriented shoe-shaped terrain contact.
//!
//! The chain solves on each shoe's pin line; wheel and terrain interfaces use inner and outer face
//! offsets respectively so force lever arms include shoe thickness. The shoe's cast shape is a
//! **pill** (capsule, radius = t/2) along the link rather than a sharp box: every contact lands on
//! a rounded surface, so the contact normal and penetration stay *continuous in pose* — a sharp
//! box flips which corner wins the cast discretely (the divergence class the sphere-cast
//! suspension fix killed in the game sim, and an MP rollback-resim requirement). Frozen as the
//! cast-oracle A/B baseline for model 4 (field-belt); the 500 mm width increment (edge columns,
//! design settled in HQ.md step 18/19) lands in whichever oracle wins the A/B.

use avian3d::prelude::ShapeCastConfig;

use super::model2::clipped_linear_piece;
use super::*;

/// Link (shoe) thickness (m): the T-34's cast shoe is ~40 mm between the ground face and the wheel
/// path. Half of it is the offset between the pin line and either face.
pub(super) const TRACK_THICKNESS: f32 = 0.04;

/// The shoe's cast shape: a longitudinal pill on the centerline — a capsule of radius t/2 whose
/// total extent along the link equals the link length, so its flat-ground footprint tiles the
/// chain exactly like the sliver box it replaced (outermost surface at pin + t/2 over the
/// cylindrical mid-section). The rounded ends make bump-corner hand-off a roll, not a drop.
fn shoe_pill(len: f32) -> Collider {
    let radius = TRACK_THICKNESS / 2.0;
    let half = (len / 2.0 - radius).max(0.0);
    Collider::capsule_endpoints(radius, Vec3::Z * -half, Vec3::Z * half)
}

// Chain-solve knobs: model 2's verified values, owned per model so the box-belt tunes independently.
/// Per-frame velocity retention of the chain's Verlet integration — the swing knob (see model 2).
const CHAIN_DAMPING: f32 = 0.88;
/// Drive-anchor stiffness (s⁻²) toward the advected reference (see model 2).
const CHAIN_DRIVE: f32 = 400.0;
/// Gauss–Seidel passes of the constraint projections (see model 2).
const CHAIN_ITERATIONS: usize = 20;
/// Max articulation between consecutive links (rad): must clear the T-34 sprocket's wrap demand of
/// ~31°/joint (see model 2).
const MAX_LINK_ANGLE: f32 = 35.0 * std::f32::consts::PI / 180.0;

/// MODEL 3's belt lives on the **pin line** — `rest_circles` inflated by t/2 — whose perimeter is
/// ~π·t longer than the belt-line loop. Reusing model 2's `BeltLength`/`LinkCount` would silently
/// eat most of the slack budget, so the pin belt owns its own length and link count.
#[derive(Resource, Default)]
pub(super) struct PinBelt {
    pub(super) length: f32,
    pub(super) count: usize,
}

pub(super) fn init_pin_belt(mut commands: Commands) {
    let length = polyline_len(&belt_loop(&pin_circles(), None)) + TRACK_SLACK;
    commands.insert_resource(PinBelt {
        length,
        count: (length / CONTACT_SPACING).round() as usize,
    });
}

/// The rest-pose wheel circles inflated to the pin line (radius + t/2): the wheels touch the inner
/// face, so the pins run a half-thickness outside every wheel surface. Shared with model 4 (the
/// field-belt rides the same pin line).
pub(super) fn pin_circles() -> Vec<(Vec2, f32)> {
    rest_circles()
        .iter()
        .map(|&(c, r)| (c, r + TRACK_THICKNESS / 2.0))
        .collect()
}

/// MODEL 3 belt contact — model 2's advected link ring on the **pin line**, each link contacting
/// terrain as an **oriented pill** (the shoe, see [`shoe_pill`]) instead of a zero-thickness
/// segment:
///
/// - **Detection = pill cast.** The pill is centred on the pin segment (thickness symmetric about
///   the pins) and cast from inside the loop along the link's outward normal; its first touch is
///   the deepest terrain feature under the shoe. The travel-distance convention makes the face
///   offset cancel: the origin backs off by `CONTACT_PROBE` and the pill's own radius rides along,
///   so `pen = PROBE − distance` measures penetration past the outer tangent (pin + t/2) with no
///   offset bookkeeping.
/// - **Pressure profile on the outer face.** Endpoint rays probe from the pins' outer-face points;
///   the same closed-form clipped profile as model 2 yields the resultant + centroid.
/// - **Force at the terrain surface.** Support + traction are applied at the centroid pushed out to
///   the outer face and back in by the centroid penetration — the real interface, so the lever arm
///   includes the shoe (and the contact dots land on the drawn outer line, not underground: the
///   penalty penetration is virtual compliance, the reference line rides ~sink inside terrain).
///
/// Support/traction/belt-speed dynamics are otherwise model 2's, per-metre × link length.
pub(super) fn apply_belt_support_boxes(
    mut hull: Query<(&GlobalTransform, Forces), With<Hull>>,
    spatial: SpatialQuery,
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

        // The fixed advected ring, on the pin line at the pin belt's own pitch (see model 2 for the
        // ring/advection rationale).
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
            let center = (wa + wb) / 2.0;
            let axis = (wb - wa) / len;
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let Ok(out_dir) = Dir3::new(out) else {
                continue;
            };
            let filter = SpatialQueryFilter::from_mask(Layer::Terrain);

            // The shoe: an oriented pill about the pin segment. Right-handed basis (lat × out =
            // axis): X = lateral, Y = outward (thickness), Z = along the link.
            let lat = out.cross(axis);
            let rot = Quat::from_mat3(&Mat3::from_cols(lat, out, axis));
            let shoe = shoe_pill(len);
            let Some(hit) = spatial.cast_shape(
                &shoe,
                center - out * CONTACT_PROBE,
                rot,
                out_dir,
                &ShapeCastConfig {
                    max_distance: CONTACT_PROBE + 0.02,
                    ..default()
                },
                &filter,
            ) else {
                continue;
            };
            // Penetration past the *outer face* (the offset cancels in the travel distance).
            let pen_max = CONTACT_PROBE - hit.distance;
            if pen_max <= 0.0 {
                continue;
            }
            let x_c = (hit.point1 - wa).dot(axis).clamp(0.0, len);

            // Endpoint penetrations along the same normal, probed from the pins' outer-face points
            // (the profile lives where the shoe meets the ground); may be ≤ 0 = that end clear.
            let end_pen = |w: Vec3| -> f32 {
                spatial
                    .cast_ray(
                        w + out * (TRACK_THICKNESS / 2.0 - CONTACT_PROBE),
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

            let (a1, m1, l1) = clipped_linear_piece(0.0, x_c, pen_a, pen_max);
            let (a2, m2, l2) = clipped_linear_piece(x_c, len, pen_max, pen_b);
            let (area, moment, contact_len) = (a1 + a2, m1 + m2, l1 + l2);
            if area <= 0.0 {
                continue;
            }
            // Resultant at the terrain surface: centroid on the pin line, pushed out to the outer
            // face, pulled back in by the centroid penetration.
            let pen_c = (pen_a.max(0.0) + pen_max) / 2.0;
            let p = wa + axis * (moment / area) + out * (TRACK_THICKNESS / 2.0 - pen_c);

            // (1) Support: penalty spring along the belt's own inward normal (see model 1/2).
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

            // (2) Traction: slip-saturated friction on the ellipse (see model 1/2).
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

            // Displayed load = the **elastic** component only (see model 2): the damping term
            // reads tick-scale micro-velocity and strobed the gizmos; the elastic term follows
            // penetration, stable at rest. Physics above uses the full load.
            contacts.0.push(Contact {
                local: to_local.transform_point3(p),
                load: SUPPORT_STIFFNESS_PER_M * area * engage,
                normal,
                slip: slip_long,
                traction,
            });
        }

        // Belt dynamics + advection, identical to model 2.
        let target = command * MAX_BELT_SPEED;
        let avail = engine_available(belt_speed);
        let engine = (BELT_GOVERNOR_GAIN * (target - belt_speed)).clamp(-avail, avail);
        let next = belt_speed + (engine - belt_reaction) / BELT_INERTIA * dt;
        belt.set(side, next.clamp(-MAX_BELT_SPEED, MAX_BELT_SPEED));
        phase.advance(side, belt_speed * dt);
    }
}

/// MODEL 3's conform — model 2's Verlet chain solve moved to the **pin line**. Same integration
/// (gravity in the hull frame + drive anchor, damped) and the same projections, with the offsets of
/// the box model:
///
/// - **wheel circles + t/2**: the pins can't come closer to a wheel centre than the inner face;
/// - **terrain planes hold the pins t/2 inside the contact plane** (`(p − q)·m ≥ t/2`): the outer
///   face rests on the ground, the pins ride a half-thickness above it;
/// - contact planes + lifts come from the same **pill casts** the physics uses.
///
/// Writes the shared [`ConformedBelts`]: its samples ARE the solved pin line (wheels ride it with a
/// +t/2 face offset in `articulate_wheels`; `draw_rig_gizmos` adds the outer-face companion line).
pub(super) fn conform_belts_boxes(
    hull: Single<&GlobalTransform, With<Hull>>,
    wheels: Query<(&RigWheel, &Transform)>,
    spatial: SpatialQuery,
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
        // Wheel circles at the wheels' CURRENT (articulated) positions, inflated to the pin line
        // (+t/2 — the inner face sits on the wheel surface).
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
        // The reference ring on the pin line, at the pin belt's own length/pitch; the top-run sag
        // budget lends whatever the belly currently demands (see model 2).
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

        // The advected reference ring in world space, for the `-` viz layer (the drive-anchor
        // target the Verlet chain is pulled toward).
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

        // Per contacting link, its terrain contact plane (point on the surface + inward normal)
        // from the pill cast at reference config; distance-0 casts (buried origin) yield no plane.
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
            let axis = (wb - wa) / len;
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let Ok(out_dir) = Dir3::new(out) else {
                continue;
            };
            let lat = out.cross(axis);
            let rot = Quat::from_mat3(&Mat3::from_cols(lat, out, axis));
            let shoe = shoe_pill(len);
            if let Some(hit) = spatial.cast_shape(
                &shoe,
                center - out * CONTACT_PROBE,
                rot,
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

        // Slack bookkeeping for next frame (see model 2).
        let joint_lift = |i: usize| lifts[(i + n - 1) % n].max(lifts[i]);
        let extra: f32 = (0..n)
            .map(|i| {
                let d = joint_lift((i + 1) % n) - joint_lift(i);
                d * d / (2.0 * ref_len[i].max(1e-3))
            })
            .sum();
        mem.belly_extra = (mem.belly_extra * 0.8 + extra * 0.2).clamp(0.0, 0.5);

        // Verlet step (see model 2): rotate state by the whole links that passed, integrate gravity
        // + the drive anchor, then project.
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
                mem.pos[i] + vel + (g2 + (joints[i] - mem.pos[i]) * CHAIN_DRIVE) * dt2
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
            // (c) Terrain: pins stay t/2 *inside* each contacting link's plane — the outer face
            // rests on the ground ((p − q)·m ≥ t/2).
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

/// The `9` viz layer: outline every shoe pill at the **physics** stations — the rigid reference
/// ring `apply_belt_support_boxes` casts from, NOT the solved visual chain — so where the physics
/// thinks the shoes are (and their tiling/scissor gaps) reads directly against the drawn chain.
pub(super) fn draw_cast_shapes(
    mut gizmos: Gizmos,
    viz: Res<VizLayers>,
    hull: Single<&GlobalTransform, With<Hull>>,
    pin_belt: Res<PinBelt>,
    phase: Res<BeltPhase>,
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
        // The exact station build of `apply_belt_support_boxes`.
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
            let center = (wa + wb) / 2.0;
            let axis = (wb - wa) / len;
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let lat = out.cross(axis);
            let rot = Quat::from_mat3(&Mat3::from_cols(lat, out, axis));
            let radius = TRACK_THICKNESS / 2.0;
            // The gizmo capsule's axis is local Y; ours is local Z — one +90° X-turn maps Y → Z.
            gizmos.primitive_3d(
                &Capsule3d {
                    radius,
                    half_length: (len / 2.0 - radius).max(0.0),
                },
                Isometry3d::new(
                    center,
                    rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                ),
                CAST_COLOR,
            );
        }
    }
}
