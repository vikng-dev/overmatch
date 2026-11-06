// Functional observation of the game's environment destruction model, game state only.
use super::*;
use crate::construction::breakage::{Breakage, SweepAccount};
use crate::headless_test::{booted_sim_on, drive_tick};
use avian3d::prelude::LinearVelocity;

#[test]
#[ignore = "functional ram observations, no pricing"]
fn tracked_ram_rubble_observation() {
    let mut app = booted_sim_on(Some(crate::terrain_grid::tests::shipped_grid()));
    app.init_resource::<Sheds>()
        .add_systems(FixedPostUpdate, keep_sheds.after(LawSet));
    let crate::headless_test::TrackedRamSetup {
        tank,
        target,
        ordinal,
        drive_ticks,
        deadline_ticks,
    } = crate::headless_test::prepare_tracked_ram(&mut app);
    let masses: Vec<_> = app
        .world()
        .get::<Breakage>(target)
        .unwrap()
        .live
        .structure
        .chunks
        .iter()
        .map(|c| c.mass)
        .collect();
    let initial_mass: f32 = masses.iter().sum();
    let mut bodies_thirty = (0, 0.0_f32);
    let mut contact = None;
    let mut speeds = Vec::new();
    for tick in 0..drive_ticks + deadline_ticks {
        drive_tick(&mut app, tank, f32::from(tick < drive_ticks), 0.0);
        let account = app.world().resource::<SweepAccount>();
        if contact.is_none() && account.contacts.contains_key(&(tank, ordinal)) {
            contact = Some(tick);
            println!(
                "MEASURED first occupied ram contact: {:?}",
                account
                    .samples
                    .iter()
                    .filter(|(b, c, ..)| *b == tank && *c == ordinal)
                    .collect::<Vec<_>>()
            );
        }
        if let Some(first) = contact {
            let after = tick - first;
            if [1, 2, 5, 10, 30].contains(&after) {
                speeds.push((
                    after,
                    app.world().get::<LinearVelocity>(tank).unwrap().0.length(),
                ));
            }
            if after == 30 {
                let pieces: Vec<_> = app
                    .world_mut()
                    .query::<(&Piece, &ComputedMass)>()
                    .iter(app.world())
                    .filter(|(piece, _)| piece.construction == ordinal)
                    .map(|(_, mass)| mass.value())
                    .collect();
                bodies_thirty = (pieces.len(), pieces.iter().sum());
            }
        }
    }
    let decor: std::collections::BTreeSet<_> = app
        .world()
        .resource::<Sheds>()
        .0
        .iter()
        .filter(|shed| shed.construction == ordinal)
        .flat_map(|shed| shed.chunks.iter().copied())
        .collect();
    let decor_mass: f32 = decor.iter().map(|&c| masses[usize::from(c)]).sum();
    let mut server_chunks = std::collections::BTreeSet::new();
    let mut rubble_chunks = std::collections::BTreeSet::new();
    for (piece, breakage) in app
        .world_mut()
        .query::<(Option<&Piece>, &Breakage)>()
        .iter(app.world())
    {
        if breakage.construction != ordinal {
            continue;
        }
        match piece {
            Some(piece) => {
                server_chunks.extend(piece.chunks.iter().copied());
                rubble_chunks.extend(piece.chunks.iter().copied());
            }
            None => server_chunks.extend(
                breakage
                    .live
                    .state
                    .fate
                    .iter()
                    .enumerate()
                    .filter_map(|(c, fate)| (*fate == Fate::Standing).then_some(c as u16)),
            ),
        }
    }
    let kept_mass: f32 = server_chunks.iter().map(|&c| masses[usize::from(c)]).sum();
    let rubble_mass: f32 = rubble_chunks.iter().map(|&c| masses[usize::from(c)]).sum();
    println!(
        "MEASURED ram rubble: speeds_mps={speeds:?}; bodies_at_plus30={}; mass_at_plus30_kg={}; initial_kg={initial_mass}; server_total_kg={kept_mass}; server_rubble_kept_kg={rubble_mass}; decor_kg={decor_mass}; unaccounted_kg={}",
        bodies_thirty.0,
        bodies_thirty.1,
        initial_mass - kept_mass - decor_mass
    );
    assert!(
        contact.is_some() && speeds.len() == 5,
        "the functional fixture must reach contact and finish its observation window"
    );
}
