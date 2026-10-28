//! THROWAWAY MEASUREMENT (never merges; its source is kept beside its results in
//! `.agents/scratch/damage-model-2026-10-28/`): what shipped and fixture rounds do to the shipped
//! fir_1 trunk's HP pool in the game's damage model under the energy dose — the HP per shot, the
//! energy the round lost in the trunk, its readout cost, and how many identical shots empty the
//! pool. Prints a table; asserts nothing about balance.
//!
//! Run: `scripts/serial-cargo test --lib --features dev_ui,bitprobe tree_shot_measure --
//! --ignored --nocapture`.

use std::sync::Arc;
use std::time::Duration;

use avian3d::prelude::{CollisionLayers, LayerMask, PhysicsPlugins, RigidBody};
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::spec::FireMechanism;
use crate::trees::{TreeBody, TreeKind, TreeState};

/// The game's fixed step, so each update runs exactly one fixed tick.
const TICK: Duration = Duration::from_micros(15_625);
/// Muzzle to trunk axis, metres.
const RANGE_M: f32 = 50.0;
/// Most identical shots fired at one cell before it is called unemptied.
const MAX_SHOTS: usize = 2000;
/// Consecutive shots that remove nothing before a cell is called unemptied.
const BARREN_SHOTS: usize = 5;

#[derive(Resource, Default)]
struct Emptied(Vec<Entity>);

fn capture_emptied(emptied: On<PoolEmptied>, mut log: ResMut<Emptied>) {
    log.0.push(emptied.entity);
}

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
        .init_resource::<Emptied>()
        .add_observer(capture_emptied)
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
            id: format!("measure_{ordinal}"),
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

