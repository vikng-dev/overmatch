//! Own weapon-view cadence and authoritative fire reconstruction for the game's damage model.
//!
//! [`OwnFirePresentation`] seeds the view gate from the owner's replicated `WeaponGate`, then
//! restores what the local cadence left over each arriving snapshot. `shooting::fire` and
//! `shooting::tick_weapon_gate` therefore use the parsed `FireCycle` on the authored-input clock.
//! [`AuthoredIntent`] retains tick-keyed consumables before authority attestation can clear them;
//! a skipped tick carries its click edge forward, never its held level.
//!
//! Replicated belt deltas and ready-to-armed edges confirm local consumptions. A component change
//! tick inside the arrival bracket proves a replication write, even when the arriving value equals
//! the view gate. Replicon's default mutable write deserializes through `&mut *component`
//! (bevy_replicon 0.45.5, `shared/replication/registry/receive_fns.rs::default_write`).
//! Unconfirmed local reloads may refresh from the last arrived gate; that changes only the view.
//!
//! Every released round requires its announced `ShotId` and original fire data. Belt counts do
//! not identify a shot or its path and never author a trajectory. Own and remote announcements
//! share `net::client`'s cursor release and the keyed seam here. A lost automatic announcement
//! omits its cosmetic round under ADR-0021; later announcements remain eligible for reconstruction.
//!
//! Scope: the controlled `Interpolated` root. Design and capture evidence:
//! `.agents/docs/design/fire-presentation.md`.

use core::time::Duration;
use std::collections::VecDeque;

use bevy::ecs::change_detection::Tick as ChangeTick;
use bevy::ecs::query::QuerySingleError;
use bevy::prelude::*;
use lightyear::core::tick::{Tick, TickDuration};
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{Interpolated, LocalTimeline, LocalTimelineSync, PingManager};

use super::protocol::{InputBridge, NetTank};
use super::sync_margin::ArrivalDelay;
use crate::ballistics::{FireShell, FireShellOrigin, MAX_COSMETIC_CATCH_UP_TICKS, ShotSource};
use crate::command::TankCommand;
use crate::state::GameplaySet;
use crate::tank::{Controlled, WeaponGate, WeaponGateState, WeaponLoads};

/// Resolution: cap tick-keyed authored history to bound memory.
const AUTHORED_TICKS: usize = 128;

/// Resolution: cap pending reload-view history to bound memory, retaining its newest causes.
const QUEUE_CAP: usize = 32;

/// Install the own-fire presentation ledger. [`record_own_intent`] is mounted separately, by
/// `net::client`, because it must be chained after the stamp that creates input provenance.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AuthoredIntent>();
    app.init_resource::<OwnFireDiag>();
    app.add_systems(Update, arm_own_fire_presentation);
    // The change-tick arrival bracket: between the post-loop stamp and the next frame's pre-loop
    // fold, the only `WeaponGate` writers are the replication apply (`PreUpdate`) and the heal
    // (which restores an already-folded value), so a moved change tick at the fold IS an arrival.
    app.add_systems(
        RunFixedMainLoop,
        (
            fold_arrivals.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
            stamp_gate_write_mark.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
        ),
    );
    app.add_systems(Update, heal_refused_presentations);
    // Before every `GameplaySet` consumer, after the attested command the bridge writes: the
    // presentation overwrites the owner's consumables, and the gate the cadence left is restored
    // over whatever arrived since. Before the Battle's hold, so a held tank presents no round.
    app.add_systems(
        FixedUpdate,
        (present_own_intent, hold_presentation_gate)
            .after(InputBridge)
            .before(super::battle::HoldCommands)
            .before(GameplaySet),
    );
    // After the cadence has run, so the ledger carries what `fire`/`tick_weapon_gate` just left.
    app.add_systems(FixedUpdate, record_presentation_gate.after(GameplaySet));
}

/// One weapon slot's presentation ledger.
#[derive(Debug, Clone, Default, PartialEq)]
struct SlotLedger {
    /// The gate the local cadence left last tick, restored over every arriving snapshot.
    presented_gate: WeaponGateState,
    /// The last state that provably ARRIVED (a replication write) — the base of the next
    /// consumption fold and the refusal heal's restore source. Never the presented copy.
    last_arrived: WeaponGateState,
    /// Belt count of the last arriving snapshot, the base of the next consumption delta.
    seen_belt: u32,
    /// Local cadence consumptions awaiting server counts, keyed by their fixed tick. FIFO settles
    /// counts for the reload view; these entries never identify authority shots.
    pending_local: VecDeque<u32>,
}

impl SlotLedger {
    /// Cadence and consumption accounting start from the same arrived state.
    fn seeded(arriving: WeaponGateState) -> Self {
        Self {
            presented_gate: arriving,
            last_arrived: arriving,
            seen_belt: arriving.belt_remaining,
            ..Self::default()
        }
    }

