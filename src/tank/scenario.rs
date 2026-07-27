use avian3d::prelude::RigidBody;
use bevy::asset::LoadState;
use bevy::prelude::*;

use super::model::{Controlled, Tank};
use super::spawn::{
    PendingTankAssets, TankContent, TankPresentation, TankSimSource, load_tank_assets,
    spawn_complete_tank,
};
use crate::sight::SightMode;
use crate::state::{AppState, GameplaySet};

pub fn sp_spawn_plugin(app: &mut App) {
    app.add_systems(Startup, load_tank_assets).add_systems(
        Update,
        spawn_tank_when_loaded.run_if(in_state(AppState::Loading)),
    );
}

/// Install the local duel's possession switch.
pub fn client_plugin(app: &mut App) {
    // Control systems in GameplaySet must see the new owner in the same frame.
    app.add_systems(
        Update,
        swap_controlled_tank
            .run_if(in_state(AppState::Playing))
            .before(GameplaySet),
    );
}

/// The local duel's two spawn POINTS — horizontal only (XZ), like every spawn definition in the
/// game. Their Y is sampled from the live surface at spawn time (`terrain_grid::spawn_pos`).
///
/// These used to be full poses with a hardcoded `y = 2.0`, which was correct for exactly one world:
/// the flat slab. Once the heightmap world landed, `--offline` spawned both Tigers ~116 m UNDER the
/// terrain (79 m after the 2560 → 1000 m re-scale) — they were never "falling through" anything,
/// they simply started inside the hill. The net server had already been fixed for this class; the
/// offline/single-player path had not, which is precisely why spawn Y is no longer authorable
/// anywhere.
pub(crate) const DUEL_SPAWN_XZ: [Vec2; 2] = [Vec2::new(10.0, 5.0), Vec2::new(10.0, -12.0)];

/// Admit the local duel after presentation preloading, then construct both roots completely. This
/// is an admission policy; assets do not initialize simulation state. Failed loads remain fatal.
fn spawn_tank_when_loaded(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pending: Option<Res<PendingTankAssets>>,
    source: TankSimSource,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(pending) = pending else {
        return;
    };
    for handle in [pending.spec.id().untyped(), pending.scene.id().untyped()] {
        if let LoadState::Failed(err) = asset_server.load_state(handle) {
            error!("required tank asset failed to load: {err}");
            panic!("required tank asset failed to load: {err}");
        }
    }
    if !pending.loaded(&asset_server) {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    // Both bodies simulate; only the Controlled marker selects input ownership. Each spawn point is
    // horizontal — the ground under it is sampled NOW, through the one rule every spawn path shares
    // (`terrain_grid::spawn_pos`: footprint max + clearance), so the duel lands on whatever world
    // this build actually has: heightmap, flat slab, or a re-authored map later.
    let grid = height.as_deref();
    spawn_tank(
        &mut commands,
        content,
        pending.presentation(),
        Transform::from_translation(crate::terrain_grid::spawn_pos(grid, DUEL_SPAWN_XZ[0]))
            .with_rotation(Quat::from_rotation_z(0.7)),
        "Tiger I (A)",
        true,
    );
    spawn_tank(
        &mut commands,
        content,
        pending.presentation(),
        Transform::from_translation(crate::terrain_grid::spawn_pos(grid, DUEL_SPAWN_XZ[1])),
        "Tiger I (B)",
        false,
    );
    commands.remove_resource::<PendingTankAssets>();
    next.set(AppState::Playing);
}

/// Spawn one complete dynamic tank for the local duel.
fn spawn_tank(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    transform: Transform,
    name: &str,
    controlled: bool,
) {
    let root = spawn_complete_tank(
        commands,
        content,
        presentation,
        (transform, Name::new(name.to_string()), RigidBody::Dynamic),
    );
    if controlled {
        commands.entity(root).insert(Controlled);
    }
}

/// Move local possession to the next tank and return to third-person view.
fn swap_controlled_tank(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    tanks: Query<Entity, With<Tank>>,
    controlled: Query<Entity, With<Controlled>>,
    mut mode: ResMut<SightMode>,
) {
    if !keys.just_pressed(KeyCode::Tab) {
        return;
    }
    let Ok(current) = controlled.single() else {
        return;
    };
    let all: Vec<Entity> = tanks.iter().collect();
    if all.len() < 2 {
        return;
    }
    let idx = all.iter().position(|&e| e == current).unwrap_or(0);
    let next = all[(idx + 1) % all.len()];
    if next == current {
        return;
    }
    commands.entity(current).remove::<Controlled>();
    commands.entity(next).insert(Controlled);

    *mode = SightMode::ThirdPerson;
}

#[cfg(test)]
mod spawn_contract_tests {
    use std::{collections::HashMap, sync::Arc};

    use bevy::prelude::{App, ResMut, Resource, Update};

    use super::super::spawn::TankSimSource;
    use crate::bake::{TankBlueprint, TankGeometry};
    use crate::spec::TankSpec;

    #[derive(Resource, Default)]
    struct SourceProbe(bool);

    fn probe_unresolved_handle(source: TankSimSource, mut probe: ResMut<SourceProbe>) {
        probe.0 = source.get().is_some();
    }

    #[test]
    fn sim_source_does_not_require_a_resolved_asset_handle() {
        let spec: TankSpec =
            ron::de::from_str(include_str!("../../assets/tiger_1/tiger_1.tank.ron"))
                .expect("the shipped Tiger spec must parse");
        let geometry = TankGeometry {
            nodes: Vec::new(),
            by_name: HashMap::new(),
            roadwheels: Vec::new(),
            collision_proxies: Vec::new(),
        };

        let mut app = App::new();
        app.insert_resource(TankBlueprint {
            geometry: Arc::new(geometry),
            spec: Arc::new(spec),
        })
        .init_resource::<SourceProbe>()
        .add_systems(Update, probe_unresolved_handle);
        app.update();

        assert!(
            app.world().resource::<SourceProbe>().0,
            "TankSimSource must read the eager blueprint without an asset-handle argument",
        );
    }
}
