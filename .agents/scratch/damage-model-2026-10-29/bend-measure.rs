//! THROWAWAY MEASUREMENT (never merges into the build; its source is kept beside its results,
//! `bend-measured.txt`): how far the shipped main-gun rounds' path turns crossing the shipped fir_1
//! trunk in the game's damage model — the horizontal angle between the round's direction at entry
//! and its direction once past the trunk, and the full 3D angle. One shot per cell at a fresh tree,
//! from 50 m, aimed to meet the trunk at height `h`, offset sideways by a fraction of the local
//! radius. Prints a table; asserts nothing.
//!
//! Run: copy to `src/ballistics/tree_bend_measure.rs`, declare `#[cfg(test)] mod
//! tree_bend_measure;` in `src/ballistics.rs`, then `scripts/serial-cargo test --lib --features
//! dev_ui,bitprobe tree_bend_measure -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Duration;

use avian3d::prelude::{CollisionLayers, LayerMask, PhysicsPlugins, RigidBody};
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::spec::FireMechanism;
use crate::trees::TreeKind;

/// The game's fixed step, so each update runs exactly one fixed tick.
const TICK: Duration = Duration::from_micros(15_625);
/// Muzzle to trunk axis, metres.
const RANGE_M: f32 = 50.0;

fn world() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin {
            level: bevy::log::Level::ERROR,
            ..Default::default()
        },
        bevy::transform::TransformPlugin,
        AssetPlugin::default(),
        PhysicsPlugins::default(),
    ))
    .init_asset::<Mesh>()
    .insert_resource(TimeUpdateStrategy::ManualDuration(TICK))
    .add_plugins(sim_plugin);
    app.insert_resource(MgShortCircuit(false))
        .insert_resource(RetainSpentShells(true))
        .add_observer(crate::trees::fell_on_pool_emptied);
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    app.world_mut().spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::Static,
        Collider::cuboid(800.0, 1.0, 800.0),
        CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
    ));
    app
}

fn spawn_tree(app: &mut App, kind: &Arc<TreeKind>, ordinal: u16) -> Entity {
    let world = app.world_mut();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, world);
    let tree = crate::trees::spawn(
        &mut commands,
        kind,
        crate::scatter::MapInstance {
            id: format!("bend_{ordinal}"),
            ordinal,
        },
        Transform::from_translation(Vec3::ZERO),
    );
    queue.apply(world);
    for _ in 0..8 {
        app.update();
    }
    tree
}

#[derive(Default)]
struct ShotRead {
    events: String,
    incidence_deg: Option<f32>,
    overmatched: Option<bool>,
    incoming: Option<Vec3>,
    outgoing: Option<Vec3>,
}

fn fire_once(app: &mut App, round: &crate::rounds::Round, tree: Entity, from: Vec3) -> ShotRead {
    let shooter = app.world_mut().spawn_empty().id();
    let before: Vec<Entity> = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<ShotDose>>()
            .iter(world)
            .collect()
    };
    app.world_mut().trigger(FireShell::of_round(
        round,
        from,
        Dir3::NEG_Z,
        FireMechanism::Single,
        false,
        Some(ShotSource {
            tank: shooter,
            weapon: 0,
            round: 0,
        }),
        None,
    ));
    app.world_mut().flush();
    let shell = {
        let world = app.world_mut();
        world
            .query_filtered::<Entity, With<ShotDose>>()
            .iter(world)
            .find(|entity| !before.contains(entity))
            .expect("the shell spawned")
    };
    let mut read = ShotRead::default();
    for _ in 0..30 {
        app.update();
        let world = app.world();
        let Some(projectile) = world.get::<Projectile>(shell) else {
            break;
        };
        if world.get::<Transform>(shell).unwrap().translation.z < -3.0 {
            read.outgoing = Some(projectile.velocity.normalize());
            break;
        }
    }
    let world = app.world();
    if let Some(marks) = world.get::<PenetrationMarks>(shell) {
        let mut events = Vec::new();
        for mark in &marks.log {
            match mark {
                ArmourMark::Crossing {
                    normal,
                    incoming,
                    overmatched,
                    volumes,
                    end,
                    ..
                } if volumes.iter().any(|crossed| crossed.volume == tree) => {
                    read.incidence_deg = Some(incoming.angle_between(-*normal).to_degrees());
                    read.overmatched = Some(*overmatched);
                    read.incoming = Some(*incoming);
                    events.push(match end {
                        CrossingEnd::Exit { .. } => "through",
                        CrossingEnd::Rest { .. } => "rest",
                    });
                }
                ArmourMark::Bounce { struck, .. } if *struck == tree => events.push("bounce"),
                ArmourMark::Burst { .. } => events.push("burst"),
                ArmourMark::Failed => events.push("failed"),
                _ => {}
            }
        }
        read.events = if events.is_empty() {
            "miss".to_owned()
        } else {
            events.join("+")
        };
    }
    app.world_mut().despawn(shell);
    app.world_mut().despawn(shooter);
    read
}

#[test]
#[ignore = "throwaway measurement; never merges"]
fn tree_bend_measure() {
    let root = crate::map::tests::shipped_assets();
    let library = crate::trees::TreeLibrary::load(&root, ["fir_1"]);
    let kind = library.get("fir_1").expect("fir_1").clone();
    eprintln!(
        "BEND|kind|height {:.3}|dbh {:.4}|substance {}",
        kind.height_m(),
        kind.dbh_m,
        kind.substance,
    );
    eprintln!("BEND|head|round|h|off_frac|events|incid_deg|overm|bend_yaw_deg|bend_3d_deg");
    let guns = crate::guns::GunRegistry::shipped();
    let mut app = world();
    let mut ordinal = 0u16;
    for key in ["88mm_AP", "88mm_APHE_ring", "88mm_APCR"] {
        let round = guns.round("kwk36_l56", key).expect("a shipped round");
        for h in [1.0_f32, 5.0, 10.0, 15.0, 20.0] {
            for frac in [0.0_f32, 0.5, 0.85] {
                let tree = spawn_tree(&mut app, &kind, ordinal);
                ordinal += 1;
                let t = RANGE_M / round.muzzle_speed;
                let from = Vec3::new(frac * kind.radius_at(h), h + 0.5 * 9.81 * t * t, RANGE_M);
                let read = fire_once(&mut app, &round, tree, from);
                let (yaw, full) = match (read.incoming, read.outgoing) {
                    (Some(a), Some(b)) => {
                        let yaw = |v: Vec3| v.x.atan2(-v.z);
                        (
                            format!("{:.3}", (yaw(b) - yaw(a)).to_degrees()),
                            format!("{:.3}", a.angle_between(b).to_degrees()),
                        )
                    }
                    _ => ("-".to_owned(), "-".to_owned()),
                };
                eprintln!(
                    "BEND|row|{key}|{h:.1}|{frac:.2}|{}|{}|{}|{yaw}|{full}",
                    read.events,
                    read.incidence_deg
                        .map_or("-".to_owned(), |v| format!("{v:.2}")),
                    read.overmatched.map_or("-".to_owned(), |v| v.to_string()),
                );
                app.world_mut().despawn(tree);
                for _ in 0..2 {
                    app.update();
                }
            }
        }
    }
}
