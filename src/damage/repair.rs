//! The game's damage model: authority-only module repair, with idle state present at spawn.

use bevy::prelude::*;
use serde::Deserialize;

use super::{
    Ammo, CrewStation, DamageConsequences, FunctionRole, TankKnockedOut, TankVolumes, VolumeFacets,
};
use crate::ballistics::ComponentHealth;
use crate::command::{ConsumeCommandEdges, ParsedTankCommand};
use crate::provenance::Sourced;
use crate::state::{GameplaySet, SimPhase};

/// Module-local repair work. `None` is idle; progress never supplies capability before completion.
#[derive(Component, Default, Debug)]
pub(crate) struct ModuleRepair {
    pub(crate) elapsed: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairData {
    hp_per_second: Sourced<f64>,
}

fn work_rate() -> f64 {
    static DATA: std::sync::LazyLock<RepairData> = std::sync::LazyLock::new(|| {
        let data: RepairData = ron::from_str(include_str!("../../assets/damage/repair.ron"))
            .expect("assets/damage/repair.ron: hp_per_second needs a value and provenance");
        assert!(
            data.hp_per_second.0.is_finite() && data.hp_per_second.0 > 0.0,
            "assets/damage/repair.ron: hp_per_second must be finite and positive; author a repair work rate"
        );
        data
    });
    DATA.hp_per_second.0
}

impl ModuleRepair {
    pub(crate) fn fraction(&self, health: &ComponentHealth) -> Option<f32> {
        self.elapsed
            .map(|elapsed| (elapsed / duration(health)) as f32)
    }

