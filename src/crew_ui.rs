//! The controlled tank's crew bar + swap input — the first piece of the controlled tank's fixed
//! player UI (the rest of its scattered readouts fold in here over time). Shared: both `GamePlugin`
//! and the armor sandbox mount it, each scoped to the tank marked [`Controlled`] (the sandbox marks
//! its single target). Digit `1`–`5` taps a source seat then a target — pure client-side selection
//! that resolves into a [`CrewSwap`] command; the sim (`damage::apply_crew_swap_commands`)
//! validates and starts the actual [`PendingSwap`]. The bar shows each seat, its occupant, deaths,
//! the current selection, and any in-flight swap countdown.

use bevy::prelude::*;

use crate::command::{CrewSwap, TankCommand};
use crate::damage::{CrewStation, Crewman, Dead, PendingSwap, VolumeOf};
use crate::tank::Controlled;

/// The controlled tank's crew bar: one cell per seat, driven by the `1`–`5` swap input.
#[derive(Component)]
struct CrewBarText;

/// Crew-bar selection: the seat tapped as the swap *source*, awaiting a target.
#[derive(Resource, Default)]
struct CrewSelect {
    source: Option<CrewStation>,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<CrewSelect>()
        .add_systems(Startup, spawn_crew_bar)
        .add_systems(Update, (crew_swap_input, update_crew_bar));
}

fn spawn_crew_bar(mut commands: Commands) {
    // Bottom-left — one cell per seat, driven by `update_crew_bar`.
    commands.spawn((
        CrewBarText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.92, 0.94, 0.78)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(10.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));
}

/// `1`–`5` crew-bar input for the controlled tank: tap a seat to select it as the swap source, tap
/// a second to command the swap; re-tapping the source (or any seat while a swap is mid-flight)
/// cancels. Selection is client-side state; only the resolved [`CrewSwap`] intent goes on the
/// command — the sim starts (and validates) the actual swap.
fn crew_swap_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut select: ResMut<CrewSelect>,
    mut tank: Query<(Entity, Option<&PendingSwap>, &mut TankCommand), With<Controlled>>,
    seats: Query<(Entity, &CrewStation, &VolumeOf)>,
) {
    const DIGITS: [KeyCode; 5] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ];
    let Some(slot) = DIGITS.iter().position(|k| keys.just_pressed(*k)) else {
        return;
    };
    let Ok((tank, pending, mut command)) = tank.single_mut() else {
        return;
    };

    // Seats for this tank in enum (slot) order.
    let mut ordered: Vec<CrewStation> = seats
        .iter()
        .filter(|(_, _, owner)| owner.tank() == tank)
        .map(|(_, s, _)| *s)
        .collect();
    ordered.sort();
    let Some(&seat_station) = ordered.get(slot) else {
        return;
    };

    // Any tap while a swap is in flight cancels it.
    if pending.is_some() {
        command.crew_swap = Some(CrewSwap::Cancel);
        select.source = None;
        return;
    }

    match select.source {
        None => select.source = Some(seat_station),
        Some(src) if src == seat_station => select.source = None, // re-tap = deselect
        Some(src) => {
            command.crew_swap = Some(CrewSwap::Start(src, seat_station));
            select.source = None;
        }
    }
}

/// Render the controlled tank's crew bar: `N: Seat` per seat in enum order, plus the occupant (when
/// foreign) and a dead mark. The selected source is bracketed `[..]`; a pending swap marks both seats
/// `~..~` and shows its countdown.
fn update_crew_bar(
    select: Res<CrewSelect>,
    tank: Query<(Entity, Option<&PendingSwap>), With<Controlled>>,
    seats: Query<(Entity, &CrewStation, &Crewman, Option<&Dead>, &VolumeOf)>,
    mut bar: Query<&mut Text, With<CrewBarText>>,
) {
    let Ok(mut text) = bar.single_mut() else {
        return;
    };
    let Ok((tank, pending)) = tank.single() else {
        *text = Text::new("");
        return;
    };

    let mut ordered: Vec<(Entity, CrewStation, Crewman, bool)> = seats
        .iter()
        .filter(|(_, _, _, _, owner)| owner.tank() == tank)
        .map(|(e, s, c, dead, _)| (e, *s, *c, dead.is_some()))
        .collect();
    ordered.sort_by_key(|(_, s, _, _)| *s);

    let cells: Vec<String> = ordered
        .iter()
        .enumerate()
        .map(|(i, (entity, seat, crewman, dead))| {
            // Notes after the seat name: who is actually manning it (when foreign), and whether the
            // occupant is dead — e.g. "Loader (Commander, dead)".
            let mut notes: Vec<&str> = Vec::new();
            if crewman.home != *seat {
                notes.push(crewman.home.label());
            }
            if *dead {
                notes.push("dead");
            }
            let detail = if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join(", "))
            };
            let mut cell = format!("{}: {}{}", i + 1, seat.label(), detail);
            if select.source == Some(*seat) {
                cell = format!("[{cell}]");
            }
            if let Some(ps) = pending
                && (*entity == ps.a || *entity == ps.b)
            {
                cell = format!("~{cell}~");
            }
            cell
        })
        .collect();

    let prefix = match pending {
        Some(ps) => format!("SWAP {:.1}s   ", ps.remaining.max(0.0)),
        None => String::new(),
    };
    *text = Text::new(format!("{prefix}{}", cells.join("    ")));
}
