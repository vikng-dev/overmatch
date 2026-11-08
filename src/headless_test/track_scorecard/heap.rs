//! Line 8c. Loose heap (the game's tank driving physics, game state only): the track sandbox's
//! rubble yard pile, built by the yard's own builder ([`crate::track_sandbox::rubble::build_yard`]),
//! settled asleep, then driven straight over its centre at two throttles.

use super::*;

/// Resolution: the longest line 8c waits for the heap to fall asleep before it drives (s); the
/// wait ends on the tick every piece sleeps.
const HEAP_SETTLE_S: usize = 30;
/// Fixture: the horizontal travel past which line 8c counts a heap piece as moved (m).
const HEAP_MOVED_M: f32 = 0.5;
/// Resolution: the spacing of line 8c's downward ray grid over the heap (m), a small fraction of
/// a piece.
const HEAP_RAY_GRID_M: f32 = 0.05;

/// The hull-local z of the tank's rear-most belt point (m, forward is −z).
fn rear_z(geom: &RigGeom) -> f32 {
    Side::ALL
        .iter()
        .flat_map(|&side| geom.rest.get(side).iter())
        .map(|&(c, r)| c.x + r + geom.model.pin_to_outer)
        .fold(f32::NEG_INFINITY, f32::max)
}

/// The heap as a strip one track gauge wide about the drive line (x = 0) meets it, from a grid
/// of rays cast down onto the pieces: (the highest piece surface above the ground, the strip's
/// near and far piece edges along the drive as z). `None` while no piece lies in the strip.
fn heap_strip(bed: &Bed, half_gauge: f32) -> Option<(f32, f32, f32)> {
    let world = bed.app.world();
    let bodies: Vec<(&Collider, Position, Rotation, ColliderAabb)> = bed
        .pieces
        .iter()
        // A piece the destruction model removed has no surface.
        .filter_map(|&p| {
            Some((
                world.get::<Collider>(p)?,
                *world.get::<Position>(p)?,
                *world.get::<Rotation>(p)?,
                *world.get::<ColliderAabb>(p)?,
            ))
        })
        .filter(|(.., aabb)| aabb.max.x >= -half_gauge && aabb.min.x <= half_gauge)
        .collect();
    let (lo, hi) = bodies
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (.., a)| {
            (lo.min(a.min.z), hi.max(a.max.z))
        });
    let (mut top, mut near, mut far) = (f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY);
    let columns = (2.0 * half_gauge / HEAP_RAY_GRID_M).ceil() as usize;
    let rows = ((hi - lo).max(0.0) / HEAP_RAY_GRID_M).ceil() as usize;
    for i in 0..=columns {
        let x = (-half_gauge + i as f32 * HEAP_RAY_GRID_M).min(half_gauge);
        for k in 0..=rows {
            let z = lo + k as f32 * HEAP_RAY_GRID_M;
            for (collider, at, rot, aabb) in &bodies {
                if !(aabb.min.x..=aabb.max.x).contains(&x)
                    || !(aabb.min.z..=aabb.max.z).contains(&z)
                {
                    continue;
                }
                let origin = Vec3::new(x, aabb.max.y + HEAP_RAY_GRID_M, z);
                let reach = aabb.max.y - aabb.min.y + 2.0 * HEAP_RAY_GRID_M;
                if let Some((t, _)) = collider.cast_ray(*at, *rot, origin, Vec3::NEG_Y, reach, true)
                {
                    top = top.max(origin.y - t);
                    near = near.max(z);
                    far = far.min(z);
                }
            }
        }
    }
    top.is_finite().then_some((top, near, far))
}