    pub(crate) fn set_fraction(&mut self, fraction: Option<f32>, health: &ComponentHealth) {
        self.elapsed = fraction.map(|fraction| f64::from(fraction) * duration(health));
    }
}

/// The game's damage model: whether any of this tank's modules is under repair. Repair is a tank
/// activity, not a quality a part supplies: a repairing crew holds the tank on its brakes
/// (`track::sim::apply_track_forces`), and every capability keeps its authored requirement.
pub(crate) fn underway(tank: &TankVolumes, volumes: &Query<VolumeFacets>) -> bool {
    tank.iter().any(|volume| {
        volumes
            .get(volume)
            .expect(super::LIVE_VOLUME)
            .repair
            .is_some_and(|repair| repair.elapsed.is_some())
    })
}

fn duration(health: &ComponentHealth) -> f64 {
    f64::from(health.points()) / work_rate()
}

pub(super) fn install(app: &mut App) {
    work_rate();
    app.add_systems(
        FixedUpdate,
        toggle_repairs
            .before(ConsumeCommandEdges)
            .before(SimPhase::DrivingForces)
            .in_set(GameplaySet)
            .run_if(not(resource_exists::<crate::ClientReplica>)),
    )
    .add_systems(
        FixedPostUpdate,
        advance_repairs
            .after(DamageConsequences)
            .before(SimPhase::Publish)
            .in_set(GameplaySet)
            .run_if(not(resource_exists::<crate::ClientReplica>)),
    );
}

/// Only function-bearing, non-ammunition, non-crew pools are repairable modules: the one
/// definition the repair systems and the status panel (`crew_ui`) share.
pub(crate) type Modules = (With<FunctionRole>, Without<Ammo>, Without<CrewStation>);

fn toggle_repairs(
    tanks: Query<(&TankVolumes, &ParsedTankCommand, Has<TankKnockedOut>)>,
    mut modules: Query<(&ComponentHealth, &mut ModuleRepair), Modules>,
) {
    for (volumes, command, knocked_out) in &tanks {
        if !command.repair || knocked_out {
            continue;
        }
        let cancelling = volumes.iter().any(|volume| {
            modules
                .get(volume)
                .is_ok_and(|(_, repair)| repair.elapsed.is_some())
        });
        for volume in volumes.iter() {
            // Armor, crew and ammunition are legitimate non-module volumes.
            if let Ok((health, mut repair)) = modules.get_mut(volume) {
                repair.elapsed = (!cancelling && health.is_empty()).then_some(0.0);
            }
        }
    }
}

fn advance_repairs(
    time: Res<Time>,
    tanks: Query<(&TankVolumes, Has<TankKnockedOut>)>,
    mut modules: Query<(&mut ComponentHealth, &mut ModuleRepair), Modules>,
) {
    for (volumes, knocked_out) in &tanks {
        for volume in volumes.iter() {
            // Only modules carry timers; the other ballistic roles remain unchanged.
            if let Ok((mut health, mut repair)) = modules.get_mut(volume) {
                if knocked_out {
                    repair.elapsed = None;
                } else if let Some(elapsed) = repair.elapsed.as_mut() {
                    *elapsed += time.delta_secs_f64();
                    if *elapsed >= duration(&health) {
                        health.refill();
                        repair.elapsed = None;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::command::TankCommand;
    use crate::damage::{
        Capability, Group, Part, TankCapabilities, VolumeFacets, VolumeOf, capability_available,
        requirement_met,
    };

    // ASSUMED fixture pools and fixed step: synthetic module data, independent of any tank asset.
    fn fixture() -> (World, Entity, [Entity; 2]) {
        let mut world = World::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f64(1.0 / 64.0));
        world.insert_resource(time);
        let tank = world
            .spawn((TankCommand::default(), ParsedTankCommand::default()))
            .id();
        let modules = [21, 40].map(|points| {
            let mut health = ComponentHealth::full(points);
            health.drain();
            world
                .spawn((VolumeOf(tank), FunctionRole::Engine, health))
                .id()
        });
        (world, tank, modules)
    }

    fn press(world: &mut World, tank: Entity, tick: u32, authored: u32) {
        *world.get_mut::<ParsedTankCommand>(tank).unwrap() =
            ParsedTankCommand::try_from(TankCommand {
                repair: true,
                for_tick: authored,
                ..default()
            })
            .unwrap()
            .attest(tick);
        world.run_system_once(toggle_repairs).unwrap();
    }

    fn advance(world: &mut World, ticks: usize) {
        for _ in 0..ticks {
            world.run_system_once(advance_repairs).unwrap();
        }
    }

    /// The game's damage model: capability consumers must see an empty pool until all authored
    /// HP/rate work is done; a partial refill or a rounded-down deadline breaks that contract.
    #[test]
    fn module_returns_whole_at_hp_over_rate_never_a_tick_before() {
        let (mut world, tank, modules) = fixture();
        assert!(
            world
                .get::<ModuleRepair>(modules[0])
                .unwrap()
                .elapsed
                .is_none()
        );
        press(&mut world, tank, 1, 1);
        // DERIVED fixture deadline: 21 HP / 20 HP/s = 1.05 s; the first complete tick is 68.
        advance(&mut world, 67);
        assert!(world.get::<ComponentHealth>(modules[0]).unwrap().is_empty());
        advance(&mut world, 1);
        let health = world.get::<ComponentHealth>(modules[0]).unwrap();
        assert_eq!(health.held(), health.capacity());
        assert!(world.get::<ComponentHealth>(modules[1]).unwrap().is_empty());
        advance(&mut world, 60);
        let health = world.get::<ComponentHealth>(modules[1]).unwrap();
        assert_eq!(health.held(), health.capacity());
    }

    /// The game's damage model: cancellation preserves finished modules for their consumers,
    /// but a restarted unfinished module owes its entire work, rather than banking progress.
    #[test]
    fn cancel_keeps_finished_modules_and_drops_unfinished_work() {
        let (mut world, tank, modules) = fixture();
        press(&mut world, tank, 1, 1);
        advance(&mut world, 96);
        press(&mut world, tank, 2, 2);
        assert!(!world.get::<ComponentHealth>(modules[0]).unwrap().is_empty());
        assert!(
            world
                .get::<ModuleRepair>(modules[1])
                .unwrap()
                .elapsed
                .is_none()
        );
        advance(&mut world, 128);
        assert!(world.get::<ComponentHealth>(modules[1]).unwrap().is_empty());
        press(&mut world, tank, 3, 3);
        advance(&mut world, 127);
        assert!(world.get::<ComponentHealth>(modules[1]).unwrap().is_empty());
        advance(&mut world, 1);
        assert!(!world.get::<ComponentHealth>(modules[1]).unwrap().is_empty());
    }

    /// The game's damage model: a running repair holds the tank on its brakes
    /// through the production drive-target selector while every capability keeps its
    /// authored requirement, so a fire gate still admits fire. A repair that closed a capability
    /// instead, or a hold that missed the running timer, fails here.
    #[test]
    fn repairing_holds_the_tank_but_keeps_every_capability_requirement() {
        use crate::track::drive::{DriveAxes, DriveTarget};
        use crate::track::sim::drive_target;

        fn gates(tanks: Query<(&TankVolumes, &TankCapabilities)>, facets: Query<VolumeFacets>) {
            let (volumes, caps) = tanks.single().unwrap();
            assert!(underway(volumes, &facets));
            assert_eq!(
                drive_target(&drive_command(), Some(volumes), Some(caps), &facets),
                DriveTarget::Brake,
                "the gameplay adapter must turn a running repair into service braking"
            );
            assert!(capability_available(
                Some(volumes),
                Some(caps),
                Capability::Drive,
                &facets
            ));
            assert!(requirement_met(
                Some(volumes),
                &vec![Group::Single(Part::Breech)],
                &facets
            ));
        }
        fn drive_command() -> ParsedTankCommand {
            ParsedTankCommand::try_from(TankCommand {
                throttle: 1.0,
                ..default()
            })
            .unwrap()
        }
        let (mut world, tank, _) = fixture();
        world
            .entity_mut(tank)
            .insert(TankCapabilities([(Capability::Drive, vec![])].into()));
        world.spawn((
            VolumeOf(tank),
            FunctionRole::Breech,
            ComponentHealth::full(10),
        ));
        press(&mut world, tank, 1, 1);
        world.run_system_once(gates).unwrap();
        advance(&mut world, 128);
        let (released, held) = world
            .run_system_once(
                |tanks: Query<(&TankVolumes, &TankCapabilities)>, facets: Query<VolumeFacets>| {
                    let (tank, caps) = tanks.single().unwrap();
                    (
                        drive_target(&drive_command(), Some(tank), Some(caps), &facets),
                        drive_target(&ParsedTankCommand::hold(), Some(tank), Some(caps), &facets),
                    )
                },
            )
            .unwrap();
        assert_eq!(
            released,
            DriveTarget::Axes(DriveAxes {
                throttle: 1.0,
                steer: 0.0
            }),
            "a finished repair releases the hold"
        );
        assert_eq!(
            held,
            DriveTarget::Brake,
            "a phase hold still selects service braking with no repair"
        );
    }

    /// The game's damage model: knocked-out lives cannot start or complete repairs, so terminal
    /// state cannot revive a module even if the knockout arrived during its timer.
    #[test]
    fn knocked_out_tank_cannot_start_or_finish_repair() {
        let (mut world, tank, modules) = fixture();
        world.entity_mut(tank).insert(TankKnockedOut {
            reason: super::super::KnockoutReason::CrewLoss,
        });
        press(&mut world, tank, 1, 1);
        assert!(
            world
                .get::<ModuleRepair>(modules[0])
                .unwrap()
                .elapsed
                .is_none()
        );
        world.entity_mut(tank).remove::<TankKnockedOut>();
        press(&mut world, tank, 2, 2);
        world.entity_mut(tank).insert(TankKnockedOut {
            reason: super::super::KnockoutReason::CrewLoss,
        });
        advance(&mut world, 128);
        assert!(world.get::<ComponentHealth>(modules[0]).unwrap().is_empty());
    }

    /// The game's damage model: only module functions return; ammunition, crew and functionless
    /// pools stay empty, even when the ammunition or crew pool also names a function, as a tank
    /// sheet may. Dropping either exclusion from [`Modules`] refills a pool here.
    #[test]
    fn ammunition_crew_and_functionless_pools_never_return() {
        let (mut world, tank, _) = fixture();
        let pools = [
            world
                .spawn((Ammo, FunctionRole::Engine, VolumeOf(tank)))
                .id(),
            world
                .spawn((CrewStation::Driver, FunctionRole::Engine, VolumeOf(tank)))
                .id(),
            world.spawn(VolumeOf(tank)).id(),
        ];
        for pool in pools {
            let mut health = ComponentHealth::full(10);
            health.drain();
            world.entity_mut(pool).insert(health);
        }
        press(&mut world, tank, 1, 1);
        advance(&mut world, 128);
        for pool in pools {
            assert!(world.get::<ComponentHealth>(pool).unwrap().is_empty());
        }
    }

    /// The game's damage model: the server's hold-last input must neither cancel an active repair
    /// nor restart a cancelled one. Removing consumable attestation breaks both halves.
    #[test]
    fn held_input_neither_cancels_nor_starts_twice() {
        let (mut world, tank, modules) = fixture();
        press(&mut world, tank, 1, 1);
        advance(&mut world, 1);
        press(&mut world, tank, 2, 1);
        assert!(
            world
                .get::<ModuleRepair>(modules[0])
                .unwrap()
                .elapsed
                .is_some()
        );
        press(&mut world, tank, 3, 3);
        press(&mut world, tank, 4, 3);
        assert!(
            world
                .get::<ModuleRepair>(modules[0])
                .unwrap()
                .elapsed
                .is_none()
        );
    }
    /// The game's damage model: replicas are presentation only. Removing either authority-only
    /// schedule gate lets a client start or advance work and creates a second timeline.
    #[test]
    fn replicas_never_start_or_advance_repairs() {
        let (mut world, tank, modules) = fixture();
        *world.get_mut::<ParsedTankCommand>(tank).unwrap() =
            ParsedTankCommand::try_from(TankCommand {
                repair: true,
                ..default()
            })
            .unwrap();
        let mut app = App::new();
        *app.world_mut() = world;
        app.init_resource::<bevy::ecs::schedule::Schedules>();
        app.insert_resource(crate::ClientReplica);
        install(&mut app);
        app.world_mut().run_schedule(FixedUpdate);
        assert!(
            app.world()
                .get::<ModuleRepair>(modules[0])
                .unwrap()
                .elapsed
                .is_none()
        );
        app.world_mut()
            .get_mut::<ModuleRepair>(modules[0])
            .unwrap()
            .elapsed = Some(0.5);
        app.world_mut().run_schedule(FixedPostUpdate);
        assert_eq!(
            app.world().get::<ModuleRepair>(modules[0]).unwrap().elapsed,
            Some(0.5)
        );
    }
}
