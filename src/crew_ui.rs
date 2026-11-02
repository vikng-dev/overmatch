//! The game's damage-model view: controlled-tank crew, weapon and repair status, and swap selection.
//!
//! UI selection emits [`CrewSwap`]; simulation validates and executes the swap.

use bevy::prelude::*;

use crate::command::{CrewSwap, TankCommand, TankDeviceSet};
use crate::damage::{
    Capability, CrewStation, Crewman, Dead, PendingSwap, TankCapabilities, TankKnockedOut,
    TankVolumes, VolumeFacets, VolumeOf, evaluate, part_facts,
};
use crate::flow::{Flow, FlowScoped, InMatchSystems, MatchResourceExt};
use crate::spec::FireCycle;
use crate::tank::{
    Controlled, TankRoot, Weapon, WeaponIndex, WeaponLoad, WeaponLoads, WeaponRound,
};
use crate::ui_font::{PANEL_BG, TEXT, UiFonts, set_text};

/// The controlled tank's crew bar: one cell per seat, driven by the `1`–`5` swap input.
#[derive(Component)]
pub(crate) struct CrewBarText;

/// The controlled tank's vitals panel (top-left corner): crew count, capability, and weapon state.
#[derive(Component)]
pub(crate) struct StatusPanelText;

/// Crew-bar selection: the seat tapped as the swap *source*, awaiting a target.
#[derive(Resource, Default)]
struct CrewSelect {
    source: Option<CrewStation>,
}

/// Mount the crew bar + vitals panel.
///
/// CONTRACT: the mounting root owes this plugin a [`crate::WeaponClock`] — [`update_status_panel`]
/// derives each weapon's countdown against it and takes it as a plain `Res`, so a root that lacks
/// one dies on its first `Update` with a parameter-validation panic. The game gets it from
/// `shooting::plugin` and the net client/server from `net::protocol`. It stays a hard requirement
/// rather than an `Option` on purpose: absent, the panel could only invent a readout, and the tank
/// whose reload state cannot be told is the one this card exists to report on.
pub fn plugin(app: &mut App) {
    crate::flow::require(app, "crew_ui");
    app.init_match_resource::<CrewSelect>()
        .add_systems(OnEnter(Flow::InMatch), (spawn_crew_bar, spawn_status_panel))
        .add_systems(
            Update,
            (
                crew_swap_input.in_set(TankDeviceSet),
                update_crew_bar,
                update_status_panel,
            )
                .in_set(InMatchSystems),
        );
}