    /// Fold one PROVEN arrival in. Consumption is `max(belt delta, arm edge)`: an `Automatic`'s
    /// round moves both signals at once, a `Single`'s consumption draws no belt and shows only as
    /// the `None → Some` arm edge (`shooting::fire` is the only writer that arms a ready gate;
    /// `Some → Some` is cyclic/pause bookkeeping and `Some → None` is retirement, so neither is a
    /// consumption). A belt REFILL raises `belt_remaining` and accounts for no rounds, so the
    /// swap boundary undercounts by whatever the server fired between its refill and this
    /// snapshot. Returns the rounds this arrival accounted for.
    fn confirm(&mut self, arriving: WeaponGateState) -> u32 {
        let belt_fired = self.seen_belt.saturating_sub(arriving.belt_remaining);
        let arm_edge =
            u32::from(self.last_arrived.ready_tick().is_none() && arriving.ready_tick().is_some());
        let fired = belt_fired.max(arm_edge);
        self.seen_belt = arriving.belt_remaining;
        self.last_arrived = arriving;
        // Settle local reload-view counts oldest-first, without assigning shot identities.
        for _ in 0..fired {
            self.pending_local.pop_front();
        }
        fired
    }
}

/// The owner's fire-presentation ledger: one entry per weapon slot, in `WeaponGate` order.
#[derive(Component, Debug)]
struct OwnFirePresentation {
    slots: Vec<SlotLedger>,
    /// The gate component's change tick at the end of the last fixed-loop pass — the arrival
    /// bracket's reference point (module doc).
    gate_write_mark: Option<ChangeTick>,
}

impl OwnFirePresentation {
    fn with_slots(slots: Vec<SlotLedger>) -> Self {
        Self {
            slots,
            gate_write_mark: None,
        }
    }
}

/// Counts of local reload-view refreshes without server confirmation.
#[derive(Resource, Default, Debug)]
struct OwnFireDiag {
    /// Presented reloads restored after the server stayed silent past the refusal deadline.
    healed: u64,
}

/// One released shot's wire/state facts — everything presentation needs except its age, which is
/// the seam's to measure.
#[derive(Clone, Copy)]
pub(super) struct ReleasedShot {
    pub(super) birth: Vec3,
    pub(super) origin: Vec3,
    pub(super) direction: Dir3,
    /// The fired round, bound from the shooter's weapon data.
    pub(super) round: crate::rounds::Round,
    pub(super) tracer: bool,
    pub(super) mechanism: crate::spec::FireMechanism,
    pub(super) shooter: ShotSource,
    pub(super) shot: crate::ShotId,
}

/// The game's damage-model presentation: one announced round, muzzle dressing and recoil kick.
/// Age is measured on the releasing cursor, not the leading local timeline, so a crossing release
/// starts at the muzzle. `None` names a release beyond [`MAX_COSMETIC_CATCH_UP_TICKS`]; the caller
/// records its omission.
pub(super) fn present_released_shot(
    released_tick: Tick,
    (cursor_tick, overstep): (Tick, f64),
    shot: ReleasedShot,
    recoil: &mut super::client::PendingRecoilKicks,
    commands: &mut Commands,
) -> Option<u32> {
    // A release happens at or after the crossing, so the age is non-negative by construction; the
    // clamp mirrors `fire_catch_up_ticks`' don't-rewind rule for a malformed tick.
    let age = (f64::from(cursor_tick - released_tick) + overstep).max(0.0);
    // Whole ticks feed the fixed-tick catch-up march; the dropped fraction is under one tick of
    // shell flight.
    let catch_up_ticks = age as u32;
    if catch_up_ticks > MAX_COSMETIC_CATCH_UP_TICKS {
        return None;
    }
    present_shot_at_age(catch_up_ticks, shot, recoil, commands);
    Some(catch_up_ticks)
}

/// The trigger body the seam and the pre-sync arrival fallback share: one `FireShell` plus one
/// queued barrel kick per released shot. Age policy lives at the call sites — this only writes it.
pub(super) fn present_shot_at_age(
    catch_up_ticks: u32,
    shot: ReleasedShot,
    recoil: &mut super::client::PendingRecoilKicks,
    commands: &mut Commands,
) {
    commands.trigger(FireShell {
        shot_origin: FireShellOrigin::Reconstructed,
        catch_up_ticks,
        ..FireShell::of_round(
            &shot.round,
            shot.birth,
            shot.origin,
            shot.direction,
            shot.mechanism,
            shot.tracer,
            Some(shot.shooter),
            Some(shot.shot),
        )
    });
    recoil.push(shot.shooter.tank, shot.shooter.weapon);
}

/// Malformed outside input: one disclosed weapon record must name every spawn-built slot.
#[derive(Debug)]
enum WeaponRecordFault {
    SlotCount {
        expected: usize,
        gates: usize,
        loads: usize,
    },
}

impl std::fmt::Display for WeaponRecordFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SlotCount {
                expected,
                gates,
                loads,
            } => write!(
                f,
                "expected {expected} weapon slots, received {gates} gates and {loads} loads"
            ),
        }
    }
}

