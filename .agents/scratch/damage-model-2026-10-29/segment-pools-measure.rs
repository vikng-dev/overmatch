//! THROWAWAY MEASUREMENT (never merges; its source is kept beside its results in
//! `.agents/scratch/damage-model-2026-10-29/`): what shipped and fixture rounds do to the shipped
//! fir_1's trunk segments in the game's damage model under per-segment pools (issue 149) — the HP
//! the first shot takes from the struck segment, the energy the round lost there, and how many
//! identical shots break the tree, and at which segment. The fir is measured at scale 1 (a mature
//! fir), 0.5 (a young tree, about 0.2 m at the base) and 0.25 (a sapling, about 0.1 m). Prints a
//! table; asserts nothing about balance.
//!
//! Run: copy to `src/ballistics/tree_shot_measure.rs`, declare `#[cfg(test)] mod
//! tree_shot_measure;` in `src/ballistics.rs`, and run `scripts/serial-cargo test --lib --features
//! dev_ui,bitprobe tree_shot_measure -- --ignored --nocapture`.

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
/// Most identical shots fired at one cell before it is called unbroken.
const MAX_SHOTS: usize = 2000;
/// Consecutive shots that remove nothing before a cell is called unbroken.
const BARREN_SHOTS: usize = 5;

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

fn spawn_tree(app: &mut App, kind: &Arc<TreeKind>, scale: f32, ordinal: u16) -> Entity {
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
        Transform::from_translation(Vec3::ZERO).with_scale(Vec3::splat(scale)),
    );
    queue.apply(world);
    for _ in 0..8 {
        app.update();
    }
    tree
}

fn segments_of(app: &App, tree: Entity) -> Vec<Entity> {
    app.world()
        .get::<Children>(tree)
        .expect("segments")
        .iter()
        .collect()
}

#[derive(Clone, Debug, Default)]
struct ShotRead {
    /// Pool units this shot took, summed over the tree's segments.
    lost: u32,
    /// The segments the shot's dose reached, by index.
    struck: Vec<usize>,
    events: String,
    chord: Option<f32>,
    energy: Option<f32>,
    speed_in: Option<f32>,
    speed_out: Option<f32>,
}

fn fire_once(
    app: &mut App,
    round: &crate::rounds::Round,
    segments: &[Entity],
    from: Vec3,
) -> ShotRead {
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
    if let Some(dose) = world.get::<ShotDose>(shell) {
        for tally in &dose.tallies {
            if let Some(k) = segments.iter().position(|&s| s == tally.address) {
                let lost: u32 = tally.settles.iter().map(|settle| settle.lost).sum();
                read.lost += lost;
                if lost > 0 {
                    read.struck.push(k);
                }
            }
        }
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
                } if volumes.iter().any(|c| segments.contains(&c.volume)) => {
                    let ours = volumes.iter().filter(|c| segments.contains(&c.volume));
                    read.chord = Some(ours.clone().map(|c| c.chord).sum());
                    read.energy = Some(ours.map(|c| c.energy).sum());
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
                ArmourMark::Bounce { struck, .. } if segments.contains(struck) => {
                    events.push("bounce")
                }
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

/// The rounds measured: the main gun's solid and sub-calibre rounds and the machine gun's, each
/// fired from its gun at its muzzle speed, and two fixture 75 mm APCBC bodies (6.8 kg, 75 mm, the
/// APCBC type's nose) at 790 and 925 m/s, built off the solid 88's record.
fn rounds() -> Vec<(String, crate::rounds::Round)> {
    let guns = crate::guns::GunRegistry::shipped();
    let mut out: Vec<(String, crate::rounds::Round)> = [
        ("kwk36_l56", "88mm_AP"),
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
    // The trees: (label, instance scale, the heights shot, metres above the base).
    let base_mid = 0.5 * kind.segments[0].height_m;
    let trees: [(&str, f32, Vec<f32>); 3] = [
        ("mature", 1.0, vec![1.0, 10.0, 20.0]),
        ("young", 0.5, vec![base_mid * 0.5]),
        ("sapling", 0.25, vec![base_mid * 0.25]),
    ];
    for (label, scale, _) in &trees {
        let body = TreeBody::new(kind.clone(), *scale);
        let pools: Vec<String> = (0..kind.segments.len())
            .map(|k| {
                let d = (kind.segments[k].r_bottom_m + kind.r_top_m(k)) * scale;
                format!("{k}:{d:.4}m={}", body.segment_hp(k))
            })
            .collect();
        eprintln!(
            "MEASURE|tree|{label}|scale {scale}|height {:.3}|dbh {:.4}|hp_per_m2 {}|joules_per_hp {}|pools {}",
            kind.height_m() * scale,
            kind.dbh_m * scale,
            kind.hp_per_m2,
            super::energy_dose::joules_per_hp(),
            pools.join(" "),
        );
    }
    eprintln!(
        "MEASURE|head|tree|round|h|off_frac|segment|pool|events|chord_m|energy_J|v_in|v_out|hp_shot1|shots|state"
    );
    let mut app = world();
    let mut ordinal = 0u16;
    for (key, round) in rounds() {
        for (label, scale, heights) in &trees {
            for &h in heights {
                for frac in [0.0_f32, 0.5] {
                    let tree = spawn_tree(&mut app, &kind, *scale, ordinal);
                    ordinal += 1;
                    let segments = segments_of(&app, tree);
                    let body = app.world().get::<TreeBody>(tree).unwrap().clone();
                    let segment = body.prefix.iter().filter(|&&b| h >= b).count() - 1;
                    let t = RANGE_M / round.muzzle_speed;
                    let r = body.radius_at(h);
                    let from = Vec3::new(frac * r, h + 0.5 * 9.81 * t * t, RANGE_M);
                    let mut first: Option<ShotRead> = None;
                    let (mut shots, mut barren) = (0usize, 0usize);
                    while shots < MAX_SHOTS {
                        let read = fire_once(&mut app, &round, &segments, from);
                        shots += 1;
                        barren = if read.lost == 0 { barren + 1 } else { 0 };
                        if first.is_none() {
                            first = Some(read);
                        }
                        let down = *app.world().get::<TreeState>(tree).unwrap()
                            != TreeState::Standing;
                        if down || barren >= BARREN_SHOTS {
                            break;
                        }
                    }
                    let first = first.unwrap();
                    let fmt =
                        |v: Option<f32>, d: usize| v.map_or("-".to_owned(), |v| format!("{v:.d$}"));
                    eprintln!(
                        "MEASURE|row|{label}|{key}|{h:.3}|{frac:.2}|{segment}|{}|{}{:?}|{}|{}|{}|{}|{:.3}|{shots}|{:?}",
                        body.segment_hp(segment),
                        first.events,
                        first.struck,
                        fmt(first.chord, 4),
                        fmt(first.energy, 0),
                        fmt(first.speed_in, 1),
                        fmt(first.speed_out, 1),
                        f64::from(first.lost) / 1000.0,
                        app.world().get::<TreeState>(tree).unwrap(),
                    );
                    app.world_mut().despawn(tree);
                    for _ in 0..2 {
                        app.update();
                    }
                }
            }
        }
    }
}
