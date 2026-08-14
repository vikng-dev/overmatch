//! The own tank's fire presentation: intent edges present on the local tick, legality reconciles
//! forward.
//!
//! Under unpredicted drive the owner leaves the prediction set (`net::server`), so its `WeaponGate`
//! is plain replicated state written into the live component on arrival, while `shooting::fire` and
//! `shooting::tick_weapon_gate` keep mutating that same component locally. Three consequences, all
//! from that one collision:
//!
//! - the local trigger produces no flash until an arriving snapshot says ready — intent waiting on
//!   a round trip;
//! - a snapshot produced BEFORE the local fire carries the pre-fire gate and re-arms readiness, so
//!   the cosmetic round rate becomes the tick rate;
//! - `net::protocol`'s attestation runs client-side too, so a starved input buffer zeroes the
//!   player's own trigger for a tick he is authoring.
//!
//! # THE GATE IS A LEGALITY REPORT, NEVER PERMISSION TO DRAW
//!
//! [`OwnFirePresentation`] carries the gate the local cadence left last tick and restores it over
//! every arriving snapshot. Presentation readiness is therefore local at every tick after the seed:
//! an arriving deadline cannot delay a flash, and an arriving `ready` cannot produce one. The
//! cadence is not re-derived here — `tick_weapon_gate` and `fire` run on the restored gate, so the
//! presented interval IS the sim's own `60/rpm → duration_ticks` arithmetic, not a copy of it.
//!
//! What the snapshot is used for is the ledger: its `belt_remaining` deltas count the rounds the
//! server actually fired, against the rounds this client presented. Divergence is one-directional
//! by construction (`net::protocol`'s attestation and `damage::requirement_met` can only make the
//! server fire FEWER rounds than the owner presented), and the correction is forward only — an
//! absorbed count, never a retraction. The opposite direction is reported, never acted on.
//!
//! # INTENT COMES FROM WHAT THIS CLIENT AUTHORED, KEYED BY TICK
//!
//! [`AuthoredIntent`] is the client's own copy of the consumables it filed, recorded at the one
//! place input provenance is created (`net::client::stamp_input_tick`) and keyed by the tick the
//! command was authored FOR. The presentation reads that copy instead of the attested command, so
//! `TankCommand::fail_consumables_closed` — an authority rule about values lightyear substituted —
//! can no longer zero the owner's own flash. It is redundancy, not hold-last: a tick this client
//! never authored yields no round, exactly as on the server.
//!
//! The one latch is the edge: a click authored for a tick the sim stepped over is presented by the
//! next presentation tick, the shipped fix Valve names in `basecombatweapon_shared.cpp` ("hold onto
//! the edge trigger" — an edge consumed by the wrong frame is gone forever). The LEVEL is never
//! carried forward: only the entry authored for the current tick states the trigger is held now.
//!
//! # SCOPE
//!
//! Client-side, own tank, `Interpolated` and not `Predicted` — the same observable role
//! `net::recoil_overlay` arms on. In predicted mode nothing here arms and the fire path is
//! bit-identical to what it was: the ledger component is absent, so the gate is untouched and the
//! attested command reaches `shooting::fire` unmodified.
//!
//! Design note: `.agents/scratch/burst-state-fire-stack-map-2026-08-14.md`.

use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Interpolated, LocalTimeline, Predicted};

use super::protocol::{InputBridge, NetTank};
use crate::ballistics::{FireShell, FireShellOrigin};
use crate::command::TankCommand;
use crate::state::GameplaySet;
use crate::tank::{Controlled, WeaponGate, WeaponGateState};

/// Authored ticks retained before the oldest is dropped. A memory bound only: consumption is by
/// exact tick, so an evicted entry is one no presentation tick can still reach.
const AUTHORED_TICKS: usize = 128;

/// Install the own-fire presentation ledger. [`record_own_intent`] is mounted separately, by
/// `net::client`, because it must be chained after the stamp that creates input provenance.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AuthoredIntent>();
    app.add_systems(Update, arm_own_fire_presentation);
    app.add_observer(count_presented_round);
    // Before every `GameplaySet` consumer, after the attested command the bridge writes: the
    // presentation overwrites the owner's consumables, and the gate the cadence left is restored
    // over whatever arrived since.
    app.add_systems(
        FixedUpdate,
        (present_own_intent, hold_presentation_gate)
            .after(InputBridge)
            .before(GameplaySet),
    );
    // After the cadence has run, so the ledger carries what `fire`/`tick_weapon_gate` just left.
    app.add_systems(FixedUpdate, record_presentation_gate.after(GameplaySet));
}

