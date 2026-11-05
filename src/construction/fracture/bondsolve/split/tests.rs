//! The split's experiments (game state only): Avian bodies at the game's 64 Hz tick, the contact
//! reading mounted, gravity the game's. Fixture numbers are fixture data; every material number
//! is the shipped brick's. MEASURED readings print for the lane report.

use std::time::Duration;

use avian3d::prelude::{
    AngularVelocity, Collider, ColliderDensity, ComputedCenterOfMass, ComputedMass, ConstantForce,
    Gravity, GravityScale, LinearVelocity, PhysicsPlugins, Position, RigidBody, Rotation, Sleeping,
    SleepingDisabled,
};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::construction::fracture::bondsolve::{Geometry, Patch};
use crate::construction::fracture::{
    Block, Bond, Cause, Chunk, Joint, Material, Side, State, Structure, clip_polygon,
};
use crate::contact_transfer::{ContactTransfer, ContactTransfers, StructureBody};
use std::collections::HashMap;

fn brick() -> Material {
    Material::of(
        crate::substances::SubstanceRegistry::shipped_static(),
        "Brick",
    )
    .expect("the shipped registry reads")
    .expect("brick breaks")
}

fn gravity() -> f64 {
    f64::from(crate::physical_laws::gravity_mps2())
}

/// Boxes of `material` joined by bonded seams over the shared faces `joints` names: chunk `a`,
/// the far side, a point on the face and its unit normal.
fn fixture(
    material: Material,
    blocks: &[Block],
    joints: &[(usize, Side, Vec3, Vec3)],
) -> Structure {
    let chunks = blocks
        .iter()
        .map(|b| {
            let volume = (b.max - b.min).element_product();
            Chunk {
                volume,
                centroid: b.center(),
                mass: material.density * volume,
                material,
                buried: false,
            }
        })
        .collect();
    let mut patches = Vec::new();
    let joints = joints
        .iter()
        .map(|&(a, b, at, normal)| {
            let own = blocks[a].face_on(at, normal);
            let face = match b {
                Side::Chunk(b) => clip_polygon(&own, &blocks[b].face_on(at, normal), normal),
                Side::Ground => own,
            };
            let (mut area, mut first) = (0.0, Vec3::ZERO);
            for pair in face[1..].windows(2) {
                let (p, q) = (pair[0] - face[0], pair[1] - face[0]);
                let triangle = normal.dot(p.cross(q)).abs() / 2.0;
                area += triangle;
                first += (p + q) * (triangle / 3.0);
            }
            let joint = Joint {
                a,
                b,
                area,
                centroid: face[0] + first / area,
                normal,
                bond: Bond::Bonded(material.seam),
            };
            patches.push(
                Patch::face(&joint, &face).expect("a fixture face is a nondegenerate polygon"),
            );
            joint
        })
        .collect();
    Structure {
        chunks,
        joints,
        geometry: Geometry::Faces(patches),
    }
}

/// Avian at the game's fixed 64 Hz tick, one tick per update, under the game's gravity, with the
/// contact reading mounted.
fn world() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::transform::TransformPlugin,
        AssetPlugin::default(), // Avian's collider cache reads `AssetEvent<Mesh>`.
        PhysicsPlugins::default(),
        crate::contact_transfer::sim_plugin,
    ))
    .init_asset::<Mesh>()
    .insert_resource(Gravity(Vec3::NEG_Y * crate::physical_laws::gravity_mps2()))
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        15_625,
    )));
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    for _ in 0..3 {
        app.update(); // The first updates start the clocks.
    }
    app
}

/// A static or dynamic box body of brick, `size` full lengths, centred at `at`, turned `turn`.
fn brick_box(app: &mut App, kind: RigidBody, size: Vec3, at: Vec3, turn: Quat) -> Entity {
    let material = brick();
    app.world_mut()
        .spawn((
            Transform::from_translation(at).with_rotation(turn),
            kind,
            Collider::cuboid(size.x, size.y, size.z),
            ColliderDensity(material.density),
            crate::substances::contact_friction(material.friction),
        ))
        .id()
}

/// A dynamic 0.3 m cube of fixture density 1000 kg/m³ held off gravity and pressed along `push`
/// (N) by a constant force: a press Avian delivers through contact, as a parked body's is.
fn pusher(app: &mut App, at: Vec3, push: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            Transform::from_translation(at),
            RigidBody::Dynamic,
            Collider::cuboid(0.3, 0.3, 0.3),
            ColliderDensity(1000.0),
            crate::substances::contact_friction(brick().friction),
            GravityScale(0.0),
            ConstantForce(push),
            SleepingDisabled,
        ))
        .id()
}

/// Each structure point's force on the structure this step (N, world) at its point: the solver's
/// transfer over the step, and a sleeping pair's last transfer.
fn forces(app: &App) -> Vec<(ContactTransfer, Vec3)> {
    let reading = app.world().resource::<ContactTransfers>();
    let dt = reading.dt();
    reading
        .groups()
        .flatten()
        .map(|p| (*p, -p.impulse / dt))
        .chain(reading.resting().iter().map(|r| (r.transfer, r.press())))
        .collect()
}

#[derive(Component)]
struct ChunkCollider;

