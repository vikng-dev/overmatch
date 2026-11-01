//! Battle-scoped environment construction, game state only. The replicated Battle number is
//! the transition; ledgers and pose carriers name that generation so arrival order cannot mix
//! worlds. No countdown, reset message or remembered transition flag advances a client world.

use bevy::ecs::query::QuerySingleError;
use bevy::prelude::*;
use lightyear::prelude::ReplicationSystems;

use super::battle::BattleState;
use crate::world::{CurrentWorld, WorldGeneration};

/// The identity of the construction currently installed here. The authority entity distinguishes
/// reconnects to servers whose Battle numbers happen to coincide.
#[derive(Resource, Default)]
struct BuiltWorld(Option<(Entity, u32)>);

pub(super) fn install_client(app: &mut App) {
    app.insert_resource(CurrentWorld(None))
        .init_resource::<BuiltWorld>()
        .add_systems(PreUpdate, follow_battle.after(ReplicationSystems::Receive))
        .add_systems(
            PostUpdate,
            hide_other_worlds
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
        );
}

fn follow_battle(world: &mut World) {
    let identity = match world.query::<(Entity, &BattleState)>().single(world) {
        Ok((entity, battle)) => (entity, battle.number),
        Err(QuerySingleError::NoEntities(_)) => {
            // A ledger may arrive before the first Battle, including on reconnect. No construction
            // is selected, so whichever Battle arrives next constructs its world.
            world.resource_mut::<CurrentWorld>().0 = None;
            world.resource_mut::<BuiltWorld>().0 = None;
            return;
        }
        Err(QuerySingleError::MultipleEntities(_)) => {
            panic!("a client has one active authority Battle after replication receive")
        }
    };
    if world.resource::<BuiltWorld>().0 == Some(identity) {
        return;
    }
    world.insert_resource(CurrentWorld(Some(WorldGeneration(identity.1))));
    crate::world::reconstruct(world);
    world.resource_mut::<BuiltWorld>().0 = Some(identity);
}

/// Called in the same command flush as the authority's next fill-up. New ledger entities start
/// new diff histories, so no acknowledged append cursor can span two worlds.
pub(super) fn start_next(world: &mut World, number: u32) {
    let old: Vec<_> = world
        .query_filtered::<Entity, Or<(
            With<super::walls::WallLedger>,
            With<super::destruction::DestructionLedger>,
            With<super::debris_stress::DebrisPose>,
        )>>()
        .iter(world)
        .collect();
    for entity in old {
        assert!(
            world.despawn(entity),
            "authority carriers and ledgers are separate roots"
        );
    }
    world.insert_resource(CurrentWorld(Some(WorldGeneration(number))));
    crate::world::reconstruct(world);
    world
        .run_system_cached(super::walls::spawn_ledger)
        .expect("a fresh world can construct its wall ledger");
    world
        .run_system_cached(super::destruction::spawn_ledger)
        .expect("a fresh world can construct its tree ledger");
}

/// Retired carriers can outlive the phase packet in flight. They keep their replication identity
/// until authority despawn arrives, but never draw against another world's construction.
fn hide_other_worlds(
    generation: Res<CurrentWorld>,
    mut carriers: Query<
        (
            &WorldGeneration,
            &mut Visibility,
            Has<crate::buildings::breakage::WallPiece>,
        ),
        With<super::debris_stress::DebrisPose>,
    >,
) {
    for (belongs, mut visibility, wall) in &mut carriers {
        if Some(*belongs) != generation.0 {
            visibility.set_if_neq(Visibility::Hidden);
        } else if !wall {
            // A wall's record owns its visibility; a trunk or lever carrier is visible at its pose.
            visibility.set_if_neq(Visibility::Inherited);
        }
    }
}

#[cfg(test)]
mod tests;
