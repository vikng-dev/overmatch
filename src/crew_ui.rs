//! The controlled tank's crew bar + swap input — the first piece of the controlled tank's fixed
//! player UI (the rest of its scattered readouts fold in here over time). Shared: both `GamePlugin`
//! and the armor sandbox mount it, each scoped to the tank marked [`Controlled`] (the sandbox marks
//! its single target). Digit `1`–`5` taps a source seat then a target — pure client-side selection
//! that resolves into a [`CrewSwap`] command; the sim (`damage::apply_crew_swap_commands`)
//! validates and starts the actual [`PendingSwap`]. The bar shows each seat, its occupant, deaths,
//! the current selection, and any in-flight swap countdown.

use bevy::prelude::*;

use crate::command::{CrewSwap, TankCommand};
use crate::damage::{
    Capability, CrewStation, Crewman, Dead, PendingSwap, TankCapabilities, TankKnockedOut,
    TankVolumes, VolumeFacets, VolumeOf, capability_effectiveness, evaluate, part_qualities,
};
use crate::spec::ViewKind;
use crate::tank::{Controlled, TankRoot, TankSim, TankViews, Weapon, WeaponIndex};

/// The controlled tank's crew bar: one cell per seat, driven by the `1`–`5` swap input.
#[derive(Component)]
struct CrewBarText;

/// The controlled tank's vitals panel (top-left corner): crew count, capability, and weapon state.
#[derive(Component)]
struct StatusPanelText;

/// Crew-bar selection: the seat tapped as the swap *source*, awaiting a target.
#[derive(Resource, Default)]
struct CrewSelect {
    source: Option<CrewStation>,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<CrewSelect>()
        .add_systems(Startup, (spawn_crew_bar, spawn_status_panel))
        .add_systems(
            Update,
            (crew_swap_input, update_crew_bar, update_status_panel),
        );
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

fn spawn_status_panel(mut commands: Commands) {
    // Top-left — the controlled tank's vitals card (the crew bar owns bottom-left). A subtle dark
    // card behind a compact multi-line stat block, driven by `update_status_panel`.
    commands.spawn((
        StatusPanelText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 0.95, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            left: Val::Px(10.0),
            padding: UiRect::all(Val::Px(8.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.04, 0.06, 0.08, 0.62)),
    ));
}

/// Render the controlled tank's vitals card: crew alive/total, Drive + Gunner-sight effectiveness,
/// and each weapon's reload/fire state (`READY` / `x.xs` / `no-fire`). A pure view over the same
/// local components the world-space status block used to show — the seed of the designed player HUD.
fn update_status_panel(
    tank: Query<
        (
            Entity,
            &TankVolumes,
            &TankCapabilities,
            &TankViews,
            Option<&TankKnockedOut>,
        ),
        With<Controlled>,
    >,
    seats: Query<(Option<&Dead>, &VolumeOf), With<CrewStation>>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot)>,
    sims: Query<&TankSim>,
    facets: Query<VolumeFacets>,
    mut panel: Query<(&mut Text, &mut Visibility), With<StatusPanelText>>,
) {
    let Ok((mut text, mut visibility)) = panel.single_mut() else {
        return;
    };
    // Show the first controlled tank (exactly one on a client). Hide the whole card — text AND its
    // background — whenever none resolves, so no empty dark panel lingers in the corner.
    let Some((tank, volumes, caps, views, knocked_out)) = tank.iter().next() else {
        *text = Text::new("");
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Visible;

    // Crew alive/total: this tank's seats, a seat down when flagged `Dead` (the notion the crew bar
    // draws too).
    let mut total = 0;
    let mut alive = 0;
    for (dead, owner) in &seats {
        if owner.tank() == tank {
            total += 1;
            if dead.is_none() {
                alive += 1;
            }
        }
    }

    // Capabilities: Drive (whole-tank, composed from its requirement groups) + the Gunner sight's
    // live gate. Part qualities resolved once for the sight + weapon gates below.
    let quality = part_qualities(volumes, &facets);
    let drive = capability_effectiveness(Some(volumes), Some(caps), Capability::Drive, &facets);
    let gunner = views
        .0
        .get(&ViewKind::Gunner)
        .map_or(0.0, |view| evaluate(&view.requires, &quality));

    // Weapons: name + reload state, flagged `no-fire` when the fire gate is unmet (dead
    // gunner/loader/breech). Sorted by name so the order is stable frame to frame.
    let mut weapon_entries = weapons
        .iter()
        .filter(|(_, _, root)| root.0 == tank)
        .map(|(weapon, slot, root)| {
            // Reload state is root-resident (`TankSim`), addressed by the weapon's slot.
            let remaining = sims
                .get(root.0)
                .ok()
                .and_then(|sim| sim.weapons.get(slot.0))
                .map_or(0.0, |w| w.reload_remaining);
            let status = if remaining > 0.0 {
                format!("{remaining:.1}s")
            } else if evaluate(&weapon.fire, &quality) > 0.0 {
                "READY".to_string()
            } else {
                "no-fire".to_string()
            };
            (
                weapon.name.to_string(),
                format!("{} {}", weapon.name, status),
            )
        })
        .collect::<Vec<_>>();
    // Order by weapon name only — not the mutable status text — so rows don't reshuffle as reload
    // state changes; "-" stands in for a tank with no weapons (the world-space block's placeholder).
    weapon_entries.sort_by(|a, b| a.0.cmp(&b.0));
    let weapon_line = if weapon_entries.is_empty() {
        "-".to_string()
    } else {
        weapon_entries
            .into_iter()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>()
            .join("   ")
    };

    // Terminal state: ALIVE, or the knock-out and its cause.
    let state = knocked_out
        .map(|ko| format!("KNOCKED OUT ({})", ko.reason.label()))
        .unwrap_or_else(|| "ALIVE".to_string());

    *text = Text::new(format!(
        "{state}\nCrew {alive}/{total}\n{} {}%   Gunner {}%\nWeapons: {}",
        Capability::Drive.label(),
        (drive * 100.0).round() as i32,
        (gunner * 100.0).round() as i32,
        weapon_line,
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