fn check_weapon_record(
    gate: &WeaponGate,
    loads: &WeaponLoads,
    expected: usize,
) -> Result<(), WeaponRecordFault> {
    if gate.weapons.len() == expected && loads.weapons.len() == expected {
        Ok(())
    } else {
        Err(WeaponRecordFault::SlotCount {
            expected,
            gates: gate.weapons.len(),
            loads: loads.weapons.len(),
        })
    }
}

/// Arm the own tank once the server stream owns its gate, each slot's load as it arrived. Both
/// arrive together: one spawn inserts them and one `CombatDisclosure` scope discloses them.
fn arm_own_fire_presentation(
    tanks: Query<
        (Entity, Ref<WeaponGate>, Ref<WeaponLoads>),
        (
            With<NetTank>,
            With<Controlled>,
            With<Interpolated>,
            Without<OwnFirePresentation>,
            Without<ChildOf>,
        ),
    >,
    mut commands: Commands,
) {
    for (entity, gate, loads) in &tanks {
        if let Err(fault) = check_weapon_record(&gate, &loads, gate.weapons.len()) {
            if gate.is_changed() || loads.is_changed() {
                warn!("net: {entity} refused malformed own-fire state: {fault}");
            }
            continue;
        }
        info!("net: {entity} own interpolated gate armed with the fire-presentation ledger");
        let slots = gate
            .weapons
            .iter()
            .copied()
            .map(SlotLedger::seeded)
            .collect();
        commands
            .entity(entity)
            .insert(OwnFirePresentation::with_slots(slots));
    }
}

/// Close the arrival bracket: remember the gate's change tick after the last fixed write of the
/// frame, so the next pre-loop fold can tell replication writes from local ones.
fn stamp_gate_write_mark(mut roots: Query<(Ref<WeaponGate>, &mut OwnFirePresentation)>) {
    for (gate, mut ledger) in &mut roots {
        ledger.gate_write_mark = Some(gate.last_changed());
    }
}

/// Fold proven gate arrivals into the ledger, ahead of this frame's fixed ticks. Value-blind
/// arrival proof (module doc): a change tick moved since [`stamp_gate_write_mark`] is a
/// replication write even when the value equals the presented copy; an unmoved one is silence —
/// the signal [`heal_refused_presentations`] measures — never a fold.
fn fold_arrivals(
    arrival: Option<Res<ArrivalDelay>>,
    timeline: Res<LocalTimeline>,
    mut trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    mut roots: Query<(
        Entity,
        Ref<WeaponGate>,
        Ref<WeaponLoads>,
        &mut OwnFirePresentation,
        Option<&crate::CombatantId>,
    )>,
) {
    let revealed_tick = arrival
        .as_ref()
        .and_then(|estimator| estimator.newest_remote())
        .map(|tick| tick.0);
    for (entity, gate, loads, mut ledger, combatant) in &mut roots {
        if let Err(fault) = check_weapon_record(&gate, &loads, ledger.slots.len()) {
            if ledger.gate_write_mark != Some(gate.last_changed()) || loads.is_changed() {
                warn!("net: {entity} refused malformed own-fire state: {fault}");
            }
            ledger.gate_write_mark = Some(gate.last_changed());
            continue;
        }
        if ledger.gate_write_mark != Some(gate.last_changed()) {
            // An unset mark (fresh arm) folds against the seed: zero rounds, unless a replication
            // write landed between the arm and this fold, which confirms as any other.
            ledger.gate_write_mark = Some(gate.last_changed());
            for (weapon, (entry, &arriving)) in
                ledger.slots.iter_mut().zip(&gate.weapons).enumerate()
            {
                if arriving == entry.last_arrived {
                    continue;
                }
                let fired = entry.confirm(arriving);
                if fired > 0 {
                    crate::shot_trace::record_global(
                        &mut trace,
                        "belt_rx",
                        timeline.tick().0,
                        || {
                            serde_json::json!({
                                "c": combatant.map(|id| id.0), "w": weapon,
                                "rt": revealed_tick, "fired": fired,
                                "belt": arriving.belt_remaining,
                            })
                        },
                    );
                }
            }
        }
    }
}

/// Restore the presented gate over every arriving snapshot. Consumption accounting lives in
/// [`fold_arrivals`]; this is the presentation-side restore only.
fn hold_presentation_gate(mut roots: Query<(&mut WeaponGate, &OwnFirePresentation)>) {
    for (mut gate, ledger) in &mut roots {
        // Malformed arrivals are visibly refused by fold_arrivals; never restore a partial record.
        if gate.weapons.len() != ledger.slots.len() {
            continue;
        }
        // Immutable deref first: a tick with no arriving snapshot must not flag the component.
        let arrived = ledger
            .slots
            .iter()
            .zip(&gate.weapons)
            .any(|(entry, arriving)| *arriving != entry.presented_gate);
        if !arrived {
            continue;
        }
        for (entry, arriving) in ledger.slots.iter().zip(&mut gate.weapons) {
            *arriving = entry.presented_gate;
        }
    }
}

