//! Headless measurement beds over the scripted harness, in the game's tank driving physics (game
//! state only). Each runs one `SANDBOX_HARNESS` scenario on the sandbox headless and prints what a
//! reader compares between two builds; none is a gate (`-- --ignored friction_bed --nocapture`).
//!
//! Per tick it reads the hull's pose bits, the belt's total load, and every touching avian contact
//! of a hull collider (an authored proxy, a bump stop, or a side plate) carrying a normal impulse:
//! what it touches (terrain, a standing chunk, a loose piece), the hull-local point, and the pair's
//! friction. Authored proxies are named by their nodes.

use avian3d::prelude::{
    Collider, ColliderOf, ContactGraph, ContactPairFlags, LinearVelocity, Position, Rotation,
};
use bevy::prelude::*;

use super::{BeltContacts, Hull, mount_harness};
use crate::bake::TankBlueprint;
use crate::buildings::breakage::BuildingChunk;
use crate::debris::DebrisPiece;
use crate::headless_test::{BOOT_LEASE, finish_plugins, headless_shell, start_fixed_clock};
use crate::track::bump_stop::{BumpStop, SidePlate};

/// One hull collider's touch in one tick.
struct Touch {
    tick: usize,
    hull_part: String,
    other: &'static str,
    /// Hull-local contact point (m).
    local: Vec3,
    impulse: f32,
    friction: f32,
    belt_load: f32,
}

/// One scenario's run.
struct Run {
    /// Per tick: hull translation and rotation bits.
    bits: Vec<[u32; 7]>,
    positions: Vec<Vec3>,
    pitches: Vec<f32>,
    speeds: Vec<f32>,
    belt_loads: Vec<f32>,
    touches: Vec<Touch>,
    rest_y: f32,
}