/// One weapon slot's presentation ledger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SlotLedger {
    /// The gate the local cadence left last tick, restored over every arriving snapshot.
    presented_gate: WeaponGateState,
    /// Rounds presented locally since the ledger was seeded.
    presented: u32,
    /// Rounds the arriving snapshots account for, summed from their belt deltas.
    confirmed: u32,
    /// `belt_remaining` of the last arriving snapshot — the base of the next delta.
    seen_belt: u32,
}

impl SlotLedger {
    /// Seed from the arriving gate. The ONE tick the server's copy decides anything.
    fn seeded(arriving: WeaponGateState) -> Self {
        Self {
            presented_gate: arriving,
            presented: 0,
            confirmed: 0,
            seen_belt: arriving.belt_remaining,
        }
    }

    /// Fold one arriving snapshot in. A belt REFILL raises `belt_remaining` and accounts for no
    /// rounds, so the swap boundary undercounts by whatever the server fired between its refill and
    /// this snapshot.
    fn confirm(&mut self, arriving: WeaponGateState) {
        let fired = self.seen_belt.saturating_sub(arriving.belt_remaining);
        self.confirmed = self.confirmed.saturating_add(fired);
        self.seen_belt = arriving.belt_remaining;
    }

    /// Rounds presented that no arriving snapshot accounts for. Includes the rounds still in flight
    /// down the link, so it settles on a burst's phantom count only once the last snapshot lands.
    fn absorbed(self) -> u32 {
        self.presented.saturating_sub(self.confirmed)
    }

    /// The direction that cannot happen for own fire: rounds accounted for that this client never
    /// presented. Reported, never acted on — presenting them now would be a round drawn after the
    /// fact.
    fn overrun(self) -> u32 {
        self.confirmed.saturating_sub(self.presented)
    }
}

/// The owner's fire-presentation ledger: one entry per weapon slot, in `WeaponGate` order.
#[derive(Component, Debug)]
struct OwnFirePresentation {
    slots: Vec<SlotLedger>,
}

/// Arm the own tank once the server stream — not local prediction — owns its gate.
///
/// `Without<Predicted>` makes this inert in predicted mode, where the gate is predicted state with
/// a rollback condition and the sim already owns it.
#[expect(clippy::type_complexity, reason = "one arming predicate, spelled out")]
fn arm_own_fire_presentation(
    tanks: Query<
        (Entity, &WeaponGate),
        (
            With<NetTank>,
            With<Controlled>,
            With<Interpolated>,
            Without<Predicted>,
            Without<OwnFirePresentation>,
            Without<ChildOf>,
        ),
    >,
    mut commands: Commands,
) {
    for (entity, gate) in &tanks {
        info!("net: {entity} own interpolated gate armed with the fire-presentation ledger");
        commands.entity(entity).insert(OwnFirePresentation {
            slots: gate
                .weapons
                .iter()
                .copied()
                .map(SlotLedger::seeded)
                .collect(),
        });
    }
}

/// Restore the presented gate over every arriving snapshot, folding the snapshot into the ledger.
fn hold_presentation_gate(mut roots: Query<(&mut WeaponGate, &mut OwnFirePresentation)>) {
    for (mut gate, mut ledger) in &mut roots {
        // Immutable derefs first: a tick with no arriving snapshot must flag neither component.
        let arrived = ledger.slots.iter().enumerate().any(|(slot, entry)| {
            gate.weapons
                .get(slot)
                .is_some_and(|arriving| *arriving != entry.presented_gate)
        });
        if !arrived {
            continue;
        }
        for (slot, entry) in ledger.slots.iter_mut().enumerate() {
            let Some(arriving) = gate.weapons.get(slot).copied() else {
                continue;
            };
            if arriving == entry.presented_gate {
                continue;
            }
            entry.confirm(arriving);
            if entry.overrun() > 0 {
                warn!(
                    "net: weapon {slot} accounted for {} rounds this client never presented \
                     (presented {}, confirmed {}, absorbed {}) — own legality cannot exceed own \
                     intent",
                    entry.overrun(),
                    entry.presented,
                    entry.confirmed,
                    entry.absorbed(),
                );
            }
            gate.weapons[slot] = entry.presented_gate;
        }
    }
}