pub(crate) fn spawn_crew_bar(mut commands: Commands, fonts: Res<UiFonts>) {
    // Bottom-left — one cell per seat, driven by `update_crew_bar`.
    commands.spawn((
        FlowScoped(Flow::InMatch),
        CrewBarText,
        crate::hud::HudDock::Fixed,
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

pub(crate) fn spawn_status_panel(mut commands: Commands, fonts: Res<UiFonts>) {
    // Top-left — the controlled tank's vitals card (the crew bar owns bottom-left). A subtle dark
    // card behind a compact multi-line stat block, driven by `update_status_panel`.
    commands.spawn((
        FlowScoped(Flow::InMatch),
        StatusPanelText,
        crate::hud::HudDock::Fixed,
        Text::new(""),
        TextFont {
            // Regular: a dense multi-line stat block.
            font: fonts.body.clone().into(),
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(TEXT),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            left: Val::Px(10.0),
            padding: UiRect::all(Val::Px(8.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(PANEL_BG),
    ));
}

/// One weapon's status readout, split out so the precedence is unit-testable without standing up the
/// whole ECS panel. `no_fire` is the per-weapon fire gate (gunner/breech/barrel out of action) — an *unusable*
/// weapon. The crew-gate/no-fire state wins over any timer readout: a weapon that can't fire must not
/// tease a reload or swap countdown. A load pause is explicit even when its remaining work is zero:
/// the weapon is not ready until a worker resumes it.
fn weapon_status(
    fire_cycle: FireCycle,
    no_fire: bool,
    remaining_secs: f32,
    belt_remaining: u32,
    paused: bool,
) -> String {
    if no_fire {
        return "no-fire".to_string();
    }
    if paused {
        return match fire_cycle {
            FireCycle::Single { .. } => "LOAD PAUSED".to_string(),
            FireCycle::Automatic { .. } => "SWAP PAUSED".to_string(),
        };
    }
    match fire_cycle {
        FireCycle::Single { .. } if remaining_secs > 0.0 => format!("{remaining_secs:.1}s"),
        FireCycle::Single { .. } => "READY".to_string(),
        FireCycle::Automatic { .. } if belt_remaining == 0 => format!("SWAP {remaining_secs:.1}s"),
        FireCycle::Automatic { .. } => format!("{belt_remaining} rds"),
    }
}

/// One weapon's round readout: the loaded round's display name, and the selected round's while it
/// waits for the next load, each with its fuze mode's name in brackets when its fuze lists more
/// than one mode. `None` for a weapon with nothing to choose: one round, with one fuze mode or none.
fn round_line(weapon_name: &str, rounds: &[WeaponRound], load: &WeaponLoad) -> Option<String> {
    let pickable = |round: &WeaponRound| round.fuze_modes().len() > 1;
    if rounds.len() < 2 && !rounds.iter().any(pickable) {
        return None;
    }
    let name = |index: u8| {
        let round = rounds
            .get(usize::from(index))
            .expect("a weapon load indexes the same stowage built at spawn");
        let mode = load
            .fuze_mode(rounds, index)
            .and_then(|mode| round.fuze_modes().get(usize::from(mode)))
            .filter(|_| pickable(round));
        match mode {
            Some(mode) => format!("{} [{}]", round.name, mode.name),
            None => round.name.clone(),
        }
    };
    let mut line = format!("{weapon_name} round: {}", name(load.loaded));
    if load.selected != load.loaded {
        line.push_str(&format!("   next: {}", name(load.selected)));
    }
    Some(line)
}

/// The game's damage model: a reduced module names the held-share rate its consumers use.
fn module_status(
    health: &crate::ballistics::ComponentHealth,
    repair: &crate::damage::repair::ModuleRepair,
) -> String {
    match repair.fraction(health) {
        Some(fraction) => format!("REPAIR {:.0}%", (fraction * 100.0).floor()),
        None if health.is_empty() => "OUT OF ACTION".to_owned(),
        None if health.held() < health.capacity() => {
            format!(
                "RATE {}%",
                served_percent(crate::damage::module_share(health))
            )
        }
        None => "READY".to_owned(),
    }
}

/// A served share as a whole percent the player can act on: a share that still serves never
/// reads 0, and one short of whole never reads 100.
fn served_percent(share: f32) -> i32 {
    let percent = (share * 100.0).round() as i32;
    if share <= 0.0 {
        0
    } else if share >= 1.0 {
        100
    } else {
        percent.clamp(1, 99)
    }
}

/// Render the controlled tank's vitals card: crew available/total, the served Drive task rate,
/// each weapon's reload/fire state (`READY` / `x.xs` / `no-fire`), and the round choice of each
/// weapon that has one ([`round_line`]), plus module repair progress and readiness.
/// A pure view over the controlled tank's local components.
#[expect(
    clippy::too_many_arguments,
    reason = "a view over the controlled tank's crew, weapons, gates and clocks, writing one panel"
)]
fn update_status_panel(
    tank: Query<
        (
            Entity,
            &TankVolumes,
            &TankCapabilities,
            Option<&TankKnockedOut>,
            &crate::command::ParsedTankCommand,
        ),
        With<Controlled>,
    >,
    seats: Query<(Option<&Dead>, &VolumeOf), With<CrewStation>>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot)>,
    gates: Query<&crate::tank::WeaponGate>,
    // The authority's round choice; on a client, the owner's replicated copy.
    loads: Query<&WeaponLoads>,
    weapon_clock: Res<crate::WeaponClock>,
    fixed_time: Res<Time<Fixed>>,
    modules: Query<
        (
            &Name,
            &crate::ballistics::ComponentHealth,
            &crate::damage::repair::ModuleRepair,
        ),
        crate::damage::repair::Modules,
    >,
    facets: Query<VolumeFacets>,
    mut panel: Query<(&mut Text, &mut Visibility), With<StatusPanelText>>,
) {
    let (mut text, mut visibility) = panel
        .single_mut()
        .expect("crew UI mounts exactly one status panel for the match");
    // No local rig before spawn or between possessions: hide the card and its background.
    let Some((tank, volumes, caps, knocked_out, command)) =
        crate::tank::controlled_one(tank.single())
    else {
        set_text(text.reborrow(), "");
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    visibility.set_if_neq(Visibility::Visible);

    // Crew active/total: this tank's seats, a seat down when flagged `Dead` (the notion the crew bar
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

    // The game's damage model: the selector supplies one task rate to the sim and HUD;
    // served throughput masks unstaffed controls; brake holds remain separate intent facts.
    let quality = part_facts(volumes, &facets).quality;
    let target = crate::track::sim::drive_target(command, Some(volumes), Some(caps), &facets);
    let drive_line = drive_rate_line(&target);

    // Weapons: name + per-fire-mode state, flagged `no-fire` when the fire gate is unmet (knocked-out
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
                .expect("complete tank roots carry their weapon gate")
                .weapons
                .get(slot.0)
                .copied()
                .expect("weapon and gate slots are built together at spawn");
            let remaining_secs =
                state.remaining_ticks(weapon_clock.0) as f32 * fixed_time.timestep().as_secs_f32();
            let no_fire = evaluate(&weapon.fire, &quality) <= 0.0;
            let status = weapon_status(
                weapon.fire_cycle,
                no_fire,
                remaining_secs,
                state.belt_remaining,
                state.paused_at_tick().is_some(),
            );
            let load = loads
                .get(root.0)
                .expect("complete tank roots carry their weapon loads")
                .weapons
                .get(slot.0)
                .expect("weapon and load slots are built together at spawn");
            let rounds = round_line(&weapon.name, &weapon.rounds, load);
            (
                weapon.name.to_string(),
                format!("{} {}", weapon.name, status),
                rounds,
            )
        })
        .collect::<Vec<_>>();
    // Order by weapon name only — not the mutable status text — so rows don't reshuffle as reload
    // state changes; "-" stands in for a tank with no weapons (the world-space block's placeholder).
    weapon_entries.sort_by(|a, b| a.0.cmp(&b.0));
    let round_lines: String = weapon_entries
        .iter()
        .filter_map(|(_, _, rounds)| rounds.as_deref())
        .map(|line| format!("\n{line}"))
        .collect();
    let weapon_line = if weapon_entries.is_empty() {
        "-".to_string()
    } else {
        weapon_entries
            .into_iter()
            .map(|(_, entry, _)| entry)
            .collect::<Vec<_>>()
            .join("   ")
    };

    // Terminal state: ALIVE, or the knock-out and its cause.
    let state = knocked_out
        .map(|ko| format!("KNOCKED OUT ({})", ko.reason.label()))
        .unwrap_or_else(|| "ALIVE".to_string());

    // The tank also owns armor, crew and ammunition volumes; only module functions get rows.
    let mut module_rows: Vec<_> = volumes
        .iter()
        .filter_map(|volume| {
            modules.get(volume).ok().map(|(name, health, repair)| {
                let status = module_status(health, repair);
                (name.as_str(), format!("\n{name}: {status}"))
            })
        })
        .collect();
    module_rows.sort_by_key(|(name, _)| *name);
    let repairing = volumes.iter().any(|volume| {
        modules
            .get(volume)
            .is_ok_and(|(_, _, repair)| repair.elapsed.is_some())
    });
    let repair_hint = if knocked_out.is_some() {
        "Repair unavailable"
    } else if repairing {
        "Repairing — F cancels"
    } else {
        "F — repair modules"
    };
    let module_lines: String = module_rows.into_iter().map(|(_, row)| row).collect();
    let card = format!(
        "{state}\nCrew {alive}/{total}\n{drive_line}\nWeapons: {weapon_line}{round_lines}\n{repair_hint}{module_lines}",
    );
    set_text(text.reborrow(), &card);
}

/// The game's damage model: the HUD names the same task rate that the simulation consumes.
/// A hold keeps served throughput; unstaffed controls serve zero even with an available task path.
pub(crate) fn drive_rate_line(target: &crate::track::drive::DriveTarget) -> String {
    format!(
        "{} rate {}%",
        Capability::Drive.label(),
        served_percent(target.supply.served_task_rate())
    )
}

/// `1`–`5` crew-bar input for the controlled tank: tap a seat to select it as the swap source, tap
/// a second to command the swap; re-tapping the source (or any seat while a swap is mid-flight)
/// cancels. Selection is client-side state; only the resolved [`CrewSwap`] intent goes on the
/// command — the sim starts (and validates) the actual swap. In [`TankDeviceSet`].
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
    let Some((tank, pending, mut command)) = crate::tank::controlled_one(tank.single_mut()) else {
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
/// foreign) and an out-of-action mark. The selected source is bracketed `[..]`; a pending swap marks both seats
/// `~..~` and shows its countdown.
fn update_crew_bar(
    select: Res<CrewSelect>,
    tank: Query<(Entity, Option<&PendingSwap>), With<Controlled>>,
    seats: Query<(Entity, &CrewStation, &Crewman, Option<&Dead>, &VolumeOf)>,
    mut bar: Query<&mut Text, With<CrewBarText>>,
) {
    let mut text = bar
        .single_mut()
        .expect("crew UI mounts exactly one crew bar for the match");
    let Some((tank, pending)) = crate::tank::controlled_one(tank.single()) else {
        set_text(text.reborrow(), "");
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
            // station is out of action — e.g. "Loader (Commander, out of action)".
            let mut notes: Vec<&str> = Vec::new();
            if crewman.home != *seat {
                notes.push(crewman.home.label());
            }
            if *dead {
                notes.push("out of action");
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
    set_text(text.reborrow(), &format!("{prefix}{}", cells.join("    ")));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's damage model: the mounted panel shows served Drive work for unstaffed
    /// Backup paths and staffed phase holds, rather than recomposing its sources.
    /// Fixture: ten-point source pools and an independent Commander Backup path.
    #[test]
    fn drive_rate_readout_uses_the_consumed_task_rate() {
        use crate::ballistics::ComponentHealth;
        use crate::command::ParsedTankCommand;
        use crate::damage::{ControlStaffing, FunctionRole, GradedGroup, Group, Part};
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(crate::WeaponClock(crate::sim_tick::SimTick::new(0)));
        world.insert_resource(Time::<Fixed>::default());
        let panel = world
            .spawn((StatusPanelText, Text::new(""), Visibility::Visible))
            .id();
        let tank = world
            .spawn((
                Controlled,
                TankCapabilities::new(
                    [(
                        Capability::Drive,
                        vec![Group::Graded(GradedGroup::Backup(vec![
                            (1.0, vec![Part::Driver, Part::Engine]),
                            (0.6, vec![Part::Commander, Part::Transmission]),
                        ]))],
                    )]
                    .into(),
                    ControlStaffing::Any(vec![vec![CrewStation::Driver]]),
                ),
                ParsedTankCommand::hold(),
            ))
            .id();
        let driver = world
            .spawn((
                CrewStation::Driver,
                Crewman {
                    home: CrewStation::Driver,
                },
                VolumeOf(tank),
            ))
            .id();
        world.spawn((
            CrewStation::Commander,
            Crewman {
                home: CrewStation::Commander,
            },
            VolumeOf(tank),
        ));
        for (role, held) in [(FunctionRole::Engine, 5), (FunctionRole::Transmission, 10)] {
            world.spawn((
                role,
                ComponentHealth::holding(10, held * crate::ballistics::POOL_UNITS_PER_HP),
                VolumeOf(tank),
            ));
        }
        for staffed in [false, true] {
            if staffed {
                world.entity_mut(driver).remove::<Dead>();
            } else {
                world.entity_mut(driver).insert(Dead);
            }
            for command in [
                ParsedTankCommand::hold(),
                ParsedTankCommand::try_from(TankCommand {
                    throttle: 1.0,
                    ..default()
                })
                .unwrap(),
            ] {
                world.entity_mut(tank).insert(command);
                world.run_system_once(update_status_panel).unwrap();
                let text = &world.get::<Text>(panel).unwrap().0;
                let expected = if staffed {
                    "Drive rate 60%"
                } else {
                    "Drive rate 0%"
                };
                assert!(text.contains(expected), "{text}");
                assert_eq!(
                    *world.get::<Visibility>(panel).unwrap(),
                    Visibility::Visible
                );
            }
        }
        world.entity_mut(tank).remove::<Controlled>();
        world.run_system_once(update_status_panel).unwrap();
        assert!(world.get::<Text>(panel).unwrap().0.is_empty());
        assert_eq!(*world.get::<Visibility>(panel).unwrap(), Visibility::Hidden);
    }

    /// The game's damage model: the player must see a reduced module's rate rather than READY.
    /// Fixture: 14 of 142 HP, whose held share rounds to ten percent.
    #[test]
    fn a_reduced_module_row_names_its_rate() {
        let health = crate::ballistics::ComponentHealth::holding(
            142,
            14 * crate::ballistics::POOL_UNITS_PER_HP,
        );
        assert_eq!(
            module_status(&health, &crate::damage::repair::ModuleRepair::default()),
            "RATE 10%"
        );
    }

    /// The game's damage model: a module that still serves must not read as out of action, and
    /// one below capacity must not read as whole, since repair and driving both act on that
    /// difference. Fixture: 1 and 335 of 336 HP.
    #[test]
    fn a_rate_names_a_serving_pool_above_zero_and_a_short_one_below_whole() {
        let row = |held: u32| {
            module_status(
                &crate::ballistics::ComponentHealth::holding(
                    336,
                    held * crate::ballistics::POOL_UNITS_PER_HP,
                ),
                &crate::damage::repair::ModuleRepair::default(),
            )
        };
        assert_eq!(row(1), "RATE 1%");
        assert_eq!(row(335), "RATE 99%");
        assert_eq!(served_percent(0.0), 0);
        assert_eq!(served_percent(1.0), 100);
    }

    /// The game's damage model: integer pool units retain a one-unit shortage above f32's
    /// exact integer range, so the player still sees the module needs repair.
    #[test]
    fn one_unit_short_is_never_whole() {
        // Fixture: one pool unit short of an admitted 20,000 HP pool.
        let health = crate::ballistics::ComponentHealth::holding(20_000, 19_999_999);
        let share = crate::damage::module_share(&health);
        assert!(share < 1.0);
        assert_eq!(served_percent(share), 99);
        assert_eq!(
            module_status(&health, &crate::damage::repair::ModuleRepair::default()),
            "RATE 99%"
        );
    }
    const AUTO: FireCycle = FireCycle::Automatic {
        cyclic: crate::sim_tick::TickSpan::of(5),
        belt_size: 150,
        swap: crate::sim_tick::TickSpan::of(224),
        tracer_every: 5,
    };
    const SINGLE: FireCycle = FireCycle::Single {
        reload: crate::sim_tick::TickSpan::of(192),
    };

    /// The crew-gate/no-fire state wins over every timer readout, in BOTH fire modes: a weapon that
    /// can't fire reads `no-fire`, never a (frozen) reload or swap countdown. This is the regression
    /// guard for the dry-belt MG whose swap timer froze on a gun with its crew out of action and showed a permanently
    /// stuck `SWAP` countdown instead of `no-fire`.
    #[test]
    fn no_fire_wins_over_frozen_timers() {
        // Automatic, dry belt, gun crew out of action: the swap timer is frozen — must read `no-fire`.
        assert_eq!(weapon_status(AUTO, true, 2.7, 0, true), "no-fire");
        // Single, mid-reload, fire crew out of action: the reload is frozen — must read `no-fire`.
        assert_eq!(weapon_status(SINGLE, true, 1.4, 0, true), "no-fire");
    }

    /// The game's damage model: a paused gate is unavailable even at zero remaining work.
    /// The player must never read READY or a running countdown for it. Fixture: a pause on
    /// the completion boundary, in each mechanism's load phase.
    #[test]
    fn a_paused_load_is_explicit_even_at_its_completion_boundary() {
        assert_eq!(weapon_status(SINGLE, false, 0.0, 0, true), "LOAD PAUSED");
        assert_eq!(weapon_status(AUTO, false, 0.0, 0, true), "SWAP PAUSED");
    }

    /// The game's damage model: the readout names the fuze mode the next shot fires with, the
    /// crew's pick or the default before any, and only for a round whose fuze lists more than one
    /// mode; a weapon with one such round still shows its line. The crew steers the toggle by it: a
    /// mutant that showed the default after a pick, bracketed a one-mode fuze, or hid the line of a
    /// one-round weapon with a mode to pick would show the crew a setting the round does not carry.
    #[test]
    fn the_readout_names_a_pickable_fuze_mode() {
        let round = |key: &str| WeaponRound::resolve(key).expect("a shipped round");
        let rounds = [round("88mm_APHE_ring"), round("88mm_HE")];
        let (one_mode, two_modes) = (&rounds[0].name, &rounds[1].name);
        assert_eq!(rounds[0].fuze_modes().len(), 1, "fixture: a one-mode fuze");
        let modes = rounds[1].fuze_modes();
        let line = |load: WeaponLoad| round_line("Gun", &rounds, &load);
        assert_eq!(
            line(WeaponLoad {
                loaded: 0,
                selected: 1,
                ..default()
            }),
            Some(format!(
                "Gun round: {one_mode}   next: {two_modes} [{}]",
                modes[usize::from(rounds[1].round.fuze_mode)].name
            )),
        );
        assert_eq!(
            line(WeaponLoad {
                loaded: 1,
                selected: 1,
                fuze_modes: vec![0, 1],
            }),
            Some(format!("Gun round: {two_modes} [{}]", modes[1].name)),
        );
        assert!(
            round_line("Gun", &rounds[1..], &WeaponLoad::default()).is_some(),
            "a one-round weapon with a mode to pick shows its line"
        );
    }

    /// The round readout a player steers the selector by: a weapon with a choice names the loaded
    /// round, and names the selection only while it differs, i.e. while it waits for the next load.
    /// A mutant that showed the selection as loaded would tell the player a round is chambered
    /// that the next shot does not fire; a one-round weapon shows no line at all.
    #[test]
    fn a_weapon_with_a_choice_names_the_loaded_round_and_a_pending_selection() {
        let round = |key: &str| WeaponRound::resolve(key).expect("a shipped round");
        let choice = [round("88mm_AP"), round("88mm_APCR")];
        let (first, second) = (&choice[0].name, &choice[1].name);
        assert_eq!(
            round_line(
                "Gun",
                &choice,
                &WeaponLoad {
                    loaded: 0,
                    selected: 0,
                    fuze_modes: Vec::new(),
                }
            ),
            Some(format!("Gun round: {first}")),
        );
        assert_eq!(
            round_line(
                "Gun",
                &choice,
                &WeaponLoad {
                    loaded: 0,
                    selected: 1,
                    fuze_modes: Vec::new(),
                }
            ),
            Some(format!("Gun round: {first}   next: {second}")),
        );
        assert_eq!(
            round_line(
                "Gun",
                &choice[..1],
                &WeaponLoad {
                    loaded: 0,
                    selected: 0,
                    fuze_modes: Vec::new(),
                }
            ),
            None,
        );
    }
}