fn run(spec: &str, ticks: usize) -> Run {
    let _lease = BOOT_LEASE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut app = headless_shell();
    app.add_plugins(super::plugin);
    mount_harness(
        &mut app,
        super::harness::parse_spec(spec).unwrap_or_else(|err| panic!("{spec}: {err}")),
    );
    finish_plugins(&mut app);
    start_fixed_clock(&mut app);
    let started = std::time::Instant::now();
    while !app
        .world()
        .contains_resource::<super::harness::HarnessLog>()
    {
        assert!(
            started.elapsed().as_secs() < 120,
            "the harness never set up"
        );
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let rest_y = app.world().resource::<super::RigGeom>().hull_rest_y;
    // The authored proxies are flattened onto the hull at their nodes' root poses.
    let proxies: Vec<(Vec3, String)> = {
        let blueprint = app.world().resource::<TankBlueprint>();
        blueprint
            .geometry
            .collision_proxies
            .iter()
            .map(|&i| {
                let node = &blueprint.geometry.nodes[i];
                (node.root_position, node.name.clone())
            })
            .collect()
    };
    let mut out = Run {
        bits: Vec::new(),
        positions: Vec::new(),
        pitches: Vec::new(),
        speeds: Vec::new(),
        belt_loads: Vec::new(),
        touches: Vec::new(),
        rest_y,
    };
    for tick in 0..ticks {
        app.update();
        let world = app.world_mut();
        let (hull, position, rotation, velocity) = world
            .query_filtered::<(Entity, &Position, &Rotation, &LinearVelocity), With<Hull>>()
            .single(world)
            .map(|(e, p, r, v)| (e, p.0, r.0, v.0))
            .expect("one hull");
        let t = position.to_array().map(f32::to_bits);
        let q = rotation.to_array().map(f32::to_bits);
        out.bits.push([t[0], t[1], t[2], q[0], q[1], q[2], q[3]]);
        out.positions.push(position);
        let forward = rotation * Vec3::NEG_Z;
        out.pitches.push(forward.y.asin().to_degrees());
        out.speeds
            .push(Vec3::new(velocity.x, 0.0, velocity.z).length());
        let belt_load: f32 = world.resource::<BeltContacts>().all().map(|c| c.load).sum();
        out.belt_loads.push(belt_load);
        let to_local = Transform::from_translation(position)
            .with_rotation(rotation)
            .to_matrix()
            .inverse();
        let mut hull_parts =
            world.query_filtered::<(Entity, &Transform, Has<BumpStop>, Has<SidePlate>, &ChildOf), With<Collider>>();
        let parts: Vec<(Entity, String)> = hull_parts
            .iter(world)
            .filter(|(.., parent)| parent.parent() == hull)
            .map(|(e, transform, hard_stop, side_plate, _)| {
                let name = if hard_stop {
                    "bump stop".to_owned()
                } else if side_plate {
                    "side plate".to_owned()
                } else {
                    proxies
                        .iter()
                        .find(|(at, _)| *at == transform.translation)
                        .map_or_else(|| "unnamed".to_owned(), |(_, name)| name.clone())
                };
                (e, name)
            })
            .collect();
        let graph = world.resource::<ContactGraph>();
        let mut found = Vec::new();
        for pair in graph.iter_active() {
            if !pair.flags.contains(ContactPairFlags::TOUCHING) {
                continue;
            }
            let (part, other) =
                if let Some((_, name)) = parts.iter().find(|(e, _)| *e == pair.collider1) {
                    (name.clone(), pair.collider2)
                } else if let Some((_, name)) = parts.iter().find(|(e, _)| *e == pair.collider2) {
                    (name.clone(), pair.collider1)
                } else {
                    continue;
                };
            for manifold in &pair.manifolds {
                for point in &manifold.points {
                    if point.normal_impulse > 0.0 {
                        found.push((
                            part.clone(),
                            other,
                            point.point,
                            point.normal_impulse,
                            manifold.friction,
                        ));
                    }
                }
            }
        }
        for (hull_part, other, point, impulse, friction) in found {
            let body = world.get::<ColliderOf>(other).map_or(other, |of| of.body);
            let kind = if world.get::<DebrisPiece>(body).is_some() {
                "piece"
            } else if world.get::<BuildingChunk>(other).is_some() {
                "standing chunk"
            } else {
                "terrain"
            };
            out.touches.push(Touch {
                tick,
                hull_part,
                other: kind,
                local: to_local.transform_point3(point),
                impulse,
                friction,
                belt_load,
            });
        }
    }
    out
}

/// FNV-1a over every tick's pose bits: equal digests mean bit-identical trajectories.
fn digest(bits: &[[u32; 7]]) -> u64 {
    bits.iter()
        .flatten()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, &w| {
            w.to_le_bytes()
                .iter()
                .fold(h, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
        })
}

/// The run's digest covers every tick; the motion and the touches count from tick `from` on.
fn report(label: &str, run: &Run, from: usize) {
    let first = run.positions[from];
    let last = *run.positions.last().unwrap();
    let rise = run.positions[from..]
        .iter()
        .map(|p| p.y - run.rest_y)
        .fold(f32::NEG_INFINITY, f32::max);
    let pitch = run.pitches[from..]
        .iter()
        .fold(0.0_f32, |m, p| if p.abs() > m.abs() { *p } else { m });
    println!(
        "{label}: digest {:016x}; from tick {from} at {first:.3}: travelled {:.3} m (xz), hull \
         rise max {:.3} m, pitch extreme {:.2}°, final speed {:.3} m/s, end {last:.3}",
        digest(&run.bits),
        Vec2::new(last.x - first.x, last.z - first.z).length(),
        rise,
        pitch,
        run.speeds.last().unwrap(),
    );
    let touches: Vec<&Touch> = run.touches.iter().filter(|t| t.tick >= from).collect();
    let loaded = touches.iter().filter(|t| t.belt_load > 0.0).count();
    println!(
        "{label}: {} hull-collider contact points with a normal impulse ({loaded} while the belts \
         carried load), over {} ticks",
        touches.len(),
        touches
            .iter()
            .map(|t| t.tick)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
    );
    let mut groups: std::collections::BTreeMap<(String, &str), Vec<&Touch>> = Default::default();
    for touch in touches {
        groups
            .entry((touch.hull_part.clone(), touch.other))
            .or_default()
            .push(touch);
    }
    for ((part, other), touches) in groups {
        let xs = touches.iter().map(|t| t.local.x.abs());
        let (x_lo, x_hi) = xs.fold((f32::INFINITY, 0.0_f32), |(lo, hi), x| {
            (lo.min(x), hi.max(x))
        });
        let y_lo = touches
            .iter()
            .map(|t| t.local.y)
            .fold(f32::INFINITY, f32::min);
        let impulse: f32 = touches.iter().map(|t| t.impulse).sum();
        let loaded = touches.iter().filter(|t| t.belt_load > 0.0).count();
        println!(
            "  {part} on {other}: {} points ({loaded} under belt load), hull-local |x| {x_lo:.3}..{x_hi:.3} m, \
             lowest y {y_lo:.3} m, Σ normal impulse {impulse:.0} N·s, pair friction {:.3}, first tick {}",
            touches.len(),
            touches[0].friction,
            touches[0].tick,
        );
    }
}

/// THE FRICTION BED: the four drives the lane's before/after compares (flat runway, rubble pile,
/// nose-first into the standing wall, and the proxies' touches in each).
#[test]
#[ignore = "a measurement bed, run on demand"]
fn friction_bed() {
    const WARMUP: usize = 192;
    let flat = run(
        "pose=runway,warmup=192,ticks=640,throttle=1.0,out=/dev/null",
        WARMUP + 640,
    );
    report("flat runway, full throttle", &flat, WARMUP);
    let pile = run(
        "x=-13,z=1,warmup=192,ticks=640,throttle=0.3,t2=448,throttle2=0.0,out=/dev/null",
        WARMUP + 640,
    );
    report("rubble pile, 0.3 throttle 4 s then none", &pile, WARMUP);
    report(
        "rubble pile, at rest on it, the last 3 s",
        &pile,
        WARMUP + 448,
    );
    let wall = run(
        "x=-18,z=1,warmup=192,ticks=640,throttle=1.0,out=/dev/null",
        WARMUP + 640,
    );
    report("standing wall, full throttle nose-first", &wall, WARMUP);
}