/// Carry the gate the cadence just left into the ledger, so the next arriving snapshot is
/// recognizable as one.
fn record_presentation_gate(mut roots: Query<(&WeaponGate, &mut OwnFirePresentation)>) {
    for (gate, mut ledger) in &mut roots {
        let moved = ledger.slots.iter().enumerate().any(|(slot, entry)| {
            gate.weapons
                .get(slot)
                .is_some_and(|current| *current != entry.presented_gate)
        });
        if !moved {
            continue;
        }
        for (slot, entry) in ledger.slots.iter_mut().enumerate() {
            if let Some(current) = gate.weapons.get(slot) {
                entry.presented_gate = *current;
            }
        }
    }
}

/// Count one presented round against its slot. Same observer channel `net::recoil_overlay` excites
/// on, and the same three filters: a reconstructed opponent shot is somebody else's, a sandbox
/// free-fly shot has no shooter, and the query admits only the armed own root.
fn count_presented_round(fire: On<FireShell>, mut ledgers: Query<&mut OwnFirePresentation>) {
    if fire.shot_origin != FireShellOrigin::Local {
        return;
    }
    let Some(source) = fire.shooter else {
        return;
    };
    let Ok(mut ledger) = ledgers.get_mut(source.tank) else {
        return;
    };
    if let Some(entry) = ledger.slots.get_mut(source.weapon) {
        entry.presented = entry.presented.saturating_add(1);
    }
}

/// One tick's consumable intent, as this client authored it.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Consumables {
    primary: bool,
    secondary: bool,
}

/// The client's own record of the consumables it filed, keyed by the tick each was authored FOR.
#[derive(Resource, Default)]
pub(super) struct AuthoredIntent {
    authored: VecDeque<(u32, Consumables)>,
    /// Highest tick already spoken for, so an edge is presented exactly once.
    presented_through: Option<u32>,
}

impl AuthoredIntent {
    fn record(&mut self, for_tick: u32, consumables: Consumables) {
        if self
            .authored
            .back()
            .is_some_and(|(back, _)| *back >= for_tick)
        {
            // A repeated or receding stamp names a tick already filed; the first record for a tick
            // is the authored one.
            return;
        }
        self.authored.push_back((for_tick, consumables));
        if self.authored.len() > AUTHORED_TICKS {
            self.authored.pop_front();
        }
    }

    /// Consume every unspoken-for entry authored at or before `tick`.
    ///
    /// THE EDGE LATCH: an entry whose tick the sim stepped over still yields its click here rather
    /// than being dropped. The LEVEL is not latched — only the entry authored for `tick` states the
    /// trigger is held now, so a gap in what this client authored fails closed instead of holding
    /// the last value.
    fn take(&mut self, tick: u32) -> Consumables {
        let mut taken = Consumables::default();
        let mut newest = None;
        while let Some(&(for_tick, consumables)) = self.authored.front() {
            if for_tick > tick {
                break;
            }
            self.authored.pop_front();
            if self
                .presented_through
                .is_some_and(|through| for_tick <= through)
            {
                continue;
            }
            taken.primary |= consumables.primary;
            newest = Some((for_tick, consumables.secondary));
        }
        taken.secondary = newest.is_some_and(|(for_tick, held)| for_tick == tick && held);
        self.presented_through = Some(tick);
        taken
    }
}

/// Record what this client just filed for its stamped tick. Mounted by `net::client`, chained after
/// `stamp_input_tick` under the same `not(is_in_rollback)` gate as the writers: a replayed tick
/// restores a historical `ActionState` this ledger has already filed.
pub(super) fn record_own_intent(
    mut intent: ResMut<AuthoredIntent>,
    slots: Query<&ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    let Ok(state) = slots.single() else {
        return;
    };
    intent.record(
        state.0.for_tick,
        Consumables {
            primary: state.0.fire_primary,
            secondary: state.0.fire_secondary,
        },
    );
}