/// The authority's split loop over one construction: its bodies in Avian and their bonded
/// graphs, read once per tick ([`Authority::tick`]).
struct Authority {
    structure: Structure,
    blocks: Vec<Block>,
    state: State,
    /// Per joint, how its push-only part stands.
    contact: Vec<Contact>,
    bodies: Vec<(Entity, Body)>,
    colliders: HashMap<Entity, usize>,
}

/// What one tick did.
#[derive(Debug, Default)]
struct Tick {
    /// Bodies whose hold performed sparse work.
    solved: usize,
    /// Bodies whose hold found a mechanism, left unjudged.
    mechanisms: usize,
    products: usize,
    joint_visits: usize,
    changes: Vec<(usize, Change)>,
    /// Bodies and colliders spawned and despawned by splits.
    spawned: (usize, usize),
    despawned: (usize, usize),
}

impl Authority {
    /// Every group of `structure` at spawn as one body: static when anchored, dynamic otherwise.
    fn spawn(app: &mut App, structure: Structure, blocks: Vec<Block>) -> Self {
        let state = State::new(&structure);
        let mut authority = Self {
            contact: vec![Contact::Shut; structure.joints.len()],
            structure,
            blocks,
            state,
            bodies: Vec::new(),
            colliders: HashMap::new(),
        };
        for members in groups(&authority.structure, &authority.state, &authority.contact) {
            authority.body(
                app,
                members,
                (Vec3::ZERO, Quat::IDENTITY),
                (Vec3::ZERO, Vec3::ZERO),
            );
        }
        authority
    }

    /// A new body of `members`, its construction frame at `pose`, moving at `velocity` (linear at
    /// its centre of mass, angular).
    fn body(
        &mut self,
        app: &mut App,
        members: Vec<usize>,
        pose: (Vec3, Quat),
        velocity: (Vec3, Vec3),
    ) -> Entity {
        let body = Body::of(&self.structure, &self.state, &self.contact, members);
        let mut entity = app.world_mut().spawn((
            Transform::from_translation(pose.0).with_rotation(pose.1),
            StructureBody,
        ));
        if body.anchored {
            entity.insert(RigidBody::Static);
        } else {
            entity.insert((
                RigidBody::Dynamic,
                LinearVelocity(velocity.0),
                AngularVelocity(velocity.1),
            ));
        }
        let entity = entity.id();
        for &c in &body.members {
            let material = self.structure.chunks[c].material;
            let collider = app
                .world_mut()
                .spawn((
                    ChildOf(entity),
                    Transform::IDENTITY,
                    Collider::convex_hull(self.blocks[c].points.clone())
                        .expect("an admitted chunk hull is a convex collider"),
                    ColliderDensity(material.density),
                    crate::substances::contact_friction(material.friction),
                    ChunkCollider,
                ))
                .id();
            self.colliders.insert(collider, c);
        }
        self.bodies.push((entity, body));
        entity
    }

    fn body_of(&self, chunk: usize) -> Entity {
        self.bodies
            .iter()
            .find(|(_, b)| b.node(chunk).is_some())
            .expect("every standing chunk has a body")
            .0
    }

