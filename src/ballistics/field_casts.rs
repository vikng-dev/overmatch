//! The game's damage model: field walks over real fixture geometry at two rigid poses.
//!
//! Cone, lattice, grazing and box-directed rays must resolve to finite reaches and nonnegative
//! doses. An unobstructed inline box proves struck-tank filter admission independently of the
//! selected vehicle. Clients receive these resolved reaches (`damage_cam::record` and
//! `damage_cam::pen_cam::plan`); they do not repeat the casts.
//! Cross-platform direction generation remains guarded by `spall_cone`'s independent invariants.

use std::time::Duration;

use avian3d::prelude::{Position, Rotation};
use bevy::ecs::system::RunSystemOnce;
use bevy::time::TimeUpdateStrategy;

use super::fuzz::{PROBE_TANK_AT, ProbeTank, probe_world};
use super::spall_cone::{ConeShape, FieldDescriptor};
use super::*;

/// Fixture strength `S` (J): a lattice, graze or box ray carries it whole, a cone
/// direction `S·f_k`. Fixture data.
const STRENGTH: f32 = 30.0;

/// The `cone` family's field ratio `λ` (a 26.1° cone, DERIVED): fixture data.
const CONE_RATIO: f32 = 2.27;

/// The tank-local layout, read once at spawn pose.
struct Layout {
    lo: Vec3,
    hi: Vec3,
    /// Damageable boxes by name: (name, local AABB centre).
    boxes: Vec<(String, Vec3)>,
}

/// A rigid pose of the tank root.
#[derive(Clone, Copy)]
struct Pose {
    label: &'static str,
    translation: Vec3,
    rotation: Quat,
}

/// One world-space ray, the strength it is thrown with, and its share of its cone (1 outside the
/// `cone` family).
struct WorldRay {
    label: String,
    origin: Vec3,
    dir: Dir3,
    strength: f32,
    weight: f32,
}

fn axis(index: usize) -> Vec3 {
    Vec3::AXES[index % 3]
}

/// Fixture poses shared by the game's damage-model field invariants and their pricing bed.
fn fixture_poses() -> [Pose; 2] {
    [
        Pose {
            label: "A",
            translation: PROBE_TANK_AT,
            rotation: Quat::IDENTITY,
        },
        Pose {
            label: "B",
            translation: PROBE_TANK_AT + Vec3::new(-3.25, 0.5, 2.75),
            rotation: Quat::from_xyzw(0.02, 0.31, -0.015, 0.95).normalize(),
        },
    ]
}