/// Overwrite the owner's consumables with what this client authored for the tick being simulated.
fn present_own_intent(
    timeline: Res<LocalTimeline>,
    mut intent: ResMut<AuthoredIntent>,
    mut tanks: Query<&mut TankCommand, With<OwnFirePresentation>>,
) {
    // No armed root: nothing consumes the ledger, so nothing is spent either.
    let Ok(mut command) = tanks.single_mut() else {
        return;
    };
    let authored = intent.take(timeline.tick().0);
    // Assignment, never `|=`: the bridge has already written the authority's copy of this same
    // intent into the command, and taking both would present one click twice.
    command.fire_primary = authored.primary;
    command.fire_secondary = authored.secondary;
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// The Tiger's coax cadence: 750 rpm over the 64 Hz fixed tick is `ceil(0.08 / 0.015625)`.
    const PERIOD_TICKS: u32 = 6;
    const BELT: u32 = 150;

    fn ready(belt: u32) -> WeaponGateState {
        WeaponGateState {
            ready_tick: None,
            paused_at_tick: None,
            belt_remaining: belt,
        }
    }

    fn armed(at: u32, belt: u32) -> WeaponGateState {
        let mut gate = ready(belt);
        gate.arm(at, PERIOD_TICKS);
        gate
    }

    fn world_with_ledger(gate: WeaponGateState) -> (World, Entity) {
        let mut world = World::new();
        let root = world
            .spawn((
                WeaponGate {
                    weapons: vec![gate],
                },
                OwnFirePresentation {
                    slots: vec![SlotLedger::seeded(gate)],
                },
            ))
            .id();
        (world, root)
    }

    fn gate_of(world: &World, root: Entity) -> WeaponGateState {
        world.get::<WeaponGate>(root).expect("gate").weapons[0]
    }

    /// THE PHANTOM RE-FIRE, and the property the mutation check breaks: after the local cadence
    /// arms a deadline, snapshots produced at server ticks BEFORE that fire carry the pre-fire
    /// (ready) gate. Every one of them must leave the presentation gate armed — letting a single
    /// one through makes the owner ready on a tick inside its own cyclic interval, and `fire`
    /// emits a cosmetic round at the TICK rate instead of the cyclic rate.
    ///
    /// Deleting the restore in `hold_presentation_gate` turns the ready count below from 0 into 4.
    #[test]
    fn a_stale_ready_snapshot_never_re_arms_the_presentation_gate() {
        const FIRE_TICK: u32 = 1_000;
        let (mut world, root) = world_with_ledger(ready(BELT));

        // The local cadence fires and arms the cyclic interval, then records it.
        world.get_mut::<WeaponGate>(root).expect("gate").weapons[0] = armed(FIRE_TICK, BELT - 1);
        world
            .run_system_once(record_presentation_gate)
            .expect("record runs");

        let mut ready_ticks = 0;
        for arrival in 1..=(PERIOD_TICKS - 2) {
            // A snapshot produced before the fire: full belt, no deadline.
            world.get_mut::<WeaponGate>(root).expect("gate").weapons[0] = ready(BELT);
            world
                .run_system_once(hold_presentation_gate)
                .expect("hold runs");
            let gate = gate_of(&world, root);
            if gate.is_ready() {
                ready_ticks += 1;
            }
            assert_eq!(
                gate,
                armed(FIRE_TICK, BELT - 1),
                "arrival {arrival}: the presented gate must survive the snapshot",
            );
        }
        assert_eq!(
            ready_ticks, 0,
            "a stale snapshot made the owner ready inside its own cyclic interval",
        );
        // And the cadence still retires the deadline on its own clock, with nothing arriving.
        assert!(
            gate_of(&world, root).deadline_reached(FIRE_TICK + PERIOD_TICKS),
            "the presented deadline matures locally, not on the snapshot",
        );
    }

    /// V1's shape: an arriving deadline the server owns — a belt swap still running there — must
    /// not reach back and stop a presentation the local cadence has already released.
    #[test]
    fn an_arriving_deadline_never_delays_a_presentation_the_cadence_released() {
        let (mut world, root) = world_with_ledger(armed(500, 0));
        // The local swap completes: belt refilled, deadline retired.
        world.get_mut::<WeaponGate>(root).expect("gate").weapons[0] = ready(BELT);
        world
            .run_system_once(record_presentation_gate)
            .expect("record runs");
        // A snapshot from before the server's own swap completed.
        world.get_mut::<WeaponGate>(root).expect("gate").weapons[0] = armed(500, 0);
        world
            .run_system_once(hold_presentation_gate)
            .expect("hold runs");
        assert_eq!(
            gate_of(&world, root),
            ready(BELT),
            "the owner's flash must not wait on the server's copy of the swap",
        );
    }

    /// FORWARD CORRECTION ONLY. Presented rounds the arriving belt does not account for are
    /// absorbed — counted, never un-drawn — and the gate is untouched by the accounting.
    #[test]
    fn rounds_the_server_did_not_fire_are_absorbed_not_retracted() {
        let mut ledger = SlotLedger::seeded(ready(BELT));
        ledger.presented = 5;
        // The server is two rounds behind: three of the five are accounted for.
        ledger.confirm(ready(BELT - 3));
        assert_eq!(ledger.confirmed, 3);
        assert_eq!(
            ledger.absorbed(),
            2,
            "the excess is absorbed, not retracted"
        );
        assert_eq!(ledger.overrun(), 0);
        // A later snapshot catches up; the absorbed count settles to the true phantom count.
        ledger.confirm(ready(BELT - 5));
        assert_eq!(ledger.absorbed(), 0);
    }

    /// The direction that cannot happen for own fire is REPORTED, not compensated: the ledger
    /// records the overrun and presents nothing extra.
    #[test]
    fn the_server_firing_more_than_was_presented_is_reported_not_re_presented() {
        let mut ledger = SlotLedger::seeded(ready(BELT));
        ledger.presented = 1;
        ledger.confirm(ready(BELT - 4));
        assert_eq!(ledger.overrun(), 3);
        assert_eq!(
            ledger.absorbed(),
            0,
            "an overrun is not a negative absorption",
        );
        assert_eq!(
            ledger.presented, 1,
            "no round is manufactured to close the gap",
        );
    }

    /// A belt refill raises `belt_remaining`; it accounts for no rounds and never runs the counter
    /// backward.
    #[test]
    fn a_belt_refill_accounts_for_no_rounds() {
        let mut ledger = SlotLedger::seeded(ready(3));
        ledger.presented = 3;
        ledger.confirm(ready(0));
        assert_eq!(ledger.confirmed, 3);
        ledger.confirm(ready(BELT));
        assert_eq!(ledger.confirmed, 3, "the swap accounts for nothing");
        assert_eq!(ledger.overrun(), 0);
        ledger.confirm(ready(BELT - 2));
        assert_eq!(ledger.confirmed, 5, "the next belt counts from its refill");
    }

    /// THE EDGE LATCH, and the property its mutation check breaks: a click authored for a tick the
    /// sim stepped over is presented by the next presentation tick.
    ///
    /// Requiring an exact tick match in `take` drops the click and this test goes red — the lost
    /// first-shot window.
    #[test]
    fn a_click_authored_for_a_stepped_over_tick_still_presents() {
        let mut intent = AuthoredIntent::default();
        intent.record(
            10,
            Consumables {
                primary: true,
                secondary: false,
            },
        );
        intent.record(
            11,
            Consumables {
                primary: false,
                secondary: false,
            },
        );
        // The sim never ran tick 10.
        let taken = intent.take(11);
        assert!(
            taken.primary,
            "the authored click must survive a tick the sim stepped over",
        );
        // And exactly once.
        assert!(
            !intent.take(12).primary,
            "a consumed click must not present a second time",
        );
    }

    /// THE LEVEL IS NOT LATCHED. Only the entry authored for the tick being simulated states the
    /// trigger is held; a tick this client never authored yields no round, the same refusal
    /// `TankCommand::fail_consumables_closed` makes on the authority — without hold-last.
    #[test]
    fn a_held_trigger_is_never_carried_into_a_tick_this_client_did_not_author() {
        let mut intent = AuthoredIntent::default();
        intent.record(
            20,
            Consumables {
                primary: false,
                secondary: true,
            },
        );
        intent.record(
            22,
            Consumables {
                primary: false,
                secondary: true,
            },
        );
        assert!(intent.take(20).secondary, "the authored tick presents");
        assert!(
            !intent.take(21).secondary,
            "an unauthored tick must fail closed, not hold the last level",
        );
        assert!(intent.take(22).secondary, "and the next authored tick does");
    }

    /// RELEASE STOPS PRESENTATION ON THE LOCAL TICK: the authored level falls with the device, with
    /// no arriving fact between the release and the flash stopping.
    #[test]
    fn releasing_the_trigger_stops_presentation_on_the_authored_tick() {
        let mut intent = AuthoredIntent::default();
        for tick in 30..34 {
            intent.record(
                tick,
                Consumables {
                    primary: false,
                    secondary: tick < 32,
                },
            );
        }
        let held: Vec<bool> = (30..34).map(|tick| intent.take(tick).secondary).collect();
        assert_eq!(held, vec![true, true, false, false]);
    }

    /// The attested command is the authority's copy of the same intent; taking both would present
    /// one click twice.
    #[test]
    fn the_presentation_replaces_the_attested_command_rather_than_joining_it() {
        let mut world = World::new();
        world.insert_resource(LocalTimeline::default());
        world.resource_mut::<LocalTimeline>().apply_delta(40);
        let mut intent = AuthoredIntent::default();
        // Nothing authored for tick 40 — the bridge's copy says fire, this client's record does not.
        intent.record(
            41,
            Consumables {
                primary: true,
                secondary: true,
            },
        );
        world.insert_resource(intent);
        let root = world
            .spawn((
                TankCommand {
                    fire_primary: true,
                    fire_secondary: true,
                    ..default()
                },
                OwnFirePresentation {
                    slots: vec![SlotLedger::default()],
                },
            ))
            .id();
        world
            .run_system_once(present_own_intent)
            .expect("presentation runs");
        let command = world.get::<TankCommand>(root).expect("command");
        assert!(
            !command.fire_primary && !command.fire_secondary,
            "the presentation must replace the attested consumables, not join them",
        );
    }

    /// V3: A LEGALITY RULE MUST NOT STUTTER THE OWNER'S OWN FLASH. `net::protocol`'s bridge runs on
    /// the client too, so an unattested tick reaches `shooting::fire` with the trigger zeroed by
    /// `TankCommand::fail_consumables_closed` — for a round the player IS authoring. The
    /// presentation restores it from this client's own record of that tick.
    ///
    /// Dropping the overwrite in `present_own_intent` leaves the zeroed command in place and this
    /// test goes red — one dropped round in the middle of a held burst.
    #[test]
    fn an_unattested_tick_cannot_zero_a_trigger_this_client_authored() {
        let mut world = World::new();
        world.insert_resource(LocalTimeline::default());
        world.resource_mut::<LocalTimeline>().apply_delta(60);
        let mut intent = AuthoredIntent::default();
        intent.record(
            60,
            Consumables {
                primary: false,
                secondary: true,
            },
        );
        world.insert_resource(intent);
        // What the bridge left: the attestation failed closed on a tick this client authored.
        let mut attested = TankCommand {
            fire_secondary: true,
            for_tick: 59,
            ..default()
        };
        attested.fail_consumables_closed();
        assert!(!attested.fire_secondary, "the bridge zeroed the trigger");
        let root = world
            .spawn((
                attested,
                OwnFirePresentation {
                    slots: vec![SlotLedger::default()],
                },
            ))
            .id();
        world
            .run_system_once(present_own_intent)
            .expect("presentation runs");
        assert!(
            world
                .get::<TankCommand>(root)
                .expect("command")
                .fire_secondary,
            "the owner's own flash must survive an unattested input tick",
        );
    }

    /// PREDICTED MODE IS INERT: the ledger arms only where the server stream owns the gate, so the
    /// predicted fire path keeps reading its own predicted `WeaponGate` exactly as before.
    #[test]
    fn only_the_own_interpolated_gate_arms() {
        let mut app = App::new();
        let gate = || WeaponGate {
            weapons: vec![ready(BELT)],
        };
        let own = app
            .world_mut()
            .spawn((NetTank, Controlled, Interpolated, gate()))
            .id();
        let predicted = app
            .world_mut()
            .spawn((NetTank, Controlled, Predicted, gate()))
            .id();
        let opponent = app.world_mut().spawn((NetTank, Interpolated, gate())).id();

        app.world_mut()
            .run_system_once(arm_own_fire_presentation)
            .expect("arming runs");

        assert!(app.world().get::<OwnFirePresentation>(own).is_some());
        assert!(app.world().get::<OwnFirePresentation>(predicted).is_none());
        assert!(app.world().get::<OwnFirePresentation>(opponent).is_none());
    }

    /// The seed is the ONE tick the arriving gate decides anything: the ledger starts from the
    /// belt the server reports, so the first delta is measured from a real base.
    #[test]
    fn the_ledger_seeds_from_the_arriving_belt() {
        let mut app = App::new();
        let own = app
            .world_mut()
            .spawn((
                NetTank,
                Controlled,
                Interpolated,
                WeaponGate {
                    weapons: vec![ready(BELT - 7), ready(0)],
                },
            ))
            .id();
        app.world_mut()
            .run_system_once(arm_own_fire_presentation)
            .expect("arming runs");
        let ledger = app
            .world()
            .get::<OwnFirePresentation>(own)
            .expect("armed ledger");
        assert_eq!(ledger.slots.len(), 2, "one entry per weapon slot");
        assert_eq!(ledger.slots[0].seen_belt, BELT - 7);
        assert_eq!(ledger.slots[0].absorbed(), 0);
    }
}