    /// One physics step, then every awake body held and judged once; a cut body splits.
    fn tick(&mut self, app: &mut App) -> Tick {
        app.update();
        let mut contacts: HashMap<Entity, Vec<(usize, Vec3, Vec3)>> = HashMap::new();
        for (point, force) in forces(app) {
            if let Some(&c) = self.colliders.get(&point.structure_collider) {
                contacts
                    .entry(point.structure)
                    .or_default()
                    .push((c, point.point, force));
            }
        }
        let world = app.world();
        let gravity = world.resource::<Gravity>().0;
        let mut tick = Tick::default();
        let mut cut = Vec::new();
        for (index, (entity, body)) in self.bodies.iter_mut().enumerate() {
            let e = world.entity(*entity);
            if e.contains::<Sleeping>() {
                continue; // At rest: Avian holds it, nothing new loads it.
            }
            let position = e.get::<Position>().expect("a body has a pose").0;
            let rotation = e.get::<Rotation>().expect("a body has a pose").0;
            let spin = e.get::<AngularVelocity>().map_or(Vec3::ZERO, |w| w.0);
            let inverse = rotation.inverse();
            let mut applied = vec![Wrench::default(); body.members.len()];
            for &(c, point, force) in contacts.get(entity).into_iter().flatten() {
                let i = body
                    .node(c)
                    .expect("a body's contact lies on its own collider");
                let local = (inverse * (point - position)).as_dvec3();
                applied[i].add(
                    Wrench::at((inverse * force).as_dvec3(), body.arm(i, local)),
                    1.0,
                );
            }
            let held = hold(
                body,
                &applied,
                (inverse * gravity).as_dvec3(),
                (inverse * spin).as_dvec3(),
            );
            assert!(
                held.resultants
                    .iter()
                    .all(|w| w.linear.is_finite() && w.angular.is_finite()),
                "a converged hold is finite"
            );
            tick.solved += usize::from(held.products > 0);
            tick.products += held.products;
            tick.joint_visits += held.joint_visits;
            if held.mechanism {
                tick.mechanisms += 1;
                continue; // No judge may read an unbalanced graph.
            }
            let found = judge(&self.structure, &self.state, &self.contact, body, &held);
            if !found.is_empty() {
                cut.push(index);
                tick.changes.extend(found);
            }
        }
        apply(&mut self.state, &mut self.contact, &tick.changes);
        let groups = groups(&self.structure, &self.state, &self.contact);
        for index in cut.into_iter().rev() {
            let (entity, old) = self.bodies.swap_remove(index);
            let parts: Vec<_> = groups
                .iter()
                .filter(|g| old.node(g[0]).is_some())
                .cloned()
                .collect();
            let e = app.world().entity(entity);
            let pose = (
                e.get::<Position>().expect("a body has a pose").0,
                e.get::<Rotation>().expect("a body has a pose").0,
            );
            let velocity = (
                e.get::<LinearVelocity>().map_or(Vec3::ZERO, |v| v.0),
                e.get::<AngularVelocity>().map_or(Vec3::ZERO, |w| w.0),
            );
            if parts.len() == 1 {
                // Still one rigid body: its graph changes, its Avian body does not.
                let body = Body::of(
                    &self.structure,
                    &self.state,
                    &self.contact,
                    parts[0].clone(),
                );
                self.bodies.push((entity, body));
                let last = self.bodies.len() - 1;
                self.bodies.swap(index, last);
                continue;
            }
            // ADR-0014: a split spawns new bodies; no sim state moves onto the old entity.
            app.world_mut().despawn(entity);
            self.colliders.retain(|_, c| old.node(*c).is_none());
            tick.despawned.0 += 1;
            tick.despawned.1 += old.members.len();
            let centre = old.centre().as_vec3();
            for part in parts {
                let size = part.len();
                let at = Body::of(&self.structure, &self.state, &self.contact, part.clone())
                    .centre()
                    .as_vec3();
                let linear = velocity.0 + velocity.1.cross(pose.1 * (at - centre));
                self.body(app, part, pose, (linear, velocity.1));
                tick.spawned.0 += 1;
                tick.spawned.1 += size;
            }
        }
        tick
    }
}

/// THE CONTACT RESULTANT IS STATICS (the game's environment destruction model): a board resting
/// on two knife-edge supports, each its own body, reads from Avian's contact transfer the two
/// reactions a rigid beam's statics gives, within 1 % of its weight, and no net moment about its
/// centre of mass. The split rests on it: every body's hold takes its faces' loads from this
/// reading. Awake (sleep disabled) each step is read anew; asleep the pair keeps its last step.
#[test]
fn a_board_on_two_supports_reads_its_statics() {
    // Fixture: a 2 m board, 0.1 m thick and 0.4 m deep, on 45°-turned square bars whose top edges
    // stand at x = -0.7 and x = 0.5.
    let (left, right) = (-0.7_f32, 0.5_f32);
    let edge = 0.1 * std::f32::consts::SQRT_2;
    for awake in [true, false] {
        let mut app = world();
        let supports = [left, right].map(|x| {
            brick_box(
                &mut app,
                RigidBody::Static,
                Vec3::new(0.2, 0.2, 0.6),
                Vec3::new(x, 0.0, 0.0),
                Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
            )
        });
        let board = brick_box(
            &mut app,
            RigidBody::Dynamic,
            Vec3::new(2.0, 0.1, 0.4),
            Vec3::new(0.0, edge + 0.05 + 1e-3, 0.0),
            Quat::IDENTITY,
        );
        app.world_mut().entity_mut(board).insert(StructureBody);
        if awake {
            app.world_mut().entity_mut(board).insert(SleepingDisabled);
        }
        let mut readings = Vec::new();
        let mut asleep_at = None;
        for tick in 0..400 {
            app.update();
            let world = app.world();
            if world.entity(board).contains::<Sleeping>() {
                asleep_at.get_or_insert(tick);
            }
            if tick < 160 {
                continue; // Settling.
            }
            let mass = world.get::<ComputedMass>(board).expect("a body").value();
            let centre = world.get::<Position>(board).expect("a body").0
                + world.get::<Rotation>(board).expect("a body").0
                    * world.get::<ComputedCenterOfMass>(board).expect("a body").0;
            let weight = mass * world.resource::<Gravity>().0.length();
            let mut reaction = [Vec3::ZERO; 2];
            let mut moment = Vec3::ZERO;
            for (point, force) in forces(&app) {
                assert_eq!(point.structure, board);
                let k = supports
                    .iter()
                    .position(|&s| s == point.body)
                    .expect("the board touches only its supports");
                reaction[k] += force;
                moment += (point.point - centre).cross(force);
            }
            let expected = [
                weight * (right - centre.x) / (right - left),
                weight * (centre.x - left) / (right - left),
            ];
            readings.push((
                (reaction[0].y - expected[0]) / weight,
                (reaction[1].y - expected[1]) / weight,
                moment.length() / (weight * (right - left)),
                (reaction[0].x + reaction[1].x)
                    .abs()
                    .max((reaction[0].z + reaction[1].z).abs())
                    / weight,
            ));
        }
        let worst = |f: fn(&(f32, f32, f32, f32)) -> f32| {
            readings.iter().map(f).fold(0.0_f32, |a, b| a.max(b.abs()))
        };
        let spread = |f: fn(&(f32, f32, f32, f32)) -> f32| {
            let values: Vec<_> = readings.iter().map(f).collect();
            values.iter().copied().fold(f32::MIN, f32::max)
                - values.iter().copied().fold(f32::MAX, f32::min)
        };
        println!(
            "MEASURED board awake={awake} asleep_at={asleep_at:?}: worst error of W: left {:.2e}, right {:.2e}, moment {:.2e}, side {:.2e}; tick-to-tick spread of W: left {:.2e}, right {:.2e}",
            worst(|r| r.0),
            worst(|r| r.1),
            worst(|r| r.2),
            worst(|r| r.3),
            spread(|r| r.0),
            spread(|r| r.1),
        );
        assert_eq!(
            asleep_at.is_some(),
            !awake,
            "the board sleeps unless sleep is disabled"
        );
        for (what, error) in [
            ("left reaction", worst(|r| r.0)),
            ("right reaction", worst(|r| r.1)),
            ("moment", worst(|r| r.2)),
        ] {
            assert!(
                error < 0.01,
                "awake={awake}: {what} off statics by {error} of W"
            );
        }
    }
}