/// Every ray of one pose. Tank-local points and directions go to world by the pose alone; cone
/// directions are generated in world from the world axis, as the sim generates them.
fn rays(layout: &Layout, pose: Pose) -> Vec<WorldRay> {
    let to_world = |p: Vec3| pose.translation + pose.rotation * p;
    let dir_world = |d: Vec3| Dir3::new(pose.rotation * d).expect("fixture direction is nonzero");
    let center = (layout.lo + layout.hi) * 0.5;
    let half = (layout.hi - layout.lo) * 0.5;
    let spread = ConeShape::shipped()
        .spread(FieldDescriptor::ball(CONE_RATIO))
        .expect("the fixture ratio lies in the ball's domain");
    let weight = 1.0 / spread.directions_count() as f32;
    let mut out = Vec::new();

    // cone: 3 axes × 2 sides × 2 lateral offsets, each a cone pointing inward.
    for a in 0..3 {
        let (b, d) = (axis(a + 1), axis(a + 2));
        for (si, s) in [-1.0f32, 1.0].into_iter().enumerate() {
            for (ki, k) in [-1.0f32, 1.0].into_iter().enumerate() {
                let origin =
                    center + axis(a) * (s * 0.8 * half[a]) + b * (k * 0.35 * half[(a + 1) % 3]);
                let axis_local = -s * axis(a) + 0.2 * k * b + 0.1 * d;
                for (n, (dir, profile)) in spread.directions(dir_world(axis_local)).enumerate() {
                    out.push(WorldRay {
                        label: format!("{}/cone/{a}{si}{ki}/{n:02}", pose.label),
                        origin: to_world(origin),
                        dir,
                        strength: STRENGTH * profile,
                        weight,
                    });
                }
            }
        }
    }

    // lattice: 5 × 5 per face, entering from 1 m outside the bounds.
    for a in 0..3 {
        let (b, d) = ((a + 1) % 3, (a + 2) % 3);
        for (si, s) in [-1.0f32, 1.0].into_iter().enumerate() {
            for i in 0..5 {
                for j in 0..5 {
                    let u = (i as f32 + 0.5) / 5.0 * 2.0 - 1.0;
                    let v = (j as f32 + 0.5) / 5.0 * 2.0 - 1.0;
                    let origin = center
                        + axis(a) * (s * (half[a] + 1.0))
                        + axis(b) * (u * 0.9 * half[b])
                        + axis(d) * (v * 0.9 * half[d]);
                    let dir = -s * axis(a) + 0.07 * axis(b) + 0.04 * axis(d);
                    out.push(WorldRay {
                        label: format!("{}/lattice/{a}{si}/{i}{j}", pose.label),
                        origin: to_world(origin),
                        dir: dir_world(dir),
                        strength: STRENGTH,
                        weight: 1.0,
                    });
                }
            }
        }
    }

    // graze: along each side (x and z faces) and the top, tilted 0.04 into the face.
    for (fi, (a, s)) in [(0usize, -1.0f32), (0, 1.0), (2, -1.0), (2, 1.0), (1, 1.0)]
        .into_iter()
        .enumerate()
    {
        let (b, d) = if a == 1 { (2, 0) } else { (2 - a, 1) };
        for (ri, f) in [0.97f32, 0.93, 0.88, 0.8].into_iter().enumerate() {
            for (wi, w) in [-0.5f32, 0.5].into_iter().enumerate() {
                let origin = center - axis(b) * (half[b] + 1.0)
                    + axis(a) * (s * f * half[a])
                    + axis(d) * (w * half[d]);
                let dir = axis(b) - s * 0.04 * axis(a);
                out.push(WorldRay {
                    label: format!("{}/graze/{fi}/{ri}{wi}", pose.label),
                    origin: to_world(origin),
                    dir: dir_world(dir),
                    strength: STRENGTH,
                    weight: 1.0,
                });
            }
        }
    }

    // box: at each damageable box's centre from two quadrants.
    let quadrants = [Vec3::new(1.0, 0.6, -0.8), Vec3::new(-0.9, 0.5, 0.7)];
    for (name, at) in &layout.boxes {
        for (qi, q) in quadrants.into_iter().enumerate() {
            let from = q.normalize();
            out.push(WorldRay {
                label: format!("{}/box/{name}/{qi}", pose.label),
                origin: to_world(*at + from * (2.0 * half.length() + 1.0)),
                dir: dir_world(-from),
                strength: STRENGTH,
                weight: 1.0,
            });
        }
    }
    out
}

/// The bounds the layout is built from: the armour set's parts and damageable boxes, at spawn
/// pose, as the march's broad query boxes them ([`ProjectileMarchWorld::everything`]).
fn layout(app: &mut App) -> Layout {
    let layout = app
        .world_mut()
        .run_system_once(
            |world: ProjectileMarchWorld, health: Query<&ComponentHealth>, names: Query<&Name>| {
                let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
                let mut boxes: Vec<(String, Vec3, Vec3)> = Vec::new();
                // The broad query's visit order is unspecified; sorting by name below makes the
                // layout independent of it, and min/max is order-free.
                world.everything(&mut |entity, aabb| {
                    let Some(volume) = world.volume_of(entity) else {
                        return; // Terrain and other bodies' colliders.
                    };
                    lo = lo.min(aabb.min - PROBE_TANK_AT);
                    hi = hi.max(aabb.max - PROBE_TANK_AT);
                    if health.contains(volume) {
                        let name = names
                            .get(volume)
                            .expect("the fixture's damageable volumes have diagnostic names")
                            .as_str()
                            .to_owned();
                        let (min, max) = (aabb.min - PROBE_TANK_AT, aabb.max - PROBE_TANK_AT);
                        match boxes.iter_mut().find(|(n, ..)| *n == name) {
                            Some((_, bmin, bmax)) => {
                                *bmin = bmin.min(min);
                                *bmax = bmax.max(max);
                            }
                            None => boxes.push((name, min, max)),
                        }
                    }
                });
                boxes.sort_by(|a, b| a.0.cmp(&b.0));
                Layout {
                    lo,
                    hi,
                    boxes: boxes
                        .into_iter()
                        .map(|(name, min, max)| (name, (min + max) * 0.5))
                        .collect(),
                }
            },
        )
        .expect("the layout system runs");
    assert!(
        layout.lo.cmple(layout.hi).all() && !layout.boxes.is_empty(),
        "the fixture tank has armour bounds and damageable boxes"
    );
    layout
}

