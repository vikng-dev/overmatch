//! Controlled-tank crew UI and swap selection.
//!
//! UI selection emits [`CrewSwap`]; simulation validates and executes the swap.

use bevy::prelude::*;

use crate::command::{CrewSwap, TankCommand};
use crate::damage::{
    Capability, CrewStation, Crewman, Dead, PendingSwap, TankCapabilities, TankKnockedOut,
    TankVolumes, VolumeFacets, VolumeOf, capability_effectiveness, evaluate, part_qualities,
};
use crate::spec::{FireMode, ViewKind};
use crate::tank::{Controlled, TankRoot, TankViews, Weapon, WeaponIndex};
use crate::ui_font::UiFonts;

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

fn spawn_crew_bar(mut commands: Commands, fonts: Res<UiFonts>) {
    // Bottom-left — one cell per seat, driven by `update_crew_bar`.
    commands.spawn((
        CrewBarText,
        Text::new(""),
        TextFont {
            // Regular: a dense multi-cell seat readout.
            font: fonts.body.clone().into(),
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

fn spawn_status_panel(mut commands: Commands, fonts: Res<UiFonts>) {
    // Top-left — the controlled tank's vitals card (the crew bar owns bottom-left). A subtle dark
    // card behind a compact multi-line stat block, driven by `update_status_panel`.
    commands.spawn((
        StatusPanelText,
        Text::new(""),
        TextFont {
            // Regular: a dense multi-line stat block.
            font: fonts.body.clone().into(),
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

/// One weapon's status readout, split out so the precedence is unit-testable without standing up the
/// whole ECS panel. `no_fire` is the per-weapon fire gate (dead gunner/breech/barrel) — an *unusable*
/// weapon. The crew-gate/no-fire state wins over any timer readout: a weapon that can't fire must not
/// tease a reload or swap countdown (a dead gun's ready deadline advances to preserve its remaining
/// work, so the derived countdown would otherwise stick forever).
fn weapon_status(
    fire_mode: FireMode,
    no_fire: bool,
    remaining_secs: f32,
    belt_remaining: u32,
) -> String {
    match fire_mode {
        FireMode::Single { .. } => {
            if no_fire {
                "no-fire".to_string()
            } else if remaining_secs > 0.0 {
                format!("{remaining_secs:.1}s")
            } else {
                "READY".to_string()
            }
        }
        FireMode::Automatic { .. } => {
            if no_fire {
                "no-fire".to_string()
            } else if belt_remaining == 0 {
                // Dry belt = swap in flight; the timer freezes (and this readout with it) while the
                // gun crew can't work the swap. Only reached when the gun CAN fire (no_fire is
                // false) — a crew-dead MG reads `no-fire`, not a stuck `SWAP` countdown.
                format!("SWAP {remaining_secs:.1}s")
            } else {
                format!("{belt_remaining} rds")
            }
        }
    }
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
    gates: Query<&crate::tank::WeaponGate>,
    weapon_clock: Res<crate::WeaponClock>,
    fixed_time: Res<Time<Fixed>>,
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

    // Weapons: name + per-fire-mode state, flagged `no-fire` when the fire gate is unmet (dead
    // gunner/loader/breech). A `Single` reads as the classic READY / reload countdown; an
    // `Automatic` shows its belt count, or the swap countdown while the belt is dry. Sorted by
    // name so the order is stable frame to frame.
    let mut weapon_entries = weapons
        .iter()
        .filter(|(_, _, root)| root.0 == tank)
        .map(|(weapon, slot, root)| {
            // Fire-gate state is the root's tick-correlated `WeaponGate`, addressed by slot.
            let state = gates
                .get(root.0)
                .ok()
                .and_then(|gate| gate.weapons.get(slot.0).copied())
                .unwrap_or_default();
            let remaining_secs =
                state.remaining_ticks(weapon_clock.0) as f32 * fixed_time.timestep().as_secs_f32();
            let no_fire = evaluate(&weapon.fire, &quality) <= 0.0;
            let status = weapon_status(
                weapon.fire_mode,
                no_fire,
                remaining_secs,
                state.belt_remaining,
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    const AUTO: FireMode = FireMode::Automatic {
        rpm: 750.0,
        belt_size: 150,
        belt_swap_secs: 3.5,
        tracer_every: 5,
    };
    const SINGLE: FireMode = FireMode::Single { reload_secs: 3.0 };

    /// The crew-gate/no-fire state wins over every timer readout, in BOTH fire modes: a weapon that
    /// can't fire reads `no-fire`, never a (frozen) reload or swap countdown. This is the regression
    /// guard for the dry-belt MG whose swap timer froze on a crew-dead gun and showed a permanently
    /// stuck `SWAP` countdown instead of `no-fire`.
    #[test]
    fn no_fire_wins_over_frozen_timers() {
        // Automatic, dry belt, gun crew dead: the swap timer is frozen — must read `no-fire`.
        assert_eq!(weapon_status(AUTO, true, 2.7, 0), "no-fire");
        // Single, mid-reload, fire crew dead: the reload is frozen — must read `no-fire`.
        assert_eq!(weapon_status(SINGLE, true, 1.4, 0), "no-fire");
    }
}