/// Carry the gate the cadence just left into the ledger, so the next arriving snapshot is
/// recognizable as one — and file each local consumption (belt draw or ready→armed edge; the
/// hold restored `presented_gate` at tick start, so any delta here is the cadence's own work)
/// for the refusal deadline to measure.
fn record_presentation_gate(
    timeline: Res<LocalTimeline>,
    mut roots: Query<(&WeaponGate, &mut OwnFirePresentation)>,
) {
    let now = timeline.tick().0;
    for (gate, mut ledger) in &mut roots {
        // Malformed arrivals are visibly refused by fold_arrivals; never record partial cadence.
        if gate.weapons.len() != ledger.slots.len() {
            continue;
        }
        let moved = ledger
            .slots
            .iter()
            .zip(&gate.weapons)
            .any(|(entry, current)| *current != entry.presented_gate);
        if !moved {
            continue;
        }
        for (entry, current) in ledger.slots.iter_mut().zip(&gate.weapons) {
            let consumed = entry
                .presented_gate
                .belt_remaining
                .saturating_sub(current.belt_remaining)
                .max(u32::from(
                    entry.presented_gate.ready_tick().is_none() && current.ready_tick().is_some(),
                ));
            for _ in 0..consumed {
                entry.pending_local.push_back(now);
                if entry.pending_local.len() > QUEUE_CAP {
                    entry.pending_local.pop_front();
                }
            }
            entry.presented_gate = *current;
        }
    }
}

/// Reload-view refresh window in remote ticks: one RTT for redundant inputs, plus downlink
/// spread and a send interval. This sizes a view refresh, never proves announcement loss or refusal;
/// a later authority consumption and its announcement are still accepted.
fn refusal_wait_ticks(rtt: Duration, spread: Duration, tick: Duration) -> u32 {
    ((rtt.as_secs_f64() + spread.as_secs_f64()) / tick.as_secs_f64()).ceil() as u32 + 1
}