/// The game's damage model: every ray must walk at the posed fixture geometry.
fn cast_pose(app: &mut App, layout: &Layout, pose: Pose) {
    let tank = app.world().resource::<ProbeTank>().0;
    app.world_mut().entity_mut(tank).insert((
        Position(pose.translation),
        Rotation(pose.rotation),
        Transform::from_translation(pose.translation).with_rotation(pose.rotation),
    ));
    // Fixture: let the collider hierarchy follow the moved root before casting.
    for _ in 0..4 {
        app.update();
    }
    cast_rays(app, tank, rays(layout, pose));
}

/// The game's damage model: finite reaches and nonnegative doses for every ray; whether any box
/// receives a positive dose is an observation unless the fixture independently guarantees access.
fn cast_rays(app: &mut App, tank: Entity, rays: Vec<WorldRay>) -> bool {
    app.world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld, health: Query<&ComponentHealth>| {
                let context = resolve::ResolveContext {
                    world: &world,
                    laws: walk::WalkLaws::default(),
                };
                let within = |collider: Entity| {
                    world.volume_of(collider).is_some_and(|node| {
                        world.owners.get(node).ok().map(VolumeOf::tank) == Some(tank)
                    })
                };
                let mut dosed = false;
                for ray in &rays {
                    let reach = resolve::FieldReach::of_throw(&context, &within, ray.origin);
                    let cast = resolve::field_direction(
                        &context,
                        &reach,
                        &|volume| health.contains(volume),
                        &[],
                        ray.origin,
                        ray.dir,
                        ray.strength,
                        ray.weight,
                    )
                    .unwrap_or_else(|error| panic!("{}: the walk failed: {error:?}", ray.label));
                    assert!(cast.end.is_finite(), "{}: finite field reach", ray.label);
                    for hit in cast.doses {
                        assert!(
                            hit.amount.is_finite() && hit.amount >= 0.0,
                            "{}: finite nonnegative field dose",
                            ray.label
                        );
                        dosed |= hit.amount > 0.0;
                    }
                }
                dosed
            },
        )
        .expect("the field-cast system runs")
}

/// The game's damage model: the field fixture's tank filter admits an unobstructed owned box and
/// excludes another owner's. This catches both empty and unrestricted filters without tank data.
/// FIXTURE DATA: a unit cube two metres along -Z, with low resistance and a positive field throw.
#[test]
fn field_cast_filter_admits_only_the_struck_tanks_reachable_box() {
    use super::march_tests::{box_trimesh, world_with_trimesh_plates};
    use avian3d::prelude::{CollisionLayers, LayerMask, RigidBody};

    let mut app = world_with_trimesh_plates(&[]);
    let owner = app.world_mut().spawn_empty().id();
    let other = app.world_mut().spawn_empty().id();
    app.world_mut().spawn((
        Transform::from_translation(Vec3::NEG_Z * 2.0),
        RigidBody::Static,
        box_trimesh(Vec3::ONE),
        CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
        BallisticVolume {
            material: walk::VolumeMaterial::fixture(1.0),
            substance: "fixture".to_owned(),
        },
        ComponentHealth::full(100),
        VolumeOf(owner),
    ));
    for _ in 0..8 {
        app.update();
    }
    for tank in [owner, other] {
        let dosed = cast_rays(
            &mut app,
            tank,
            vec![WorldRay {
                label: "unobstructed owned box".to_owned(),
                origin: Vec3::ZERO,
                dir: Dir3::NEG_Z,
                strength: STRENGTH,
                weight: 1.0,
            }],
        );
        assert_eq!(
            dosed,
            tank == owner,
            "only the struck tank's box is admitted"
        );
    }
}

/// The game's damage model must walk valid cast families independently of historical outputs.
#[test]
fn field_casts_walk_real_geometry_at_both_poses() {
    let mut app = probe_world().expect("the bound tank builds");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    let layout = layout(&mut app);
    for pose in fixture_poses() {
        cast_pose(&mut app, &layout, pose);
    }
}

