// Functional observation of the game's environment destruction model, game state only.
use super::*;
use crate::construction::breakage::SweepAccount;
use crate::headless_test::{booted_sim_on, drive_tick};
use std::collections::{BTreeMap, BTreeSet};

#[test]
#[ignore = "headless lifecycle census, no timing or network"]
fn tracked_ram_entity_census() {
    let mut app = booted_sim_on(Some(crate::terrain_grid::tests::shipped_grid()));
    // Fixture generation; install the shipped carrier path without a transport or UDP socket.
    app.init_resource::<bevy_replicon::server::visibility::registry::FilterRegistry>()
        .insert_resource(AuthorityWorld(crate::world::WorldGeneration(1)));
    install_server_from(&mut app, None, scenario::RON).unwrap();
    app.init_resource::<crate::ShotClock>()
        .add_systems(FixedFirst, census_clock);
    install_counts(&mut app);
    let crate::headless_test::TrackedRamSetup {
        tank,
        ordinal,
        drive_ticks,
        deadline_ticks,
        ..
    } = crate::headless_test::prepare_tracked_ram(&mut app);
    let mut contact = None;
    for tick in 0..drive_ticks + deadline_ticks {
        drive_tick(&mut app, tank, f32::from(tick < drive_ticks), 0.0);
        let account = app.world().resource::<SweepAccount>();
        if contact.is_none() && account.contacts.contains_key(&(tank, ordinal)) {
            contact = Some(tick);
            println!(
                "MEASURED census first_contact_tick={}",
                simulation_tick(app.world().resource::<Time<Fixed>>())
            );
        }
        let actual: BTreeSet<_> = app
            .world_mut()
            .query::<Entity>()
            .iter(app.world())
            .collect();
        assert_eq!(
            app.world().resource::<Churn>().alive,
            actual,
            "lifecycle probes must account for every entity at each fixed-step boundary"
        );
        // Fixture: the historical bookkeeping census ends at contact +320.
        if contact.is_some_and(|first| tick - first == 320) {
            break;
        }
    }
    assert!(
        contact.is_some(),
        "the functional fixture reaches occupied contact"
    );
    for (&tick, counts) in &app.world().resource::<Churn>().rows {
        println!(
            "MEASURED lifecycle {}",
            serde_json::json!({
                "tick": tick, "colliders_created": counts[0], "colliders_deleted": counts[1],
                "bodies_created": counts[2], "entities_spawned": counts[3], "entities_despawned": counts[4],
            })
        );
    }
}

#[derive(Resource, Default)]
struct Churn {
    alive: BTreeSet<Entity>,
    rows: BTreeMap<u128, [usize; 5]>,
}

fn simulation_tick(time: &Time<Fixed>) -> u128 {
    time.elapsed().as_nanos() / time.timestep().as_nanos()
}

/// The transport-free fixture publishes the same fixed-step identity used for its rows.
fn census_clock(time: Res<Time<Fixed>>, mut clock: ResMut<crate::ShotClock>) {
    clock.0 = crate::sim_tick::SimTick::new(
        u32::try_from(simulation_tick(&time))
            .expect("the finite census window fits the tick clock"),
    );
}

fn entity_added(event: On<Add>, time: Res<Time<Fixed>>, mut counts: ResMut<Churn>) {
    if counts.alive.insert(event.entity) {
        counts.rows.entry(simulation_tick(&time)).or_default()[3] += 1;
    }
}

fn entity_despawned(event: On<Despawn>, time: Res<Time<Fixed>>, mut counts: ResMut<Churn>) {
    if counts.alive.remove(&event.entity) {
        counts.rows.entry(simulation_tick(&time)).or_default()[4] += 1;
    }
}

/// Bevy 0.19.1 component-scoped lifecycle observers trigger once per contributed component.
/// Deduplicating entities includes same-step births/departures; the boundary census checks coverage.
fn install_counts(app: &mut App) {
    app.init_resource::<Churn>();
    let components: Vec<_> = app
        .world()
        .components()
        .iter_registered()
        .map(|info| info.id())
        .collect();
    app.world_mut()
        .spawn(Observer::new(entity_added).with_components(components.clone()));
    app.world_mut()
        .spawn(Observer::new(entity_despawned).with_components(components));
    app.add_observer(
        |_: On<Add, Collider>, time: Res<Time<Fixed>>, mut counts: ResMut<Churn>| {
            counts.rows.entry(simulation_tick(&time)).or_default()[0] += 1;
        },
    );
    app.add_observer(
        |_: On<Remove, Collider>, time: Res<Time<Fixed>>, mut counts: ResMut<Churn>| {
            counts.rows.entry(simulation_tick(&time)).or_default()[1] += 1;
        },
    );
    app.add_observer(
        |_: On<Add, RigidBody>, time: Res<Time<Fixed>>, mut counts: ResMut<Churn>| {
            counts.rows.entry(simulation_tick(&time)).or_default()[2] += 1;
        },
    );
    app.world_mut().flush();
    let alive = app
        .world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect();
    let mut counts = app.world_mut().resource_mut::<Churn>();
    counts.alive = alive;
    counts.rows.clear();
}