#[derive(Clone, Debug, Default)]
struct ShotRead {
    lost: u32,
    events: String,
    chord: Option<f32>,
    cost: Option<f32>,
    energy: Option<f32>,
    speed_in: Option<f32>,
    speed_out: Option<f32>,
    emptied: bool,
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
    let emptied_before = app.world().resource::<Emptied>().0.len();
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
    for _ in 0..30 {
        app.update();
        let world = app.world();
        if world.get::<Projectile>(shell).is_none() {
            break;
        }
        if world.get::<Transform>(shell).unwrap().translation.z < -3.0 {
            break;
        }
    }
    let world = app.world();
    let mut read = ShotRead::default();
    if let Some(dose) = world.get::<ShotDose>(shell)
        && let Some(tally) = dose.tallies.iter().find(|tally| tally.address == tree)
    {
        read.lost = tally.settles.iter().map(|settle| settle.lost).sum();
    }
    if let Some(marks) = world.get::<PenetrationMarks>(shell) {
        let mut events = Vec::new();
        for mark in &marks.log {
            match mark {
                ArmourMark::Crossing {
                    speed_in,
                    volumes,
                    end,
                    ..
                } if volumes.iter().any(|crossed| crossed.volume == tree) => {
                    let crossed = volumes.iter().find(|c| c.volume == tree).unwrap();
                    read.chord = Some(crossed.chord);
                    read.cost = Some(crossed.cost);
                    read.energy = Some(crossed.energy);
                    read.speed_in = Some(*speed_in);
                    match end {
                        CrossingEnd::Exit { speed, .. } => {
                            events.push("through");
                            read.speed_out = Some(*speed);
                        }
                        _ => {
                            events.push("rest");
                            read.speed_out = Some(0.0);
                        }
                    }
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
    read.emptied = app.world().resource::<Emptied>().0[emptied_before..].contains(&tree);
    app.world_mut().despawn(shell);
    app.world_mut().despawn(shooter);
    read
}

/// The rounds measured: the main gun's shipped rounds and the machine gun's, each fired from its
/// gun at its muzzle speed, and two fixture 75 mm APCBC bodies (6.8 kg, 75 mm, the APCBC type's
/// nose) at 790 and 925 m/s, built off the solid 88's record.
fn rounds() -> Vec<(String, crate::rounds::Round)> {
    let guns = crate::guns::GunRegistry::shipped();
    let mut out: Vec<(String, crate::rounds::Round)> = [
        ("kwk36_l56", "88mm_AP"),
        ("kwk36_l56", "88mm_APHE_ring"),
        ("kwk36_l56", "88mm_APCR"),
        ("mg34", "7.9mm_AP"),
    ]
    .into_iter()
    .map(|(gun, key)| (key.to_owned(), guns.round(gun, key).expect("a shipped round")))
    .collect();
    let solid = guns.round("kwk36_l56", "88mm_AP").unwrap();
    for speed in [790.0f32, 925.0] {
        let body = RoundBody {
            mass: 6.8,
            caliber: 0.075,
            nose_factor: solid.body.nose_factor,
        };
        out.push((
            format!("fixture_75mm_APCBC@{speed}"),
            crate::rounds::Round {
                body,
                penetrator: body,
                muzzle_speed: speed,
                burst: None,
                ..solid
            },
        ));
    }
    out
}

#[test]
#[ignore = "throwaway measurement; never merges"]
fn tree_shot_measure() {
    let root = crate::map::tests::shipped_assets();
    let library = crate::trees::TreeLibrary::load(&root, ["fir_1"]);
    let kind = library.get("fir_1").expect("fir_1").clone();
    let body = TreeBody::new(kind.clone(), 1.0);
    eprintln!(
        "MEASURE|kind|segments {}|height {:.3}|dbh {:.4}|hp {}|hp_per_m {}|substance {}|joules_per_hp {}",
        kind.segments.len(),
        kind.height_m(),
        kind.dbh_m,
        body.hp(),
        kind.hp_per_m,
        kind.substance,
        super::energy_dose::joules_per_hp(),
    );
    eprintln!(
        "MEASURE|head|round|h|off_frac|events|chord_m|cost_refmm|energy_J|v_in|v_out|hp_shot1|shots|emptied|state"
    );
    let mut app = world();
    let mut ordinal = 0u16;
    for (key, round) in rounds() {
        let mut cells: Vec<(f32, f32)> = Vec::new();
        for h in [1.0_f32, 10.0, 20.0] {
            for frac in [0.0_f32, 0.5] {
                cells.push((h, frac));
            }
        }
        for (h, frac) in cells {
            let tree = spawn_tree(&mut app, &kind, ordinal);
            ordinal += 1;
            let t = RANGE_M / round.muzzle_speed;
            let r = kind.radius_at(h);
            let from = Vec3::new(frac * r, h + 0.5 * 9.81 * t * t, RANGE_M);
            let mut first: Option<ShotRead> = None;
            let (mut shots, mut barren, mut emptied) = (0usize, 0usize, false);
            while shots < MAX_SHOTS {
                let read = fire_once(&mut app, &round, tree, from);
                shots += 1;
                barren = if read.lost == 0 { barren + 1 } else { 0 };
                emptied = read.emptied;
                if first.is_none() {
                    first = Some(read);
                }
                if emptied || barren >= BARREN_SHOTS {
                    break;
                }
            }
            let first = first.unwrap();
            let fmt = |v: Option<f32>, d: usize| v.map_or("-".to_owned(), |v| format!("{v:.d$}"));
            eprintln!(
                "MEASURE|row|{key}|{h:.1}|{frac:.2}|{}|{}|{}|{}|{}|{}|{}|{shots}|{emptied}|{:?}",
                first.events,
                fmt(first.chord, 4),
                fmt(first.cost, 2),
                fmt(first.energy, 0),
                fmt(first.speed_in, 1),
                fmt(first.speed_out, 1),
                first.lost,
                app.world().get::<TreeState>(tree).unwrap(),
            );
            app.world_mut().despawn(tree);
            for _ in 0..2 {
                app.update();
            }
        }
    }
}