/// Repeats of each priced cone; a cone's price is the median of its repeats.
const PRICED_REPEATS: usize = 25;

/// A PRICING BED, NOT A GATE (#202): the wall time of one spall cone thrown by the shipped sampler
/// on this tree, in the game's damage model (game state only), over this module's `cone` family
/// ([`rays`]) at both poses: the throw's reach ([`resolve::FieldReach::of_throw`]) once, then
/// [`resolve::field_direction`] along every direction, as the march's `walk_field` throws it. The
/// repeats run in rounds over every cone, so drift spreads over all of them; a cone's price is
/// its median. One thread; asserts nothing about time.
#[test]
#[ignore = "a pricing bed, run on demand"]
fn the_shipped_cone_sampler_priced_per_cone() {
    use std::time::Instant;
    let mut app = probe_world().expect("the bound tank builds");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )));
    let layout = layout(&mut app);
    let tank = app.world().resource::<ProbeTank>().0;
    let mut prices: Vec<(String, usize, f64)> = Vec::new();
    for pose in fixture_poses() {
        app.world_mut().entity_mut(tank).insert((
            Position(pose.translation),
            Rotation(pose.rotation),
            Transform::from_translation(pose.translation).with_rotation(pose.rotation),
        ));
        for _ in 0..4 {
            app.update();
        }
        let mut cones: Vec<(String, Vec<WorldRay>)> = Vec::new();
        for ray in rays(&layout, pose) {
            if !ray.label.contains("/cone/") {
                continue;
            }
            let key = ray.label[..ray.label.rfind('/').expect("a cone ray's label")].to_owned();
            match cones.last_mut() {
                Some((last, directions)) if *last == key => directions.push(ray),
                _ => cones.push((key, vec![ray])),
            }
        }
        let priced = app
            .world_mut()
            .run_system_once(
                move |world: ProjectileMarchWorld, health: Query<&ComponentHealth>| {
                    let context = resolve::ResolveContext {
                        world: &world,
                        laws: walk::WalkLaws::default(),
                    };
                    let within = |collider: Entity| {
                        world.volume_of(collider).is_some_and(|node| {
                            world.owners.get(node).ok().map(VolumeOf::tank) == Some(tank)
                        })
                    };
                    let is_box = |volume: Entity| health.contains(volume);
                    let mut micros = vec![Vec::with_capacity(PRICED_REPEATS); cones.len()];
                    for _ in 0..PRICED_REPEATS {
                        for ((_, directions), times) in cones.iter().zip(&mut micros) {
                            let origin = directions[0].origin;
                            let start = Instant::now();
                            let reach = resolve::FieldReach::of_throw(&context, &within, origin);
                            for ray in directions {
                                let cast = resolve::field_direction(
                                    &context,
                                    &reach,
                                    &is_box,
                                    &[],
                                    origin,
                                    ray.dir,
                                    ray.strength,
                                    ray.weight,
                                )
                                .unwrap_or_else(|error| {
                                    panic!("{}: the walk failed: {error:?}", ray.label)
                                });
                                std::hint::black_box(cast);
                            }
                            times.push(start.elapsed().as_secs_f64() * 1e6);
                        }
                    }
                    cones
                        .iter()
                        .zip(micros)
                        .map(|((key, directions), mut times)| {
                            times.sort_by(f64::total_cmp);
                            (key.clone(), directions.len(), times[times.len() / 2])
                        })
                        .collect::<Vec<_>>()
                },
            )
            .expect("the pricing system runs");
        prices.extend(priced);
    }
    for (key, directions, micros) in &prices {
        println!("   {key:<12} {directions:>3} directions {micros:>8.0} µs");
    }
    let mut column: Vec<f64> = prices.iter().map(|(.., micros)| *micros).collect();
    column.sort_by(f64::total_cmp);
    let at = |q: f64| column[((column.len() - 1) as f64 * q).round() as usize];
    println!(
        "MEASURED shipped cone sampler, {} cones, µs a cone: p50 {:.0} p95 {:.0} mean {:.0} max {:.0}",
        column.len(),
        at(0.5),
        at(0.95),
        column.iter().sum::<f64>() / column.len() as f64,
        column[column.len() - 1]
    );
}
