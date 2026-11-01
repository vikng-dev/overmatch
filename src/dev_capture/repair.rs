//! The game's damage model: an offline-only cue for the repair panel's hidden stills.

use bevy::prelude::*;

use crate::ballistics::ComponentHealth;
use crate::command::TankCommand;
use crate::damage::{Ammo, CrewStation, FunctionRole, TankVolumes};
use crate::tank::Controlled;

pub(crate) fn offline_repair_plugin(app: &mut App) {
    if super::plan().is_some_and(|plan| plan.repair.is_some()) {
        app.add_systems(Update, start_panel_repair);
    }
}

fn start_panel_repair(
    time: Res<Time>,
    mut done: Local<bool>,
    mut tanks: Query<(&TankVolumes, &mut TankCommand), With<Controlled>>,
    mut modules: Query<
        (&FunctionRole, &mut ComponentHealth),
        (Without<Ammo>, Without<CrewStation>),
    >,
) {
    let cue = super::plan()
        .and_then(|plan| plan.repair.as_ref())
        .expect("the offline repair cue is mounted only when the plan names one");
    if *done || time.elapsed_secs() < cue.at {
        return;
    }
    // The capture waits for the controlled tank's complete data-built body.
    let Some((volumes, mut command)) = crate::tank::controlled_one(tanks.single_mut()) else {
        return;
    };
    // The tank owns non-module volumes too; only the requested functions are emptied.
    let mut found = std::collections::HashSet::new();
    for volume in volumes.iter() {
        if let Ok((function, mut health)) = modules.get_mut(volume)
            && cue.modules.contains(function)
        {
            health.drain();
            found.insert(*function);
        }
    }
    assert!(
        cue.modules.iter().all(|function| found.contains(function)),
        "capture repair.modules: every requested function must name a module on the selected tank"
    );
    command.repair = true;
    *done = true;
    info!("capture: the game's damage model: module pools emptied, production repair edge issued");
}