/// A BEAM ON A WALL SLIDES AND TIPS AT ITS DATA (the game's environment destruction model): Avian
/// holds a brick beam on a brick wall under a side force below the pair friction the material
/// data gives, and slides it at `(F − μW)/m` above; it holds the beam while its centre of mass
/// stands inside the wall's edge, and tips it once outside. The split hands every rest and every
/// opened seam to Avian on these terms; the body solve never sees them.
#[test]
fn a_beam_on_a_wall_slides_or_tips_at_its_friction_and_lever() {
    let material = brick();
    let mu = crate::substances::pair_friction(material.friction, material.friction);
    // Fixture: a 2 m beam, 0.2 m square, on a 1 m wide wall whose top stands at y = 1.
    let beam_at = |x: f32| Vec3::new(x, 1.1 + 1e-3, 0.0);
    let setup = |x: f32, wall: f32| {
        let mut app = world();
        brick_box(
            &mut app,
            RigidBody::Static,
            Vec3::new(wall, 2.0, 1.0),
            Vec3::ZERO,
            Quat::IDENTITY,
        );
        let beam = brick_box(
            &mut app,
            RigidBody::Dynamic,
            Vec3::new(2.0, 0.2, 0.2),
            beam_at(x),
            Quat::IDENTITY,
        );
        app.world_mut().entity_mut(beam).insert(SleepingDisabled);
        for _ in 0..32 {
            app.update(); // Settle on the wall.
        }
        (app, beam)
    };
    for factor in [0.9_f32, 1.1] {
        let (mut app, beam) = setup(0.0, 1.0);
        let mass = app
            .world()
            .get::<ComputedMass>(beam)
            .expect("a body")
            .value();
        let weight = mass * crate::physical_laws::gravity_mps2();
        let start = app.world().get::<Position>(beam).expect("a body").0;
        app.world_mut()
            .entity_mut(beam)
            .insert(ConstantForce(Vec3::X * factor * mu * weight));
        let ticks = 32;
        for _ in 0..ticks {
            app.update();
        }
        let speed = app.world().get::<LinearVelocity>(beam).expect("a body").0.x;
        let moved = app.world().get::<Position>(beam).expect("a body").0.x - start.x;
        let expected =
            (factor - 1.0).max(0.0) * mu * crate::physical_laws::gravity_mps2() * ticks as f32
                / 64.0;
        println!(
            "MEASURED beam slide F = {factor} μW: speed {speed:.4} m/s against {expected:.4}, moved {moved:.4} m"
        );
        if factor < 1.0 {
            assert!(
                moved.abs() < 1e-3,
                "below friction the beam holds: moved {moved} m"
            );
        } else {
            assert!(
                (speed - expected).abs() < 0.02 * expected,
                "above friction it slides at (F − μW)/m: {speed} against {expected} m/s"
            );
        }
    }
    for overhang in [-0.02_f32, 0.02] {
        // The beam's centre of mass `overhang` past the wall's +x edge, at x = 0.5.
        let (mut app, beam) = setup(0.5 + overhang, 1.0);
        for _ in 0..96 {
            app.update();
        }
        let tilt = app
            .world()
            .get::<Rotation>(beam)
            .expect("a body")
            .0
            .to_axis_angle()
            .1;
        println!("MEASURED beam overhang {overhang} m: tilt {tilt:.4} rad after 2 s");
        if overhang < 0.0 {
            assert!(tilt < 1e-3, "inside the edge the beam stands: tilt {tilt}");
        } else {
            assert!(tilt > 0.2, "past the edge the beam tips: tilt {tilt}");
        }
    }
}