/// Refresh an unconfirmed local reload from the last arrived gate after the link-sized window.
/// This presents no round and cannot suppress a later authority announcement or confirmation.
fn heal_refused_presentations(
    tick: Res<TickDuration>,
    arrival: Option<Res<ArrivalDelay>>,
    pings: Query<&PingManager>,
    mut roots: Query<(Entity, &mut WeaponGate, &mut OwnFirePresentation)>,
    mut diag: ResMut<OwnFireDiag>,
) {
    let Some(arrival) = arrival else {
        return;
    };
    let Some(newest_remote) = arrival.newest_remote() else {
        return;
    };
    let pings = match pings.single() {
        Ok(pings) => pings,
        // No link measures a round trip: before a ticket, or after the one link is gone.
        Err(QuerySingleError::NoEntities(_)) => return,
        Err(error @ QuerySingleError::MultipleEntities(_)) => {
            panic!(
                "at most one client link exists (`spawn_matched_link` refuses a second): {error}"
            )
        }
    };
    let wait = refusal_wait_ticks(pings.rtt(), arrival.stats.spread(), tick.0);
    for (entity, mut gate, mut ledger) in &mut roots {
        for (slot, entry) in ledger.slots.iter_mut().enumerate() {
            let Some(&consumed_tick) = entry.pending_local.front() else {
                continue;
            };
            if (newest_remote - Tick(consumed_tick)) <= wait as i32 {
                continue;
            }
            entry.pending_local.pop_front();
            entry.presented_gate = entry.last_arrived;
            if let Some(state) = gate.weapons.get_mut(slot) {
                *state = entry.last_arrived;
            }
            diag.healed += 1;
            warn!(
                "net: {entity} weapon {slot} round consumed locally at tick {consumed_tick} was \
                 not confirmed by state through tick {} (wait {wait}t) — \
                 reload view refreshed from the last arrived gate (healed {})",
                newest_remote.0, diag.healed,
            );
        }
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
    #[cfg(test)]
    pub(super) fn test_with_authored(for_tick: u32) -> Self {
        let mut intent = Self::default();
        intent.record(
            for_tick,
            Consumables {
                primary: true,
                secondary: false,
            },
        );
        intent
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.authored.is_empty() && self.presented_through.is_none()
    }

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
/// `stamp_input_tick` so the copy is keyed by the tick the stamp names.
///
/// Only once the local timeline is synced: lightyear's `buffer_action_state` files nothing before
/// that (it takes `SyncedLocalTimeline`), so a pre-sync click never reaches the wire, and recording
/// it would present a round locally that the authority never heard of.
pub(super) fn record_own_intent(
    sync: Option<Res<LocalTimelineSync>>,
    mut intent: ResMut<AuthoredIntent>,
    slots: Query<&ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    if !sync.is_some_and(|sync| sync.is_synced()) {
        return;
    }
    // The input slot is the controlled tank's (`net::client::claim_input_slot`): none before it
    // arrives, never two.
    let Some(state) = crate::tank::controlled_one(slots.single()) else {
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
    // No armed root: nothing consumes the ledger, so nothing is spent either. The ledger arms only
    // the controlled tank, so never two.
    let Some(mut command) = crate::tank::controlled_one(tanks.single_mut()) else {
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

    /// Fixture cadence and belt: 750 rpm on a 64 Hz clock gives `ceil(0.08 / 0.015625)` ticks.
    const PERIOD_TICKS: u32 = 6;
    const PERIOD: crate::sim_tick::TickSpan = crate::sim_tick::TickSpan::of(PERIOD_TICKS);
    const BELT: u32 = 150;

    fn ready(belt: u32) -> WeaponGateState {
        WeaponGateState {
            timing: crate::tank::WeaponTiming::Ready,
            belt_remaining: belt,
        }
    }

    fn armed(at: u32, belt: u32) -> WeaponGateState {
        let mut gate = ready(belt);
        gate.arm(crate::sim_tick::SimTick::new(at), PERIOD);
        gate
    }

    fn world_with_ledger(gate: WeaponGateState) -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(LocalTimeline::default());
        let root = world
            .spawn((
                WeaponGate {
                    weapons: vec![gate],
                },
                // The loads arrive with the gate (one spawn, one disclosure scope).
                crate::tank::WeaponLoads {
                    weapons: vec![crate::tank::WeaponLoad {
                        loaded: 0,
                        selected: 0,
                        fuze_modes: Vec::new(),
                    }],
                },
                OwnFirePresentation::with_slots(vec![SlotLedger::seeded(gate)]),
            ))
            .id();
        (world, root)
    }

    /// The game's damage model: a malformed peer record cannot confirm a consumption.
    /// The reload view relies on retaining the last complete arrival.
    #[test]
    fn a_missing_load_refuses_the_whole_arrival_without_inventing_round_zero() {
        let (mut world, root) = world_with_ledger(ready(BELT));
        let before = world
            .get::<OwnFirePresentation>(root)
            .unwrap()
            .slots
            .clone();
        world.get_mut::<WeaponLoads>(root).unwrap().weapons.clear();
        world.get_mut::<WeaponGate>(root).unwrap().weapons[0] = ready(BELT - 1);
        world.run_system_once(fold_arrivals).expect("fold runs");
        assert_eq!(
            world.get::<OwnFirePresentation>(root).unwrap().slots,
            before
        );
        assert!(
            check_weapon_record(
                world.get::<WeaponGate>(root).unwrap(),
                world.get::<WeaponLoads>(root).unwrap(),
                1
            )
            .is_err()
        );
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
    ///
    /// The restore holds in the other direction too: an arriving deadline the server owns — a belt
    /// swap still running there — must not reach back and stop a presentation the local cadence
    /// has already released.
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
            gate_of(&world, root)
                .deadline_reached(crate::sim_tick::SimTick::new(FIRE_TICK + PERIOD_TICKS)),
            "the presented deadline matures locally, not on the snapshot",
        );

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

    /// The reload view must not count a refill as a server consumption.
    #[test]
    fn a_belt_refill_accounts_for_no_rounds() {
        let mut ledger = SlotLedger::seeded(ready(3));
        assert_eq!(ledger.confirm(ready(0)), 3);
        assert_eq!(ledger.confirm(ready(BELT)), 0, "a refill is no consumption");
        assert_eq!(
            ledger.confirm(ready(BELT - 2)),
            2,
            "the next belt counts from its refill"
        );
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
    /// trigger is held; a tick this client never authored — or one the sim stepped over — yields
    /// no round, the same refusal `TankCommand::fail_consumables_closed` makes on the authority —
    /// without hold-last. Release stops presentation on the authored tick, with no arriving fact
    /// between the release and the flash stopping.
    #[test]
    fn a_held_trigger_is_never_carried_into_a_tick_this_client_did_not_author() {
        let held = |secondary| Consumables {
            primary: false,
            secondary,
        };
        let mut intent = AuthoredIntent::default();
        intent.record(20, held(true));
        intent.record(22, held(true));
        assert!(intent.take(20).secondary, "the authored tick presents");
        assert!(
            !intent.take(21).secondary,
            "an unauthored tick must fail closed, not hold the last level",
        );
        assert!(intent.take(22).secondary, "and the next authored tick does");

        let mut intent = AuthoredIntent::default();
        intent.record(20, held(true));
        assert!(
            !intent.take(21).secondary,
            "a held level authored for a stepped-over tick must not carry into the next one",
        );

        let mut intent = AuthoredIntent::default();
        for tick in 30..34 {
            intent.record(tick, held(tick < 32));
        }
        let presented: Vec<bool> = (30..34).map(|tick| intent.take(tick).secondary).collect();
        assert_eq!(
            presented,
            vec![true, true, false, false],
            "release stops presentation on the authored tick",
        );
    }

    /// NOTHING FILED, NOTHING RECORDED: before the local timeline syncs, lightyear buffers and sends
    /// no input, so a click then must not enter the own-intent ledger — it would present a round the
    /// authority never received. Once synced, the same click records. Dropping the sync gate reds
    /// the first assertion.
    #[test]
    fn a_pre_sync_click_is_not_recorded() {
        let mut world = World::new();
        world.init_resource::<AuthoredIntent>();
        world.init_resource::<LocalTimelineSync>();
        world.spawn((
            ActionState(TankCommand {
                fire_primary: true,
                for_tick: 5,
                ..default()
            }),
            InputMarker::<TankCommand>::default(),
        ));
        world
            .run_system_once(record_own_intent)
            .expect("recorder runs");
        assert!(
            world.resource::<AuthoredIntent>().authored.is_empty(),
            "a pre-sync click is never filed, so it must not be recorded",
        );
        world.resource_mut::<LocalTimelineSync>().set_synced(true);
        world
            .run_system_once(record_own_intent)
            .expect("recorder runs");
        assert_eq!(world.resource::<AuthoredIntent>().authored.len(), 1);
    }

    /// V3: A LEGALITY RULE MUST NOT STUTTER THE OWNER'S OWN FLASH. `net::protocol`'s bridge runs on
    /// the client too, so an unattested tick reaches `shooting::fire` with the trigger zeroed by
    /// `TankCommand::fail_consumables_closed` — for a round the player IS authoring. The
    /// presentation restores it from this client's own record of that tick.
    ///
    /// Dropping the overwrite in `present_own_intent` leaves the zeroed command in place and this
    /// test goes red — one dropped round in the middle of a held burst. The overwrite is an
    /// assignment both ways: the attested command is the authority's copy of the same intent, so
    /// joining it (`|=`) would present one click twice.
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
                OwnFirePresentation::with_slots(vec![SlotLedger::default()]),
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

        // Tick 61: nothing authored — the bridge's copy says fire, this client's record does not.
        world.resource_mut::<LocalTimeline>().apply_delta(1);
        *world.get_mut::<TankCommand>(root).expect("command") = TankCommand {
            fire_primary: true,
            fire_secondary: true,
            ..default()
        };
        world
            .run_system_once(present_own_intent)
            .expect("presentation runs");
        let command = world.get::<TankCommand>(root).expect("command");
        assert!(
            !command.fire_primary && !command.fire_secondary,
            "the presentation must replace the attested consumables, not join them",
        );
    }

    /// In the game's damage model, a partial disclosed load list has no valid seed for the
    /// own presentation. Refusing it prevents an invented round choice on the first consumption.
    #[test]
    fn a_partial_weapon_record_cannot_arm_the_ledger() {
        let mut world = World::new();
        let root = world
            .spawn((
                NetTank,
                Controlled,
                Interpolated,
                WeaponGate {
                    weapons: vec![ready(BELT)],
                },
                WeaponLoads {
                    weapons: Vec::new(),
                },
            ))
            .id();
        world
            .run_system_once(arm_own_fire_presentation)
            .expect("arming runs");
        assert!(world.get::<OwnFirePresentation>(root).is_none());
    }

    /// The ledger arms only on the own interpolated gate — never an opponent's, whose gate the
    /// presentation restore would freeze. The seed is the ONE tick the arriving gate decides
    /// anything: the ledger starts from the belt the server reports, so the first delta is
    /// measured from a real base.
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
                crate::tank::WeaponLoads {
                    weapons: vec![
                        crate::tank::WeaponLoad {
                            loaded: 1,
                            selected: 1,
                            fuze_modes: Vec::new(),
                        },
                        crate::tank::WeaponLoad {
                            loaded: 0,
                            selected: 0,
                            fuze_modes: Vec::new(),
                        },
                    ],
                },
            ))
            .id();
        let opponent = app
            .world_mut()
            .spawn((
                NetTank,
                Interpolated,
                WeaponGate {
                    weapons: vec![ready(BELT)],
                },
            ))
            .id();
        app.world_mut()
            .run_system_once(arm_own_fire_presentation)
            .expect("arming runs");
        assert!(
            app.world().get::<OwnFirePresentation>(opponent).is_none(),
            "an opponent's gate is never armed"
        );
        let ledger = app
            .world()
            .get::<OwnFirePresentation>(own)
            .expect("armed ledger");
        assert_eq!(ledger.slots.len(), 2, "one entry per weapon slot");
        assert_eq!(ledger.slots[0].seen_belt, BELT - 7);
        assert!(
            ledger
                .slots
                .iter()
                .all(|slot| slot.pending_local.is_empty())
        );
    }

    /// The reload view confirms each consumption once: beltless weapons use the arm edge,
    /// automatic weapons move both signals, and cyclic/retirement transitions consume nothing.
    #[test]
    fn consumption_is_the_belt_delta_or_the_arm_edge_never_their_sum() {
        let mut slot = SlotLedger::seeded(ready(BELT));
        slot.pending_local.push_back(500);
        assert_eq!(
            slot.confirm(armed(500, BELT)),
            1,
            "a Single's consumption IS the None→Some arm edge",
        );
        assert!(
            slot.pending_local.is_empty(),
            "the confirmation proves the oldest awaiting local round",
        );
        assert_eq!(slot.confirm(armed(506, BELT)), 0, "cyclic bookkeeping");
        assert_eq!(slot.confirm(ready(BELT)), 0, "retirement");
        assert_eq!(
            slot.confirm(armed(512, BELT - 1)),
            1,
            "an Automatic's round moves both signals at once — max, not sum",
        );
    }

    /// THE CHANGE-TICK BRACKET, both directions. Silence (a local write, then no replication
    /// write since the stamp) must fold NOTHING even though the live value differs from
    /// `last_arrived` — folding it would count this client's own arm as server-confirmed and the
    /// refusal heal could never fire (an unconditional fold reds the first half). A write with a
    /// value EQUAL to the presented copy (the fused happy path) must fold — a value diff as the
    /// arrival detector is blind to it and the cannon's consumption becomes invisible (reds the
    /// second half).
    #[test]
    fn only_a_replication_window_write_folds_and_an_equal_value_write_does() {
        const FIRE_TICK: u32 = 1_000;
        let (mut world, root) = world_with_ledger(ready(BELT));

        // The local cadence arms (a fixed-loop write), and the post-loop stamp closes the bracket.
        world.get_mut::<WeaponGate>(root).expect("gate").weapons[0] = armed(FIRE_TICK, BELT);
        world
            .run_system_once(record_presentation_gate)
            .expect("record runs");
        world
            .run_system_once(stamp_gate_write_mark)
            .expect("stamp runs");

        // No replication write: fold must read silence, not the differing live value.
        world.run_system_once(fold_arrivals).expect("fold runs");
        {
            let ledger = world.get::<OwnFirePresentation>(root).expect("ledger");
            assert_eq!(
                ledger.slots[0].last_arrived,
                ready(BELT),
                "a local write is not an arrival",
            );
            assert_eq!(
                ledger.slots[0].pending_local.len(),
                1,
                "the local consumption stays pending through the silence",
            );
        }

        // The replication window writes the SAME armed value the presentation already carries —
        // constructed fresh, as `deserialize_in_place` writes a decoded value, not a read-back.
        world.get_mut::<WeaponGate>(root).expect("gate").weapons[0] = armed(FIRE_TICK, BELT);
        world.run_system_once(fold_arrivals).expect("fold runs");
        let ledger = world.get::<OwnFirePresentation>(root).expect("ledger");
        assert_eq!(
            ledger.slots[0].last_arrived,
            armed(FIRE_TICK, BELT),
            "an equal-value replication write updates the arrival record",
        );
        assert!(
            ledger.slots[0].pending_local.is_empty(),
            "the confirmation releases the pending consumption",
        );
    }

    /// Fixture RTT and spread size the reload-view window, including its one-send floor.
    #[test]
    fn the_reload_refresh_window_uses_the_measured_link() {
        const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);
        assert_eq!(refusal_wait_ticks(Duration::ZERO, Duration::ZERO, TICK), 1);
        // Fixture: ceil((100 ms RTT + 60 ms spread) / 15.625 ms tick) + one send interval.
        assert_eq!(
            refusal_wait_ticks(Duration::from_millis(100), Duration::from_millis(60), TICK),
            12,
        );
    }

    /// Reload-view fixture: TickDuration, timelines, the estimator digest, and
    /// the armed ledger root.
    fn heal_world(newest_remote: u32) -> (World, Entity) {
        let (mut world, root) = world_with_ledger(ready(BELT));
        world.insert_resource(lightyear::core::tick::TickDuration(
            core::time::Duration::from_nanos(1_000_000_000 / 64),
        ));
        world.insert_resource(ArrivalDelay::test_with_newest_remote(Tick(newest_remote)));
        world.init_resource::<OwnFireDiag>();
        (world, root)
    }

    /// The game's damage model: an unconfirmed local consumption refreshes only its reload view
    /// after the link window. It cannot author a round or recoil, and an in-flight input inside
    /// the window must retain its view state.
    #[test]
    fn an_unconfirmed_consumption_refreshes_only_the_reload_view() {
        const FIRE_TICK: u32 = 100;
        let (mut world, root) = heal_world(300);
        world.spawn(PingManager::default());
        world.init_resource::<CapturedShells>();
        world.init_resource::<super::super::client::PendingRecoilKicks>();
        world.add_observer(capture_shell);
        {
            let mut gate = world.get_mut::<WeaponGate>(root).expect("gate");
            gate.weapons[0] = armed(FIRE_TICK, BELT - 1);
            let mut ledger = world.get_mut::<OwnFirePresentation>(root).expect("ledger");
            ledger.slots[0].presented_gate = armed(FIRE_TICK, BELT - 1);
            ledger.slots[0].pending_local.push_back(FIRE_TICK);
        }
        world
            .run_system_once(heal_refused_presentations)
            .expect("heal runs");
        assert_eq!(
            gate_of(&world, root),
            ready(BELT),
            "the reload heals to the last state that actually arrived",
        );
        let ledger = world.get::<OwnFirePresentation>(root).expect("ledger");
        assert_eq!(ledger.slots[0].presented_gate, ready(BELT));
        assert!(ledger.slots[0].pending_local.is_empty());
        assert_eq!(world.resource::<OwnFireDiag>().healed, 1);
        assert!(
            world.resource::<CapturedShells>().0.is_empty(),
            "a refused round presents nothing",
        );
        assert!(
            world
                .resource::<super::super::client::PendingRecoilKicks>()
                .queued()
                .is_empty(),
            "and kicks nothing",
        );

        // A consumption still inside the deadline is in flight, not refused.
        let (mut world, root) = heal_world(300);
        world.spawn(PingManager::default());
        {
            let mut gate = world.get_mut::<WeaponGate>(root).expect("gate");
            gate.weapons[0] = armed(299, BELT - 1);
            let mut ledger = world.get_mut::<OwnFirePresentation>(root).expect("ledger");
            ledger.slots[0].presented_gate = armed(299, BELT - 1);
            ledger.slots[0].pending_local.push_back(299);
        }
        world
            .run_system_once(heal_refused_presentations)
            .expect("heal runs");
        assert_eq!(
            gate_of(&world, root),
            armed(299, BELT - 1),
            "silence has not yet proven anything — no heal inside the deadline",
        );
        assert_eq!(world.resource::<OwnFireDiag>().healed, 0);
    }

    #[derive(Resource, Default)]
    struct CapturedShells(Vec<(Option<crate::ballistics::ShotSource>, u32)>);

    fn capture_shell(fire: On<FireShell>, mut captured: ResMut<CapturedShells>) {
        captured.0.push((fire.shooter, fire.catch_up_ticks));
    }

    /// A shot released at the cursor presents exactly once — one `FireShell`, one recoil
    /// kick — younger than its first tick (`catch_up_ticks` 0), which keeps it under the muzzle
    /// flash's stale gate. The fixture plants a `LocalTimeline` 16 ticks past the release as
    /// wrong-clock bait: a mutant that ages the shot on the local/arrival timeline reports 16
    /// (flash stale-gated, shell 16 ticks downrange) and reds the 0.
    #[test]
    fn a_cursor_released_shot_presents_once_younger_than_its_first_tick() {
        let mut app = App::new();
        app.init_resource::<CapturedShells>();
        app.init_resource::<super::super::client::PendingRecoilKicks>();
        app.add_observer(capture_shell);
        let mut local = LocalTimeline::default();
        local.apply_delta(116);
        app.insert_resource(local);
        let shooter = app.world_mut().spawn_empty().id();
        let points = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_points = points.clone();
        app.add_observer(move |fire: On<FireShell>| {
            captured_points
                .lock()
                .unwrap()
                .push((fire.origin, fire.birth))
        });
        let facts = ReleasedShot {
            birth: Vec3::new(1.0, 2.0, 8.0),
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Dir3::NEG_Z,
            round: crate::tank::WeaponRound::resolve("88mm_AP")
                .expect("a shipped round")
                .round,
            tracer: true,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: ShotSource {
                tank: shooter,
                weapon: 0,
                round: 0,
            },
            shot: crate::ShotId {
                combatant: crate::CombatantId(7),
                weapon: 0,
                fire_tick: 100,
            },
        };
        let catch_up = app
            .world_mut()
            .run_system_once(
                move |mut recoil: ResMut<super::super::client::PendingRecoilKicks>,
                      mut commands: Commands| {
                    present_released_shot(
                        Tick(100),
                        (Tick(100), 0.25),
                        facts,
                        &mut recoil,
                        &mut commands,
                    )
                },
            )
            .expect("the seam runs");
        assert_eq!(
            catch_up,
            Some(0),
            "released a quarter-overstep before the cursor: age < 1 tick, never the local \
             timeline's 16",
        );
        assert_eq!(
            *points.lock().unwrap(),
            vec![(Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 8.0))],
            "the echoed birth and visual muzzle remain separate through presentation"
        );
        let world = app.world();
        let captured = world.resource::<CapturedShells>();
        let &[(source, presented_catch_up)] = captured.0.as_slice() else {
            panic!("exactly one bang presents, got {}", captured.0.len());
        };
        let source = source.expect("the bang carries its shooter");
        assert_eq!((source.tank, source.weapon), (shooter, 0));
        assert_eq!(presented_catch_up, 0);
        assert_eq!(
            world
                .resource::<super::super::client::PendingRecoilKicks>()
                .queued(),
            &[(shooter, 0)],
            "one kick, through the same seam as the bang",
        );
    }
}
