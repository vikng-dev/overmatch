//! MODEL 1 — belt-primary (tag `checkpoint/track-model-1`): the belt is the sole ground contact
//! *and* sole ground reader; stations fixed in hull space; wheels rigid to the hull, riding the
//! conformed belt cosmetically. Verified and frozen — model iteration happens in sibling files.

use super::*;

/// Belt contact — the core of the model. Sample the **whole** belt loop (not just the lower run) and,
/// at each station, probe along the belt's **outward normal** (down under the tracks, forward on the
/// front face, etc.). Wherever the belt meets terrain: (1) push back with a damped penalty spring
/// along the contact normal (**support**); (2) apply **slip-based friction** — `μ·load ×
/// saturate(slip / SLIP_SATURATION)` — where the belt's longitudinal drive axis is the belt-travel
/// direction (down the front face, so friction reacts *up* → grinding-climb), capped on the friction
/// ellipse (**traction**). The longitudinal friction reacts back on the belt, which the engine
/// governor drives, so wheelspin/skid/engine-braking/hill-hold emerge. One mechanism covers ground,
/// walls, ledges, and ditch faces alike.
pub(super) fn apply_belt_support(
    mut hull: Query<(&GlobalTransform, Forces), With<Hull>>,
    spatial: SpatialQuery,
    input: Res<DriveInput>,
    time: Res<Time>,
    mut belt: ResMut<BeltSpeed>,
    mut contacts: ResMut<BeltContacts>,
) {
    let Ok((hull_gt, mut forces)) = hull.single_mut() else {
        return;
    };
    let affine = hull_gt.affine();
    contacts.0.clear(); // the sole contact system now — nothing ran before us this tick
    let dt = time.delta_secs();

    // Per-station support coefficients = per-metre × the arc-length each station represents, so the
    // totals are independent of `CONTACT_SPACING` (resolution decoupled from the physics).
    let k = SUPPORT_STIFFNESS_PER_M * CONTACT_SPACING;
    let c = SUPPORT_DAMPING_PER_M * CONTACT_SPACING;

    for side in [Side::Left, Side::Right] {
        // Physics belt = the hull-fixed rigid taut line (`rest_circles`), NOT the cosmetically-draped
        // wheels — otherwise draping the wheels onto terrain would flatten the line onto the ground and
        // null the penetration that carries the tank. Terrain rising above this rigid line generates
        // support; terrain dropping below it is bridged straight.
        let track_x = match side {
            Side::Left => -TRACK_HALF_WIDTH,
            Side::Right => TRACK_HALF_WIDTH,
        };
        let circles = rest_circles();
        // Additive differential: steer adds to the left track, subtracts from the right, so a pure
        // steer pivots in place and a steer biases the turn the same way at any throttle.
        let command = match side {
            Side::Left => input.throttle + input.steer,
            Side::Right => input.throttle - input.steer,
        }
        .clamp(-1.0, 1.0);
        let belt_speed = belt.get(side); // this tick's belt surface speed (constant over the loop)
        // Sum the longitudinal ground friction across this side's belt stations so the belt-speed
        // integrator sees the full ground reaction (traction is all on the belt now).
        let mut belt_reaction = 0.0;

        // The full closed belt loop, resampled at uniform spacing. Close it (append the first point)
        // so the seam has a segment, then use modular indices for the tangent.
        let mut loop_pts = belt_loop(&circles, None);
        if let Some(&first) = loop_pts.first() {
            loop_pts.push(first);
        }
        let stations = resample(&loop_pts, CONTACT_SPACING, 0.0);
        let n = stations.len();
        if n < 3 {
            continue;
        }

        for i in 0..n {
            let point = stations[i];
            // Belt tangent (loop-traversal direction) and outward normal, both in the side plane.
            // Winding is CCW in (z, y), so the outward normal is the tangent rotated −90°.
            let tan2 = (stations[(i + 1) % n] - stations[(i + n - 1) % n]).normalize_or_zero();
            if tan2 == Vec2::ZERO {
                continue;
            }
            let out2 = Vec2::new(tan2.y, -tan2.x);

            let p = affine.transform_point3(Vec3::new(track_x, point.y, point.x));
            // Side-plane (z, y) direction → world: local (x = 0, y = v.y, z = v.x).
            let out = affine
                .transform_vector3(Vec3::new(0.0, out2.y, out2.x))
                .normalize_or_zero();
            let Ok(out_dir) = Dir3::new(out) else {
                continue;
            };

            // Probe from just inside the belt surface, outward, for terrain the belt has met.
            let origin = p - out * CONTACT_PROBE;
            let Some(hit) = spatial.cast_ray(
                origin,
                out_dir,
                CONTACT_PROBE + 0.02,
                true,
                &SpatialQueryFilter::from_mask(Layer::Terrain),
            ) else {
                continue;
            };
            // Penetration of terrain past the belt surface. No deadband: the belt is the sole carrier
            // now, so on flat ground it settles at a small continuous sink (no parallel wheel springs
            // holding it at the surface to buzz against), and every grounded station carries its share.
            let pen = CONTACT_PROBE - hit.distance;
            if pen <= 0.0 {
                continue;
            }

            // (1) Support: penalty spring along the **belt's own inward normal** (−outward), NOT the
            // terrain hit-normal. The belt normal is smooth (from the spline), whereas the terrain
            // normal flips between "up" and "sideways" when a ray lands on an edge (a ditch lip),
            // which shoved the rig in alternating directions and made it chatter/wedge. `−out` still
            // pushes off a wall (outward points into it) and up off the ground; only the direction is
            // stabilised. Damped by the hull's speed along it.
            let normal = -out;
            let vel = forces.velocity_at_point(p);
            // Soft engagement: ramp the whole contact force in over the first CONTACT_ENGAGE metres of
            // penetration, so a station crossing the belt surface eases its force from zero instead of
            // snapping a large force on/off (which see-sawed the rigid rig at rest). Full force once
            // well engaged (the resting flat run sits far past this).
            let engage = (pen / CONTACT_ENGAGE).clamp(0.0, 1.0);
            let load = (k * pen - c * vel.dot(normal)).max(0.0) * engage;
            if load <= 0.0 {
                continue;
            }
            forces.apply_force_at_point(normal * load, p);

            // (2) Traction. The belt's drive axis is the belt-travel direction (−tangent: belt_speed
            // > 0 lays ground backward), projected into the contact plane; lateral is across it. Slip
            // is belt speed minus the ground's speed along the drive axis; friction saturates at
            // μ·load. On the front face the drive axis points *up*, so a spinning belt climbs.
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
                belt_reaction += f_long; // the belt feels the longitudinal friction as a load
            }

            contacts.0.push(Contact {
                local: Vec3::new(track_x, point.y, point.x),
                load,
                normal,
                slip: slip_long,
                traction,
            });
        }

        // Belt dynamics: the engine governor chases the commanded belt speed with force limited to
        // the drivetrain's constant-power curve; the ground friction reaction opposes it. When the
        // engine out-muscles the available grip the belt over-spins the ground → wheelspin;
        // otherwise they find rolling.
        let target = command * MAX_BELT_SPEED;
        let avail = engine_available(belt_speed);
        let engine = (BELT_GOVERNOR_GAIN * (target - belt_speed)).clamp(-avail, avail);
        let next = belt_speed + (engine - belt_reaction) / BELT_INERTIA * dt;
        belt.set(side, next.clamp(-MAX_BELT_SPEED, MAX_BELT_SPEED));
    }
}