/// A stepped brick wall, anchored: two 0.5 m thick courses under two 0.25 m thick ones, each
/// 0.5 m tall and 1 m long, the thin courses centred (fixture geometry). Joint `k` is chunk
/// `k`'s base, at height `0.5 k`.
fn stepped_wall() -> (Structure, Vec<Block>) {
    let blocks: Vec<_> = [0.25_f32, 0.25, 0.125, 0.125]
        .iter()
        .enumerate()
        .map(|(k, &half)| {
            let y = 0.5 * k as f32;
            Block::cuboid(Vec3::new(-half, y, -0.5), Vec3::new(half, y + 0.5, 0.5))
        })
        .collect();
    let joints: Vec<_> = (0..4)
        .map(|k| {
            (
                k,
                if k == 0 {
                    Side::Ground
                } else {
                    Side::Chunk(k - 1)
                },
                Vec3::Y * (0.5 * k as f32),
                Vec3::Y,
            )
        })
        .collect();
    (fixture(brick(), &blocks, &joints), blocks)
}

/// The press (N) along +x at height `h` that brings joint `k` of [`stepped_wall`] to the brick
/// seam's tensile strength: Navier's `M/S − N/A = f_t` over the joint's rectangle (DERIVED beam
/// statics, independent of the solve).
fn navier_press(structure: &Structure, blocks: &[Block], k: usize, h: f32) -> f64 {
    let size = blocks[k].max - blocks[k].min;
    let (t, w) = (f64::from(size.x), f64::from(size.z));
    let above: f64 = structure.chunks[k..]
        .iter()
        .map(|c| f64::from(c.mass))
        .sum();
    let tensile = f64::from(structure.chunks[k].material.seam.tensile);
    (tensile + above * gravity() / (t * w)) * (w * t * t / 6.0) / f64::from(h - 0.5 * k as f32)
}

/// A BONDED WALL INSIDE ONE BODY CARRIES BEAM STATICS AND CRACKS AT ITS DATA (the game's
/// environment destruction model): an anchored stepped wall is one static body; a block pressed
/// on it through Avian loads it, and every joint's solved force and moment match the cantilever's
/// statics within 1 % (the stack is statically determinate, so stiffness cannot move them). With
/// the press just under the governing joint's Navier strength nothing cracks; just over it, that
/// joint and no other cracks within two ticks, opens, and the wall becomes two bodies: the base
/// still static, the top dynamic and carried on by Avian.
#[test]
fn a_bonded_wall_reads_beam_statics_and_cracks_where_its_data_says() {
    let (structure, blocks) = stepped_wall();
    let h = 1.85_f32; // Fixture press height, on the top course.
    let limits: Vec<_> = (0..4)
        .map(|k| navier_press(&structure, &blocks, k, h))
        .collect();
    let governing = (0..4)
        .min_by(|&a, &b| limits[a].total_cmp(&limits[b]))
        .expect("four joints");
    assert_eq!(governing, 2, "fixture: the step's thin joint governs");
    let critical = limits[governing];
    let mut app = world();
    let mut authority = Authority::spawn(&mut app, structure.clone(), blocks);
    assert_eq!(authority.bodies.len(), 1, "one bonded wall is one body");
    assert!(authority.bodies[0].1.anchored);
    let face = -0.125 - 0.15 - 1e-3;
    let press = |f: f64| Vec3::X * f as f32;
    // The block first seats under a light press, so no closing blow reaches the wall.
    let cube = pusher(&mut app, Vec3::new(face, h, 0.0), press(0.02 * critical));
    for _ in 0..16 {
        let tick = authority.tick(&mut app);
        assert!(tick.changes.is_empty(), "seating cracks nothing: {tick:?}");
    }
    app.world_mut()
        .entity_mut(cube)
        .insert(ConstantForce(press(0.5 * critical)));
    let mut held_work = Vec::new();
    for _ in 0..32 {
        let tick = authority.tick(&mut app);
        assert!(
            tick.changes.is_empty(),
            "half the critical press cracks nothing: {tick:?}"
        );
        held_work.push(tick.products);
    }
    // Beam statics at each joint, from the press the fixture applies and the chunks' weights.
    let force = 0.5 * critical;
    let mut body = authority.bodies[0].1.clone();
    let mut applied = vec![Wrench::default(); 4];
    for (point, f) in forces(&app) {
        let c = authority.colliders[&point.structure_collider];
        applied[c].add(
            Wrench::at(f.as_dvec3(), body.arm(c, point.point.as_dvec3())),
            1.0,
        );
    }
    let held = hold(&mut body, &applied, DVec3::NEG_Y * gravity(), DVec3::ZERO);
    for (k, carried) in held.resultants.iter().enumerate() {
        let above: f64 = structure.chunks[k..]
            .iter()
            .map(|c| f64::from(c.mass))
            .sum();
        let y = 0.5 * k as f64;
        let expected = Wrench {
            linear: DVec3::new(force, -above * gravity(), 0.0),
            angular: DVec3::new(0.0, 0.0, -force * (f64::from(h) - y)),
        };
        let scale = expected.linear.length().max(expected.angular.length());
        let error = (carried.linear - expected.linear)
            .length()
            .max((carried.angular - expected.angular).length())
            / scale;
        println!("MEASURED stepped wall joint {k}: error {error:.2e} of its largest statics term");
        assert!(
            error < 0.01,
            "joint {k}: {carried:?} against statics {expected:?}"
        );
    }
    println!(
        "MEASURED stepped wall hold work per tick while the press settles: {held_work:?} products"
    );
    app.world_mut()
        .entity_mut(cube)
        .insert(ConstantForce(press(0.98 * critical)));
    for _ in 0..32 {
        assert!(
            authority.tick(&mut app).changes.is_empty(),
            "under its strength the wall holds"
        );
    }
    app.world_mut()
        .entity_mut(cube)
        .insert(ConstantForce(press(1.02 * critical)));
    let mut split = None;
    for tick in 0..2 {
        let report = authority.tick(&mut app);
        if !report.changes.is_empty() {
            println!(
                "MEASURED stepped wall split on tick {tick} after the press passed: {report:?}"
            );
            split = Some(report);
            break;
        }
    }
    let split = split.expect("over its strength the governing joint cracks within two ticks");
    assert_eq!(split.changes, vec![(2, Change::Opened(Cause::Tension))]);
    assert_eq!(split.spawned, (2, 4));
    assert_eq!(split.despawned, (1, 4));
    let top = authority.body_of(3);
    assert_eq!(authority.body_of(2), top);
    assert!(
        !authority
            .bodies
            .iter()
            .find(|(e, _)| *e == top)
            .expect("a body")
            .1
            .anchored
    );
    let base = authority.body_of(0);
    assert!(
        authority
            .bodies
            .iter()
            .find(|(e, _)| *e == base)
            .expect("a body")
            .1
            .anchored
    );
    let start = app.world().get::<Position>(top).expect("a body").0;
    for _ in 0..64 {
        let tick = authority.tick(&mut app);
        assert!(
            tick.changes.is_empty(),
            "the remaining joints hold: {tick:?}"
        );
    }
    let world = app.world();
    let mass = world.get::<ComputedMass>(top).expect("a body").value();
    let top_mass = structure.chunks[2].mass + structure.chunks[3].mass;
    assert!(
        (mass - top_mass).abs() < 1e-3 * top_mass,
        "{mass} against {top_mass} kg"
    );
    let moved = world.get::<Position>(top).expect("a body").0 - start;
    let turned = world
        .get::<Rotation>(top)
        .expect("a body")
        .0
        .to_axis_angle()
        .1;
    println!("MEASURED stepped wall top after 1 s: moved {moved:?} m, turned {turned:.3} rad");
    assert!(moved.is_finite() && turned.is_finite());
    assert!(
        turned > 0.05,
        "Avian carries the cut-off top on: it tips under the press"
    );
}