/// Line 8c (module doc).
pub(super) fn line(card: &mut Card) {
    use crate::track_sandbox::rubble;
    let kind = rubble::yard_source();
    // The tank's hull origin stands where the sandbox's `R` tour parks it: facing −z at the pile.
    let centre = Vec3::NEG_Z * rubble::spot_ahead_m();
    let mut rows = Vec::new();
    let mut piece_count = 0;
    for throttle in [0.6, 1.0] {
        let mut bed = Bed::on_ground(Some(base_ground()), flat());
        for piece in rubble::build_yard(&kind, centre).pile {
            let id = bed.app.world_mut().spawn(piece.body).id();
            bed.pieces.push(id);
        }
        piece_count = bed.pieces.len();
        bed.place(Vec3::ZERO, Quat::IDENTITY);
        bed.start();
        let mut asleep_after = f32::NAN;
        for tick in 1..=HEAP_SETTLE_S * TICKS {
            bed.tick(0.0, 0.0);
            if bed.awake_pieces().is_empty() {
                asleep_after = tick as f32 * DT;
                break;
            }
        }
        let awake_at_drive = bed.awake_pieces().len();
        let geom = bed.geom();
        let half_gauge =
            (geom.grip_columns(Side::Left)[1].0 - geom.grip_columns(Side::Right)[1].0).abs() / 2.0;
        let rear = rear_z(geom);
        let (top_before, _, far) =
            heap_strip(&bed, half_gauge).expect("the yard's heap lies across the drive line");
        let settled: Vec<Vec3> = bed
            .pieces
            .iter()
            .map(|&p| {
                bed.app
                    .world()
                    .get::<Position>(p)
                    .expect("a settled piece")
                    .0
            })
            .collect();
        let begin = bed.hull();
        let heading = begin.forward().with_y(0.0).normalize();
        let (mut rise_max, mut rise_centre, mut reached) = (0.0_f32, f32::NAN, f32::NAN);
        let (mut nose_up, mut nose_down) = (0.0_f32, 0.0_f32);
        let mut driven = 0;
        while driven < 15 * TICKS {
            bed.tick(throttle, 0.0);
            driven += 1;
            let hull = bed.hull();
            let rise = hull.position.y - begin.position.y;
            rise_max = rise_max.max(rise);
            nose_up = nose_up.max(hull.pitch());
            nose_down = nose_down.max(-hull.pitch());
            if reached.is_nan() && hull.position.z < centre.z {
                reached = driven as f32 * DT;
                rise_centre = rise;
            }
            // The drive ends once the belt's rear point clears the heap's far edge.
            if (hull.position + hull.rotation * Vec3::Z * rear).z < far {
                break;
            }
        }
        let top_after = heap_strip(&bed, half_gauge).map_or(f32::NAN, |(top, ..)| top);
        let world = bed.app.world();
        let (mut moved, mut travel_sum, mut travel_max, mut removed) = (0, 0.0_f32, 0.0_f32, 0);
        let (mut forward, mut back, mut sideways) = (0, 0, 0);
        for (&p, at) in bed.pieces.iter().zip(&settled) {
            let Some(now) = world.get::<Position>(p) else {
                removed += 1; // The destruction model removed it: counted, not moved.
                continue;
            };
            let d = (now.0 - *at).with_y(0.0);
            let travel = d.length();
            if travel <= HEAP_MOVED_M {
                continue;
            }
            moved += 1;
            travel_sum += travel;
            travel_max = travel_max.max(travel);
            // Within 45° of the heading is forward, within 45° of its reverse back.
            let along = d.dot(heading) / travel;
            if along >= std::f32::consts::FRAC_1_SQRT_2 {
                forward += 1;
            } else if along <= -std::f32::consts::FRAC_1_SQRT_2 {
                back += 1;
            } else {
                sideways += 1;
            }
        }
        let end = bed.hull().position;
        rows.push(vec![
            ("case", json!(format!("{throttle} throttle"))),
            ("heap_asleep_after_s", f(asleep_after)),
            ("pieces_awake_at_drive", json!(awake_at_drive)),
            ("hull_rise_max_m", f(rise_max)),
            ("hull_rise_over_heap_centre_m", f(rise_centre)),
            ("pitch_max_nose_up_deg", json!(deg(nose_up))),
            ("pitch_max_nose_down_deg", json!(deg(nose_down))),
            ("reached_heap_centre_s", f(reached)),
            ("drive_s", f(driven as f32 * DT)),
            ("hull_past_far_edge_m", f(far - end.z)),
            ("heap_top_before_m", f(top_before)),
            ("heap_top_after_m", f(top_after)),
            ("pieces_moved", json!(moved)),
            ("moved_mean_m", f(travel_sum / moved as f32)),
            ("moved_max_m", f(travel_max)),
            ("moved_forward", json!(forward)),
            ("moved_back", json!(back)),
            ("moved_sideways", json!(sideways)),
            ("pieces_removed", json!(removed)),
        ]);
    }
    card.push(Line {
        id: "heap",
        title: "8c. Loose heap",
        pass: None,
        rows,
        note: format!(
            "The track sandbox's rubble yard pile, built by its own builder from \
             `assets/dev/track_sandbox.ron` ({piece_count} pieces of its source construction's \
             chunks, its drop grid, taper and yaw step), centred {:.1} m ahead of the hull \
             origin as the sandbox's `R` tour parks it, settled with the tank parked until every \
             piece sleeps ({HEAP_SETTLE_S} s at most; asleep-after is null when the bound ran \
             out). Then drive straight over the heap's centre until the belt's rear point clears \
             the heap's far edge (15 s at most). Rise is against the hull's rest level on the \
             ground; pitch is nose up and nose down. The heap's top and far edge are read in a \
             strip one track gauge wide about the drive line, by rays cast down on a \
             {HEAP_RAY_GRID_M} m grid; past-far-edge is the hull centre's distance beyond it at \
             the end, negative short of it. Piece motion is horizontal, from each piece's settled \
             pose to the drive's end; moved is past {HEAP_MOVED_M} m, split by direction against \
             the tank's starting heading (within 45° forward or back, else sideways). No reaction ratio: it reads static balance \
             (the world's support on the pieces less their weight is the load), which holds only \
             for pieces at rest; on a drive the heap's pieces gain and shed momentum, so a ratio \
             off 1 breaches no law, and a sleeping piece holds no solver contact to read. Line \
             8's parked rows carry it.",
            rubble::spot_ahead_m()
        ),
    });
}