/// A wall section over a cracked mortar bed (fixture: a 1 m cube of footing anchored to the
/// ground, a 1 m tall, 0.25 m thick, 1 m long section on it, the bed between them cracked).
fn cracked_bed() -> (Structure, Vec<Block>, State) {
    let blocks = vec![
        Block::cuboid(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 0.5, 0.5)),
        Block::cuboid(Vec3::new(-0.125, 0.5, -0.5), Vec3::new(0.125, 1.5, 0.5)),
    ];
    let structure = fixture(
        brick(),
        &blocks,
        &[
            (0, Side::Ground, Vec3::ZERO, Vec3::Y),
            (1, Side::Chunk(0), Vec3::Y * 0.5, Vec3::Y),
        ],
    );
    let mut state = State::new(&structure);
    state.cracked[1] = true;
    (structure, blocks, state)
}

/// A CRACKED BED SQUEEZED SHUT CARRIES PUSH ONLY (the game's environment destruction model): the
/// cracked bed under a wall section stays inside the footing's body while its resultant presses,
/// carrying the section's weight within 1 %; it is re-checked every tick at no solve cost while
/// nothing changes; pulled, it opens; pressed sideways, it holds while the pressure centre stays on
/// the bed (DERIVED: `F h < W t/2`) and opens past it, the section then tipping as its own body.
#[test]
fn a_cracked_bed_carries_push_only_and_opens_when_pulled() {
    let (structure, blocks, state) = cracked_bed();
    let open = vec![Contact::Shut; structure.joints.len()];
    assert_eq!(
        groups(&structure, &state, &open).len(),
        1,
        "a shut crack keeps one body"
    );
    let mut body = Body::of(&structure, &state, &open, vec![0, 1]);
    let weight = f64::from(structure.chunks[1].mass) * gravity();
    let none = vec![Wrench::default(); 2];
    let held = hold(&mut body, &none, DVec3::NEG_Y * gravity(), DVec3::ZERO);
    let bed = held.resultants[1];
    let error = (bed.linear - DVec3::NEG_Y * weight).length() / weight;
    println!("MEASURED cracked bed weight error {error:.2e}");
    assert!(
        error < 0.01,
        "the shut bed carries the section's weight: {bed:?}"
    );
    assert!(judge(&structure, &state, &open, &body, &held).is_empty());
    let again = hold(&mut body, &none, DVec3::NEG_Y * gravity(), DVec3::ZERO);
    assert_eq!(again.products, 0, "an unchanged re-check solves nothing");
    let mut pull = none.clone();
    pull[1].linear = DVec3::Y * 1.5 * weight;
    let held = hold(&mut body, &pull, DVec3::NEG_Y * gravity(), DVec3::ZERO);
    assert_eq!(
        judge(&structure, &state, &open, &body, &held),
        vec![(1, Change::Opened(Cause::LiftForce))]
    );

    let h = 1.3_f32; // Fixture press height.
    let lever = f64::from(h) - 0.5;
    let tip = weight * 0.125 / lever;
    let face = Vec3::new(-0.125 - 0.15 - 1e-3, h, 0.0);
    let spawn = |app: &mut App| {
        let mut authority = Authority::spawn(app, structure.clone(), blocks.clone());
        authority.state = state.clone();
        authority.bodies[0].1 = Body::of(&structure, &state, &open, vec![0, 1]);
        authority
    };
    let mut app = world();
    let mut authority = spawn(&mut app);
    let footing = authority.bodies[0].0;
    // The block seats under a light press first, so no closing blow reaches the section.
    let cube = pusher(&mut app, face, Vec3::X * (0.05 * tip) as f32);
    for factor in [0.05, 0.9] {
        app.world_mut()
            .entity_mut(cube)
            .insert(ConstantForce(Vec3::X * (factor * tip) as f32));
        let mut idle = 0;
        for _ in 0..32 {
            let report = authority.tick(&mut app);
            assert!(
                report.changes.is_empty(),
                "{factor}× the tipping press: {report:?}"
            );
            idle += usize::from(report.products == 0);
        }
        println!("MEASURED cracked bed held at {factor}× W t/2h, idle ticks {idle} of 32");
    }
    app.world_mut()
        .entity_mut(cube)
        .insert(ConstantForce(Vec3::X * (1.1 * tip) as f32));
    let report = (0..2)
        .map(|_| authority.tick(&mut app))
        .find(|report| !report.changes.is_empty())
        .expect("past the bed's edge the seam opens within two ticks");
    assert_eq!(report.changes, vec![(1, Change::Opened(Cause::Tip))]);
    let section = authority.body_of(1);
    assert_ne!(section, footing);
    for _ in 0..64 {
        authority.tick(&mut app);
    }
    let turned = |app: &App, body| {
        app.world()
            .get::<Rotation>(body)
            .expect("a body")
            .0
            .to_axis_angle()
            .1
    };
    println!(
        "MEASURED cracked bed section turned {:.3} rad in 1 s past the edge",
        turned(&app, section)
    );
    assert!(
        turned(&app, section) > 0.05,
        "the opened section tips as its own body"
    );

    // A block closing 1 mm under 0.9× the tipping press strikes at about 0.2 m/s (DERIVED):
    // its blow read as force over the tick opens the bed, and Avian, now owning the section,
    // rocks it and keeps it standing.
    let mut app = world();
    let mut authority = spawn(&mut app);
    pusher(&mut app, face, Vec3::X * (0.9 * tip) as f32);
    let mut opened = None;
    for tick in 0..128 {
        let report = authority.tick(&mut app);
        if !report.changes.is_empty() {
            opened.get_or_insert((tick, report.changes));
        }
    }
    let section = authority.body_of(1);
    println!(
        "MEASURED cracked bed struck under 0.9× W t/2h: opened {opened:?}, section turned {:.4} rad after 2 s, asleep {}",
        turned(&app, section),
        app.world().entity(section).contains::<Sleeping>()
    );
    assert!(
        turned(&app, section) < 0.05,
        "a sub-tipping press cannot topple the section"
    );
}

/// THE UNIT HOUSE AS BODIES (observations for the lane report, game state only): its bodies at
/// spawn, a gravity-only run, and a 300 kN press of a hull-mass block into its wall.
/// Invariants: every hold finds no mechanism and every pose stays finite. Whether the house
/// comes to rest is observed, not asserted.
#[test]
#[ignore = "house observation for the split experiment's report"]
fn the_unit_house_as_bodies() {
    let kind = crate::construction::tests::shipped_house();
    let structure = kind.layout.structure(&kind.asset);
    let state = State::new(&structure);
    let open = vec![Contact::Shut; structure.joints.len()];
    let groups = groups(&structure, &state, &open);
    let mut census: Vec<_> = groups
        .iter()
        .map(|g| {
            (
                g.len(),
                Body::of(&structure, &state, &open, g.clone()).anchored,
            )
        })
        .collect();
    census.sort_unstable();
    let rests = structure
        .joints
        .iter()
        .filter(|j| matches!(j.bond, Bond::Resting))
        .count();
    let ground = structure
        .joints
        .iter()
        .filter(|j| j.b == Side::Ground)
        .count();
    println!(
        "MEASURED house: {} chunks, {} joints ({rests} rests, {ground} ground), {} bodies at spawn (chunks, anchored): {census:?}",
        structure.chunks.len(),
        structure.joints.len(),
        groups.len()
    );
    let mut app = world();
    // Fixture ground: a static slab whose top is the construction's grade.
    app.world_mut().spawn((
        Transform::from_translation(Vec3::NEG_Y * 0.5),
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        crate::substances::contact_friction(brick().friction),
    ));
    let label = |j: usize| -> String {
        let joint = &structure.joints[j];
        let bond = match kind.asset.joints.get(j).map(|a| a.bond) {
            Some(crate::construction::schema::Bond::Seam) => "intrinsic".to_owned(),
            Some(crate::construction::schema::Bond::Attached(a)) => {
                kind.asset.attachments[usize::from(a)].clone()
            }
            None => "anchor".to_owned(),
        };
        let material = |c: usize| {
            let c = &kind.asset.chunks[kind.layout.chunks[c] as usize];
            kind.asset.materials[usize::from(c.material)].clone()
        };
        let other = match joint.b {
            Side::Chunk(b) => material(b),
            Side::Ground => "ground".to_owned(),
        };
        format!("{bond} {}-{other}", material(joint.a))
    };
    let tally = |changes: &[(usize, Change)]| {
        let mut by: std::collections::BTreeMap<String, usize> = Default::default();
        for &(j, change) in changes {
            let change = match change {
                Change::Contact(Contact::Slipping(..)) => "Slipping".to_owned(),
                change => format!("{change:?}"),
            };
            *by.entry(format!("{change} {}", label(j))).or_default() += 1;
        }
        by
    };
    let mut authority = Authority::spawn(&mut app, structure.clone(), kind.layout.blocks.clone());
    let mut cracks = 0;
    let mut all = Vec::new();
    let mut work = Vec::new();
    for tick in 0..640 {
        let report = authority.tick(&mut app);
        cracks += report.changes.len();
        if tick == 0 {
            println!(
                "MEASURED house gravity first tick changes: {:?}",
                tally(&report.changes)
            );
        }
        all.extend(report.changes.iter().copied());
        assert_eq!(
            report.mechanisms, 0,
            "every holding joint keeps an edge, so no part of a body moves freely inside it"
        );
        work.push((report.solved, report.products, report.joint_visits));
        let awake = authority
            .bodies
            .iter()
            .filter(|(e, b)| !b.anchored && !app.world().entity(*e).contains::<Sleeping>())
            .count();
        if tick % 32 == 0 || (awake == 0 && report.products == 0) {
            println!(
                "MEASURED house gravity tick {tick}: awake {awake}, bodies {}, solved {}, mechanisms {}, products {}, visits {}, cracks so far {cracks}",
                authority.bodies.len(),
                report.solved,
                report.mechanisms,
                report.products,
                report.joint_visits
            );
        }
        if awake == 0
            && tick > 64
            && work[work.len().saturating_sub(16)..]
                .iter()
                .all(|w| w.1 == 0)
        {
            break;
        }
    }
    let first = work.first().copied().unwrap_or_default();
    let last: Vec<_> = work.iter().rev().take(4).collect();
    println!(
        "MEASURED house gravity: changes {cracks} {:?}, first tick (solved, products, visits) {first:?}, last ticks {last:?}, ticks {}",
        tally(&all),
        work.len()
    );

    let (_, face, into) = crate::construction::breakage::tests::wall_face(&kind);
    let side = crate::construction::breakage::tests::CUBE_SIDE;
    let press = 300e3_f32; // Fixture: the settle lane's strong press, N.
    let block = app
        .world_mut()
        .spawn((
            Transform::from_translation(face - into * (side / 2.0 + 1e-3)),
            RigidBody::Dynamic,
            Collider::cuboid(side, side, side),
            ColliderDensity(46_800.0 / (side * side * side)), // Fixture: hull mass, kg.
            GravityScale(0.0),
            ConstantForce(into * press),
            SleepingDisabled,
        ))
        .id();
    let colliders_before = authority.colliders.len();
    let bodies_before = authority.bodies.len();
    let (mut spawned, mut despawned, mut changes) = ((0, 0), (0, 0), 0);
    let mut worst = (0, 0, 0);
    let mut pressed = Vec::new();
    for tick in 0..192 {
        let report = authority.tick(&mut app);
        assert_eq!(report.mechanisms, 0, "as under gravity");
        changes += report.changes.len();
        pressed.extend(report.changes.iter().copied());
        spawned.0 += report.spawned.0;
        spawned.1 += report.spawned.1;
        despawned.0 += report.despawned.0;
        despawned.1 += report.despawned.1;
        if report.products > worst.1 {
            worst = (tick, report.products, report.joint_visits);
        }
        if tick % 16 == 0 || !report.changes.is_empty() {
            println!(
                "MEASURED house press tick {tick}: bodies {}, solved {}, mechanisms {}, products {}, visits {}, changes {}, spawned {:?}, despawned {:?}",
                authority.bodies.len(),
                report.solved,
                report.mechanisms,
                report.products,
                report.joint_visits,
                report.changes.len(),
                report.spawned,
                report.despawned
            );
        }
        let world = app.world();
        for (entity, _) in &authority.bodies {
            let e = world.entity(*entity);
            assert!(e.get::<Position>().expect("a body").0.is_finite());
            assert!(e.get::<LinearVelocity>().is_none_or(|v| v.0.is_finite()));
        }
        assert!(
            world
                .get::<Position>(block)
                .expect("the block")
                .0
                .is_finite()
        );
    }
    println!(
        "MEASURED house press: bodies {bodies_before} -> {}, colliders {colliders_before} -> {}, joint changes {changes}, spawned (bodies, colliders) {spawned:?}, despawned {despawned:?}, worst tick (tick, products, visits) {worst:?}",
        authority.bodies.len(),
        authority.colliders.len()
    );
    println!("MEASURED house press changes: {:?}", tally(&pressed));
}
