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
//! # TWO SELF-HEALS: THE ANNOUNCEMENT IS LOSSY, THE STATE IS THE TRUTH
//!
//! Under fused own fire the round's ONE presentation is the server's echoed announcement — a
//! droppable message — while the gate/belt state stream is the durable record. Two holes, one
//! ledger:
//!
//! - **A lost announcement** (the belt-delta fallback): consumption the state PROVES
//!   ([`fold_arrivals`]) with no announcement presenting within [`announce_wait_ticks`] is
//!   presented from this client's own spec and muzzle ([`recover_unannounced_rounds`]); a late
//!   echo for an already-recovered round is swallowed ([`OwnFirePresentation::try_swallow_owed`]).
//! - **A refused round** (the heal): a local consumption the server state stays SILENT about
//!   through [`refusal_wait_ticks`] was never fired there (a late/lost input the attestation
//!   zeroed); the presented reload restores from the last state that actually arrived
//!   ([`heal_refused_presentations`]) instead of running a reload for a round that never left.
//!
//! ARRIVALS ARE PROVEN BY THE COMPONENT'S CHANGE TICK, NEVER BY VALUE: the fused happy path arms
//! both sides with identical arithmetic, so the arriving gate frequently EQUALS the presented one
//! and a value diff is blind to it — while a refusal produces no server change, no send, and no
//! write at all. Replication applies through an unconditional deref write (`bevy_replicon`
//! `default_write`; equality-skip is opt-in and not used), so a change tick moved across the
//! replication window is an arrival and unmoved is silence.
//!
//! # SCOPE
//!
//! Client-side, own tank, `Interpolated` and not `Predicted` — the same observable role
//! `net::recoil_overlay` arms on. In predicted mode nothing here arms and the fire path is
//! bit-identical to what it was: the ledger component is absent, so the gate is untouched and the
//! attested command reaches `shooting::fire` unmodified.
//!
//! Design note: `.agents/scratch/burst-state-fire-stack-map-2026-08-14.md`; the loss-proofing
//! holes: `.agents/scratch/adaptive-cursor-frontier-2026-08-15.md` §3.

use core::time::Duration;
use std::collections::VecDeque;

use bevy::ecs::change_detection::Tick as ChangeTick;
use bevy::prelude::*;
use lightyear::core::tick::{Tick, TickDuration};
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::{
    Interpolated, IsSynced, LocalTimeline, NetworkTimeline, PingManager, Predicted,
};

use super::protocol::{InputBridge, NetTank};
use super::sync_margin::ArrivalDelay;
use crate::ballistics::{FireShell, FireShellOrigin, MAX_COSMETIC_CATCH_UP_TICKS, ShotSource};
use crate::command::TankCommand;
use crate::state::GameplaySet;
use crate::tank::{Controlled, Muzzle, TankRoot, Weapon, WeaponGate, WeaponGateState, WeaponIndex};

/// Authored ticks retained before the oldest is dropped. A memory bound only: consumption is by
/// exact tick, so an evicted entry is one no presentation tick can still reach.
const AUTHORED_TICKS: usize = 128;

/// Per-slot queue bound (reveals / pending consumptions / owed swallows). A memory bound only:
/// the deepest lawful queue is the in-flight window (≤ [`refusal_wait_ticks`]) over the fastest
/// cyclic period (6 ticks at 750 rpm) — a handful of entries at the 250 ms RTT ceiling.
const QUEUE_CAP: usize = 32;

/// Install the own-fire presentation ledger. [`record_own_intent`] is mounted separately, by
/// `net::client`, because it must be chained after the stamp that creates input provenance.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AuthoredIntent>();
    app.init_resource::<OwnFireDiag>();
    app.add_systems(Update, arm_own_fire_presentation);
    app.add_observer(count_presented_round);
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
    app.add_systems(
        Update,
        (recover_unannounced_rounds, heal_refused_presentations),
    );
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

/// One server consumption no presentation accounts for yet: stamped with the newest remote tick
/// whose arrival revealed it — the base of the announcement wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reveal {
    revealed_tick: u32,
    remaining: u32,
}

/// One weapon slot's presentation ledger.
#[derive(Debug, Clone, Default, PartialEq)]
struct SlotLedger {
    /// The gate the local cadence left last tick, restored over every arriving snapshot.
    presented_gate: WeaponGateState,
    /// The last state that provably ARRIVED (a replication write) — the base of the next
    /// consumption fold and the refusal heal's restore source. Never the presented copy.
    last_arrived: WeaponGateState,
    /// Rounds presented locally since the ledger was seeded.
    presented: u32,
    /// Rounds the arriving state accounts for ([`Self::confirm`]).
    confirmed: u32,
    /// `belt_remaining` of the last arriving snapshot — the base of the next delta.
    seen_belt: u32,
    /// Σ`remaining` == `max(0, confirmed − presented)`, reconciled on every move
    /// ([`Self::reconcile_reveals`]) — the announcements still owed a presentation.
    reveals: VecDeque<Reveal>,
    /// Local cadence consumptions awaiting server proof, by the fixed tick that consumed. FIFO;
    /// order ambiguity against server consumption is bounded by the cyclic period.
    pending_local: VecDeque<u32>,
    /// Reveal ticks already presented by the fallback: a late echo with `fire_tick` at or before
    /// one of these is the same round arriving twice and must not present again. `fire_tick`
    /// after every entry is a future round and never matches, so a stale entry cannot swallow one.
    owed_swallows: VecDeque<u32>,
}

impl SlotLedger {
    /// Seed from the arriving gate. The ONE tick the server's copy decides anything.
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
            u32::from(self.last_arrived.ready_tick.is_none() && arriving.ready_tick.is_some());
        let fired = belt_fired.max(arm_edge);
        self.confirmed = self.confirmed.saturating_add(fired);
        self.seen_belt = arriving.belt_remaining;
        self.last_arrived = arriving;
        // Server consumption proves the oldest awaiting local rounds.
        for _ in 0..fired {
            self.pending_local.pop_front();
        }
        fired
    }

    /// Keep Σ`reveals.remaining` == `max(0, confirmed − presented)`. Growth stamps the tick whose
    /// arrival revealed the excess; shrinkage (a presentation caught up) consumes oldest-first —
    /// the same FIFO the fallback presents in.
    fn reconcile_reveals(&mut self, revealed_tick: Option<u32>) {
        let target = self.confirmed.saturating_sub(self.presented);
        let mut queued: u32 = self.reveals.iter().map(|reveal| reveal.remaining).sum();
        while queued > target {
            let front = self
                .reveals
                .front_mut()
                .expect("queued > 0 implies a front");
            let cut = front.remaining.min(queued - target);
            front.remaining -= cut;
            queued -= cut;
            if front.remaining == 0 {
                self.reveals.pop_front();
            }
        }
        if target > queued
            && let Some(revealed_tick) = revealed_tick
        {
            self.reveals.push_back(Reveal {
                revealed_tick,
                remaining: target - queued,
            });
            if self.reveals.len() > QUEUE_CAP {
                self.reveals.pop_front();
            }
        }
    }

    /// Consume one owed swallow matching this echo, oldest-first (wrap-safe tick order).
    fn swallow_owed(&mut self, fire_tick: u32) -> bool {
        let Some(index) = self
            .owed_swallows
            .iter()
            .position(|owed| Tick(fire_tick) - Tick(*owed) <= 0)
        else {
            return false;
        };
        self.owed_swallows.remove(index);
        true
    }

    /// Rounds presented that no arriving snapshot accounts for. Includes the rounds still in flight
    /// down the link, so it settles on a burst's phantom count only once the last snapshot lands.
    fn absorbed(&self) -> u32 {
        self.presented.saturating_sub(self.confirmed)
    }

    /// Rounds accounted for that this client has not presented. Under fused own fire this is the
    /// lawful in-flight window (and, past the wait, the fallback's due list); in mode A it is the
    /// direction that cannot happen and is reported, never acted on.
    fn overrun(&self) -> u32 {
        self.confirmed.saturating_sub(self.presented)
    }
}

/// The owner's fire-presentation ledger: one entry per weapon slot, in `WeaponGate` order.
#[derive(Component, Debug)]
pub(super) struct OwnFirePresentation {
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

    /// Swallow a late own echo the fallback already presented for (`net::client`'s release path).
    pub(super) fn try_swallow_owed(&mut self, weapon: usize, fire_tick: u32) -> bool {
        self.slots
            .get_mut(weapon)
            .is_some_and(|slot| slot.swallow_owed(fire_tick))
    }
}

/// Self-heal counters, appended to the FRONTIER summary line by `net::extrapolate`.
#[derive(Resource, Default, Debug)]
pub(super) struct OwnFireDiag {
    /// Fallback bangs presented for state-proven consumptions with no announcement.
    recovered: u64,
    /// Presented reloads restored after the server stayed silent past the refusal deadline.
    healed: u64,
    /// Late own echoes swallowed because the fallback had already presented their round.
    pub(super) swallowed: u64,
}

impl OwnFireDiag {
    pub(super) fn describe(&self) -> String {
        format!(
            "own-fire(recovered={} healed={} swallowed={})",
            self.recovered, self.healed, self.swallowed,
        )
    }
}

/// One released shot's wire/state facts — everything presentation needs except its age, which is
/// the seam's to measure.
#[derive(Clone, Copy)]
pub(super) struct ReleasedShot {
    pub(super) origin: Vec3,
    pub(super) direction: Dir3,
    pub(super) speed: f32,
    pub(super) caliber: f32,
    pub(super) mass: f32,
    pub(super) tracer: bool,
    pub(super) mechanism: crate::spec::FireMechanism,
    pub(super) shooter: ShotSource,
    pub(super) shot: Option<crate::ShotId>,
}

/// THE SHOT-PRESENTATION SEAM: one complete released shot presents through this call — the
/// `FireShell` it triggers carries the muzzle flash and report dressing (`vfx::muzzle`), the
/// tracer/shell spawn (`ballistics`), and the ledger count ([`count_presented_round`]); the barrel
/// kick queues beside it. Every cursor release path — the fused own echo, remote fire
/// announcements, and the belt-delta fallback — funnels here.
///
/// CURSOR CLOCK (the per-channel clock map): a released shot's age is `cursor − released_tick`,
/// fractional ticks on the same clock that released it — ≈ 0..1 at a crossing release, so the
/// shell spawns at the muzzle and the flash clears `ballistics::STALE_FIRE_TICKS`. The local
/// timeline's lead+delay must never age a cursor-released shot: it spawns the tracer a whole fuse
/// downrange and stale-gates the flash.
///
/// `None` = the age exceeds [`MAX_COSMETIC_CATCH_UP_TICKS`] (the absurdity bar `ballistics`
/// enforces on every `FireShell`): nothing presents and the caller's ledger stays untouched.
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
        origin: shot.origin,
        direction: shot.direction,
        speed: shot.speed,
        caliber: shot.caliber,
        mass: shot.mass,
        mechanism: shot.mechanism,
        tracer: shot.tracer,
        shooter: Some(shot.shooter),
        shot_origin: FireShellOrigin::Reconstructed,
        catch_up_ticks,
        shot: shot.shot,
    });
    recoil.push(shot.shooter.tank, shot.shooter.weapon);
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
        commands
            .entity(entity)
            .insert(OwnFirePresentation::with_slots(
                gate.weapons
                    .iter()
                    .copied()
                    .map(SlotLedger::seeded)
                    .collect(),
            ));
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
    fused: Option<Res<crate::FusedOwnFire>>,
    arrival: Option<Res<ArrivalDelay>>,
    mut roots: Query<(Ref<WeaponGate>, &mut OwnFirePresentation)>,
) {
    let revealed_tick = arrival
        .as_ref()
        .and_then(|estimator| estimator.newest_remote())
        .map(|tick| tick.0);
    for (gate, mut ledger) in &mut roots {
        if ledger.gate_write_mark == Some(gate.last_changed()) {
            continue;
        }
        // An unset mark (fresh arm) folds once against the seed — zero rounds by construction.
        ledger.gate_write_mark = Some(gate.last_changed());
        for (slot, entry) in ledger.slots.iter_mut().enumerate() {
            let Some(arriving) = gate.weapons.get(slot).copied() else {
                continue;
            };
            if arriving == entry.last_arrived {
                continue;
            }
            if entry.confirm(arriving) > 0 {
                entry.reconcile_reveals(revealed_tick);
            }
            if entry.overrun() > 0 {
                // Under fused own fire the state's consumption legitimately lands before the
                // echoed round crosses the cursor, so a transient overrun is the in-flight
                // window (and, past the wait, the fallback's due list), not a violated invariant.
                if fused.is_some() {
                    debug!(
                        "net: weapon {slot} has {} confirmed rounds not yet presented \
                         (presented {}, confirmed {})",
                        entry.overrun(),
                        entry.presented,
                        entry.confirmed,
                    );
                } else {
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
            }
        }
    }
}

/// Restore the presented gate over every arriving snapshot. Consumption accounting lives in
/// [`fold_arrivals`]; this is the presentation-side restore only.
fn hold_presentation_gate(mut roots: Query<(&mut WeaponGate, &OwnFirePresentation)>) {
    for (mut gate, ledger) in &mut roots {
        // Immutable deref first: a tick with no arriving snapshot must not flag the component.
        let arrived = ledger.slots.iter().enumerate().any(|(slot, entry)| {
            gate.weapons
                .get(slot)
                .is_some_and(|arriving| *arriving != entry.presented_gate)
        });
        if !arrived {
            continue;
        }
        for (slot, entry) in ledger.slots.iter().enumerate() {
            if gate.weapons.get(slot).is_some() {
                gate.weapons[slot] = entry.presented_gate;
            }
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
                let consumed = entry
                    .presented_gate
                    .belt_remaining
                    .saturating_sub(current.belt_remaining)
                    .max(u32::from(
                        entry.presented_gate.ready_tick.is_none() && current.ready_tick.is_some(),
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
}

/// The announcement wait W1, in cursor ticks past a reveal: the burst-tail coverage `Q_p − Q_50`
/// — the announcement may ride the tail while the revealing snapshot rode the bulk — plus one
/// send interval. The seam presents exactly once whichever of the announcement and the fallback
/// lands first, so the wait is a heal deadline, not a correctness gate.
fn announce_wait_ticks(coverage: Duration, tick: Duration) -> u32 {
    (coverage.as_secs_f64() / tick.as_secs_f64()).ceil() as u32 + 1
}

/// The refusal deadline W2, in remote ticks past a local consumption: one full RTT (the
/// loss-repair round trip — the input rides redundant per-packet history, so one lost packet
/// re-delivers within it) + the downlink spread `Q_p − min` (the confirming snapshot may itself
/// ride the burst tail) + one send interval. Silence past this is refusal, not flight.
fn refusal_wait_ticks(rtt: Duration, spread: Duration, tick: Duration) -> u32 {
    ((rtt.as_secs_f64() + spread.as_secs_f64()) / tick.as_secs_f64()).ceil() as u32 + 1
}

/// Hole 1, the belt-delta fallback: present a state-proven consumption whose announcement never
/// crossed the cursor within [`announce_wait_ticks`], from this client's own spec and muzzle.
/// Fused mode only — in mode A the own round presents locally and the reveal queue stays empty.
///
/// The bang is synthesized at the muzzle's render pose: the tick-truth pose doctrine
/// (`shooting::rig_world_pose`) binds shells the server also computes, and this shell is a
/// client-side repair of a round the server already fired. `shot: None` — the round's `ShotId`
/// died with its announcement, so the shell flies uncorrected (no sanctioned bounces), which is
/// the honest remainder. One round per slot per frame; the triggered observer settles the reveal
/// at flush, so the next round presents next frame.
#[expect(
    clippy::too_many_arguments,
    reason = "one recovery boundary owns the wait, the pose, and the counters"
)]
fn recover_unannounced_rounds(
    fused: Option<Res<crate::FusedOwnFire>>,
    tick: Res<TickDuration>,
    arrival: Option<Res<ArrivalDelay>>,
    cursors: Query<&InterpolationTimeline, With<IsSynced<InterpolationTimeline>>>,
    mut roots: Query<(Entity, &mut OwnFirePresentation)>,
    muzzles: Query<(&Weapon, &WeaponIndex, &TankRoot, &GlobalTransform), With<Muzzle>>,
    mut recoil: ResMut<super::client::PendingRecoilKicks>,
    mut diag: ResMut<OwnFireDiag>,
    mut commands: Commands,
) {
    if fused.is_none() {
        return;
    }
    let Some(arrival) = arrival else {
        return;
    };
    let Ok(cursor) = cursors.single() else {
        return;
    };
    let wait = announce_wait_ticks(arrival.stats.coverage(), tick.0);
    // CURSOR clock, both halves: the wait is measured in cursor ticks past the reveal, and the
    // recovered round's age is the seam's `cursor − revealed_tick` — a fallback bang lands at the
    // same lag its lost announcement would have presented at.
    let cursor = (cursor.tick(), f64::from(cursor.overstep().to_f32()));
    for (root, mut ledger) in &mut roots {
        for (slot, entry) in ledger.slots.iter_mut().enumerate() {
            let Some(&Reveal { revealed_tick, .. }) = entry.reveals.front() else {
                continue;
            };
            if (cursor.0 - Tick(revealed_tick)) < wait as i32 {
                continue;
            }
            let Some((weapon, muzzle)) = muzzles.iter().find_map(|(weapon, index, tank, pose)| {
                (index.0 == slot && tank.0 == root).then_some((weapon, pose))
            }) else {
                continue;
            };
            let Ok(direction) = Dir3::new(muzzle.rotation() * Vec3::NEG_Z) else {
                continue;
            };
            let facts = ReleasedShot {
                origin: muzzle.translation(),
                direction,
                speed: weapon.speed,
                caliber: weapon.caliber,
                mass: weapon.mass,
                // The round's belt phase died with the announcement; a recovered round
                // presents traced — the visible arm of the self-heal.
                tracer: true,
                mechanism: weapon.fire_mode.mechanism(),
                shooter: ShotSource {
                    tank: root,
                    weapon: slot,
                },
                // The round's ShotId died with its announcement, so the shell flies
                // uncorrected (no sanctioned bounces) — the honest remainder.
                shot: None,
            };
            if present_released_shot(
                Tick(revealed_tick),
                cursor,
                facts,
                &mut recoil,
                &mut commands,
            )
            .is_none()
            {
                // Over-horizon reveal (a multi-second stall with a round in flight): present at
                // the horizon rather than jamming the slot — a rejected front reveal would retry
                // forever and block every round behind it. The flash observers stale-gate the
                // bang; the ledger settles through the same observer as any presentation.
                present_shot_at_age(
                    MAX_COSMETIC_CATCH_UP_TICKS,
                    facts,
                    &mut recoil,
                    &mut commands,
                );
            }
            entry.owed_swallows.push_back(revealed_tick);
            if entry.owed_swallows.len() > QUEUE_CAP {
                entry.owed_swallows.pop_front();
            }
            diag.recovered += 1;
            info!(
                "net: {root} weapon {slot} round revealed by state at tick {revealed_tick} had \
                 no announcement within {wait} cursor ticks — presenting from the belt delta \
                 (recovered {})",
                diag.recovered,
            );
        }
    }
}

/// Hole 2, the refusal heal: a local consumption the server state stays silent about through
/// [`refusal_wait_ticks`] was never fired there (the attestation zeroed a late/lost input). The
/// presented reload restores from the last state that actually arrived — the belt round returns
/// and readiness re-derives on the local cadence — instead of running a reload for a round that
/// never left. Both modes: mode A presented the phantom bang (absorbed accounting already covers
/// it); fused presented nothing and no echo will come.
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
    let Ok(pings) = pings.single() else {
        return;
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
                 never consumed by the server (state silent through tick {}, wait {wait}t) — \
                 presented reload healed from the last arrived gate (healed {})",
                newest_remote.0, diag.healed,
            );
        }
    }
}

/// Count one presented round against its slot. Same observer channel `net::recoil_overlay` excites
/// on, and the same three filters: a reconstructed opponent shot is somebody else's, a sandbox
/// free-fly shot has no shooter, and the query admits only the armed own root.
///
/// Under fused own fire (`crate::FusedOwnFire`) the own round's one presentation IS the
/// reconstructed echo, so the count admits `Reconstructed` shots too — the ledger query still
/// restricts them to the armed own root, and in mode A an own-rooted reconstructed shot cannot
/// occur (the echo is suppressed), so the admission changes nothing with the lever unset.
fn count_presented_round(
    fire: On<FireShell>,
    fused: Option<Res<crate::FusedOwnFire>>,
    mut ledgers: Query<&mut OwnFirePresentation>,
) {
    let counted = match fire.shot_origin {
        FireShellOrigin::Local => true,
        FireShellOrigin::Reconstructed => fused.is_some(),
    };
    if !counted {
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
        // A presentation catching up settles the oldest owed reveal — the announcement (or the
        // fallback's own synthesized round) accounts for it.
        entry.reconcile_reveals(None);
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
        world.insert_resource(LocalTimeline::default());
        let root = world
            .spawn((
                WeaponGate {
                    weapons: vec![gate],
                },
                OwnFirePresentation::with_slots(vec![SlotLedger::seeded(gate)]),
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
                OwnFirePresentation::with_slots(vec![SlotLedger::default()]),
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

    /// LEDGER COUNTING ACROSS THE MODES: a reconstructed round on the armed own root counts only
    /// under the fused lever (there it IS the round's one presentation); with the lever unset it
    /// does not count — mode A's Local-only accounting, pinned. A Local round counts in both.
    #[test]
    fn a_reconstructed_own_round_counts_only_under_the_fused_lever() {
        let fire = |origin| FireShell {
            origin: Vec3::ZERO,
            direction: Dir3::NEG_Z,
            speed: 755.0,
            caliber: 0.0079,
            mass: 0.0118,
            mechanism: crate::spec::FireMechanism::Automatic,
            shooter: None,
            tracer: true,
            shot_origin: origin,
            catch_up_ticks: 0,
            shot: None,
        };
        let presented = |fused: bool, origin| {
            let mut app = App::new();
            if fused {
                app.insert_resource(crate::FusedOwnFire);
            }
            app.add_observer(count_presented_round);
            let root = app
                .world_mut()
                .spawn(OwnFirePresentation::with_slots(vec![SlotLedger::default()]))
                .id();
            let mut event = fire(origin);
            event.shooter = Some(crate::ballistics::ShotSource {
                tank: root,
                weapon: 0,
            });
            app.world_mut().trigger(event);
            app.world()
                .get::<OwnFirePresentation>(root)
                .expect("armed ledger")
                .slots[0]
                .presented
        };

        assert_eq!(
            presented(false, FireShellOrigin::Reconstructed),
            0,
            "mode A: a reconstructed shot never counts against the own ledger",
        );
        assert_eq!(
            presented(true, FireShellOrigin::Reconstructed),
            1,
            "fused: the echo is the own round's one presentation and must count",
        );
        assert_eq!(
            presented(false, FireShellOrigin::Local),
            1,
            "mode A: the local round counts exactly as before",
        );
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

    /// CONSUMPTION IS `max(belt delta, arm edge)`, NEVER THEIR SUM. A `Single` draws no belt —
    /// dropping the arm-edge term (the old belt-only law) reds the first assert, and every cannon
    /// consumption becomes invisible to the fallback. Summing the signals reds the last assert —
    /// every automatic round would double-confirm. `Some → Some` (cyclic) and `Some → None`
    /// (retirement) are bookkeeping, not consumption; counting either reds the middle asserts.
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
        assert_eq!(slot.confirmed, 2);
    }

    /// Σ`reveals.remaining` == `max(0, confirmed − presented)` through every move. Dropping the
    /// settle on a presentation (the reconcile in `count_presented_round`) leaves a reveal queued
    /// after its echo arrived and the fallback presents the round twice — the second half reds.
    /// Dropping the stamp on growth loses the wait's base — the first half reds.
    #[test]
    fn reveals_carry_exactly_the_unpresented_confirmations() {
        let queued = |slot: &SlotLedger| -> u32 { slot.reveals.iter().map(|r| r.remaining).sum() };
        let mut slot = SlotLedger::seeded(ready(BELT));
        slot.confirm(ready(BELT - 2));
        slot.reconcile_reveals(Some(700));
        assert_eq!(queued(&slot), 2, "two confirmed, none presented");
        assert_eq!(slot.reveals.front().expect("stamped").revealed_tick, 700);
        // An echo catches up: the oldest owed reveal settles.
        slot.presented += 1;
        slot.reconcile_reveals(None);
        assert_eq!(queued(&slot), 1);
        // Presentation overtakes (mode A: the local bang precedes every confirm): queue empties
        // and stays empty — the fallback owes nothing.
        slot.presented += 2;
        slot.reconcile_reveals(None);
        assert!(slot.reveals.is_empty());
        // Later excess stamps the tick of the arrival that revealed IT, not the old one.
        slot.confirm(ready(BELT - 4));
        slot.reconcile_reveals(Some(800));
        assert_eq!(queued(&slot), 1);
        assert_eq!(slot.reveals.front().expect("stamped").revealed_tick, 800);
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
                ledger.slots[0].confirmed, 0,
                "a local write is not an arrival — silence is the refusal signal",
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
            ledger.slots[0].confirmed, 1,
            "an equal-value replication write IS an arrival — the arm edge confirms",
        );
        assert!(
            ledger.slots[0].pending_local.is_empty(),
            "the confirmation releases the pending consumption",
        );
    }

    /// THE WAIT LAWS, from the measured link. W1 rides the burst-tail coverage (the announcement
    /// may ride the tail while the revealing snapshot rode the bulk) + one interval; W2 rides a
    /// full loss-repair RTT + the whole spread + one interval. Dropping any term reds a case.
    #[test]
    fn the_waits_derive_from_the_measured_link() {
        const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);
        assert_eq!(announce_wait_ticks(Duration::ZERO, TICK), 1, "the floor");
        // The measured 60 ms burst tail: ceil(60/15.625) + 1.
        assert_eq!(announce_wait_ticks(Duration::from_millis(60), TICK), 5);
        assert_eq!(refusal_wait_ticks(Duration::ZERO, Duration::ZERO, TICK), 1);
        // rtt 100 ms over the burst link: ceil(160/15.625) + 1.
        assert_eq!(
            refusal_wait_ticks(Duration::from_millis(100), Duration::from_millis(60), TICK),
            12,
        );
    }

    /// AN OWED SWALLOW MATCHES ONLY THE ROUND IT COVERED: `fire_tick` at or before the recorded
    /// reveal, exactly once — a future round never matches, so a stale entry cannot eat a fresh
    /// echo. Inverting the comparison or dropping the removal reds a half.
    #[test]
    fn a_late_echo_swallow_matches_once_and_never_a_future_round() {
        let mut slot = SlotLedger::seeded(ready(BELT));
        slot.owed_swallows.push_back(100);
        assert!(!slot.swallow_owed(101), "a future round never matches");
        assert!(slot.swallow_owed(99), "the covered round swallows");
        assert!(!slot.swallow_owed(99), "exactly once");
    }

    /// What the recovery/heal worlds share: TickDuration, timelines, the estimator digest, and
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

    /// HOLE 2, THE LATE-FIRE REFUSAL: the presented reload heals once the server state stays
    /// silent past the deadline — the gate restores from the last ARRIVED state (ready, full
    /// belt), the pending consumption retires, and the counter reports it. Dropping the deadline
    /// check heals a round still in flight (reds the young half); dropping the restore leaves
    /// the player running a reload for a round that never left (reds the gate assert).
    #[test]
    fn a_refused_round_heals_the_presented_reload_from_the_arrived_gate() {
        const FIRE_TICK: u32 = 100;
        let (mut world, root) = heal_world(300);
        world.spawn(PingManager::default());
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

    /// One presented `FireShell`'s identifying fields, captured off the observer channel.
    #[derive(Resource, Default)]
    struct CapturedShells(Vec<(Option<crate::ballistics::ShotSource>, u32, bool)>);

    fn capture_shell(fire: On<FireShell>, mut captured: ResMut<CapturedShells>) {
        captured
            .0
            .push((fire.shooter, fire.catch_up_ticks, fire.shot.is_none()));
    }

    fn recovery_app(fused: bool, cursor_tick: u32) -> (App, Entity) {
        use lightyear::core::time::TickInstant;
        let mut app = App::new();
        if fused {
            app.insert_resource(crate::FusedOwnFire);
        }
        app.insert_resource(lightyear::core::tick::TickDuration(
            core::time::Duration::from_nanos(1_000_000_000 / 64),
        ));
        let mut local = LocalTimeline::default();
        local.apply_delta(110);
        app.insert_resource(local);
        app.insert_resource(ArrivalDelay::default());
        app.init_resource::<OwnFireDiag>();
        app.init_resource::<super::super::client::PendingRecoilKicks>();
        app.init_resource::<CapturedShells>();
        app.add_observer(capture_shell);
        app.add_observer(count_presented_round);
        let mut timeline = InterpolationTimeline::default();
        timeline.set_now(TickInstant::from(Tick(cursor_tick)));
        app.world_mut()
            .spawn((timeline, IsSynced::<InterpolationTimeline>::default()));
        // The armed root: one state-proven consumption revealed at tick 100, never announced.
        let mut slot = SlotLedger::seeded(ready(BELT));
        slot.confirmed = 1;
        slot.reveals.push_back(Reveal {
            revealed_tick: 100,
            remaining: 1,
        });
        let root = app
            .world_mut()
            .spawn(OwnFirePresentation::with_slots(vec![slot]))
            .id();
        app.world_mut().spawn((
            Muzzle,
            WeaponIndex(0),
            TankRoot(root),
            GlobalTransform::from(Transform::from_xyz(1.0, 2.0, 3.0)),
            Weapon {
                name: "test gun".into(),
                speed: 755.0,
                caliber: 0.088,
                mass: 10.2,
                fire_mode: crate::spec::FireMode::Single { reload_secs: 6.0 },
                recoil: None,
                barrel: None,
                fire: vec![],
                load: vec![],
                trigger: crate::spec::Trigger::Primary,
            },
        ));
        (app, root)
    }

    /// HOLE 1, THE DROPPED ANNOUNCEMENT: once the cursor passes the reveal by the derived wait,
    /// the bang presents from the belt delta — this client's own spec at its own muzzle, keyed to
    /// the armed root so the ledger settles (presented +1, the reveal retires) and the recoil
    /// kick queues. Dropping the wait check presents rounds whose announcements are still in
    /// flight (reds the young half); dropping the fused gate presents in mode A where the local
    /// tick already did (reds the unfused case); dropping the trigger, the settle, or the kick
    /// reds its own assert.
    #[test]
    fn a_dropped_announcement_presents_from_the_belt_delta_within_the_wait() {
        // Cursor 5 ticks past the reveal — beyond the cold-estimator wait of 1.
        let (mut app, root) = recovery_app(true, 105);
        app.world_mut()
            .run_system_once(recover_unannounced_rounds)
            .expect("recovery runs");
        {
            let world = app.world();
            let captured = world.resource::<CapturedShells>();
            let &[(shooter, catch_up, unkeyed)] = captured.0.as_slice() else {
                panic!(
                    "exactly one recovered bang presents, got {}",
                    captured.0.len()
                );
            };
            let source = shooter.expect("the bang is keyed to the armed root");
            assert_eq!((source.tank, source.weapon), (root, 0));
            assert_eq!(
                catch_up, 5,
                "catch-up spans reveal (100) to the CURSOR (105) — the cursor clock, never the \
                 local present (110, which a wrong-clock mutant would read as 10)",
            );
            assert!(unkeyed, "the round's ShotId died with its announcement");
            let ledger = world.get::<OwnFirePresentation>(root).expect("ledger");
            assert_eq!(
                ledger.slots[0].presented, 1,
                "the observer counted the bang"
            );
            assert!(ledger.slots[0].reveals.is_empty(), "the reveal settled");
            assert_eq!(
                ledger.slots[0].owed_swallows.front(),
                Some(&100),
                "a late echo for this round is now owed a swallow",
            );
            assert_eq!(
                world
                    .resource::<super::super::client::PendingRecoilKicks>()
                    .queued(),
                &[(root, 0)],
                "the fallback recoils through the same seam as an echoed round",
            );
            assert_eq!(world.resource::<OwnFireDiag>().describe(), {
                "own-fire(recovered=1 healed=0 swallowed=0)"
            });
        }

        // Inside the wait the announcement may still cross the cursor: nothing presents.
        let (mut app, _) = recovery_app(true, 100);
        app.world_mut()
            .run_system_once(recover_unannounced_rounds)
            .expect("recovery runs");
        assert!(
            app.world().resource::<CapturedShells>().0.is_empty(),
            "no bang inside the announcement wait",
        );

        // Mode A never owes a fallback: the local tick presents own rounds.
        let (mut app, _) = recovery_app(false, 105);
        app.world_mut()
            .run_system_once(recover_unannounced_rounds)
            .expect("recovery runs");
        assert!(
            app.world().resource::<CapturedShells>().0.is_empty(),
            "the fallback is fused-mode machinery only",
        );
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
        let facts = ReleasedShot {
            origin: Vec3::new(1.0, 2.0, 3.0),
            direction: Dir3::NEG_Z,
            speed: 755.0,
            caliber: 0.088,
            mass: 10.2,
            tracer: true,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: ShotSource {
                tank: shooter,
                weapon: 0,
            },
            shot: None,
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
        let world = app.world();
        let captured = world.resource::<CapturedShells>();
        let &[(source, presented_catch_up, _)] = captured.0.as_slice() else {
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

    /// A refused round presents NOTHING: the heal restores the gate and retires the pending
    /// consumption without a `FireShell` or a recoil kick. A mutant that routes the refusal
    /// heal through the presentation seam reds the empty captures.
    #[test]
    fn a_refused_round_presents_no_flash_no_bang_no_kick() {
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
            world.resource::<OwnFireDiag>().healed,
            1,
            "the refusal healed"
        );
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
    }

    // --- The clock-conformance tripwire -----------------------------------------------------
    //
    // Source-scan in the `no_latest_arrival_sim_writer` pattern (`net::protocol` tests): honest
    // substring scans over comment-stripped source, biased toward false trips (a harmless
    // re-read of the contract) over false passes (a presentation site quietly rewired to the
    // wrong clock).

    /// Read a repo-relative source file for a scan.
    fn read_source(rel: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
    }

    /// Blank `//` line- and `/* */` (nesting) block-comments to spaces, preserving newlines, so
    /// only CODE is scanned and prose can neither trip nor mask the tripwire.
    fn strip_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut rest = src;
        let mut block_depth = 0usize;
        while let Some(c) = rest.chars().next() {
            if block_depth > 0 {
                if rest.starts_with("/*") {
                    block_depth += 1;
                    out.push_str("  ");
                    rest = &rest[2..];
                } else if rest.starts_with("*/") {
                    block_depth -= 1;
                    out.push_str("  ");
                    rest = &rest[2..];
                } else {
                    out.push(if c == '\n' { '\n' } else { ' ' });
                    rest = &rest[c.len_utf8()..];
                }
            } else if rest.starts_with("/*") {
                block_depth = 1;
                out.push_str("  ");
                rest = &rest[2..];
            } else if rest.starts_with("//") {
                let end = rest.find('\n').unwrap_or(rest.len());
                for _ in 0..end {
                    out.push(' ');
                }
                rest = &rest[end..];
            } else {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
        out
    }

    /// One fn's body (between its outermost braces), brace-matched so braces inside it don't
    /// fool the scan. `signature` must be the unique `fn name(` prefix.
    fn fn_body<'a>(stripped: &'a str, signature: &str) -> &'a str {
        let sig = stripped
            .find(signature)
            .unwrap_or_else(|| panic!("{signature} present"));
        let open = sig + stripped[sig..].find('{').expect("fn opening brace");
        let bytes = stripped.as_bytes();
        let mut depth = 0i32;
        for (i, &b) in bytes.iter().enumerate().skip(open) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &stripped[open + 1..i];
                    }
                }
                _ => {}
            }
        }
        panic!("{signature} body unterminated");
    }

    /// THE PER-CHANNEL CLOCK MAP, pinned in source: every cursor-assigned presentation site
    /// consumes the cursor clock, and none reads the local/arrival timeline for age or rate —
    /// the seam ages a shot only from its released tick and the cursor; the belt-delta fallback
    /// carries no `LocalTimeline` at all; the client's release path routes cursor releases
    /// through the seam; `TrackDrive` is registered for interpolation so the track view's speed
    /// source is cursor-time on interpolated hulls. Rewiring any of these to arrival-fresh
    /// state reds its assert.
    #[test]
    fn cursor_assigned_presentation_reads_no_local_timeline() {
        let fire = strip_comments(&read_source("src/net/fire_presentation.rs"));
        let seam = fn_body(&fire, "pub(super) fn present_released_shot(");
        for banned in ["LocalTimeline", "fire_catch_up_ticks", "PredictedPresent"] {
            assert!(
                !seam.contains(banned),
                "the seam ages on the cursor only: found {banned}",
            );
        }
        let fallback = fn_body(&fire, "fn recover_unannounced_rounds(");
        for banned in ["LocalTimeline", "fire_catch_up_ticks"] {
            assert!(
                !fallback.contains(banned),
                "the belt-delta fallback is cursor-aged: found {banned}",
            );
        }

        let client = strip_comments(&read_source("src/net/client.rs"));
        let release = fn_body(&client, "fn spawn_reconstructed_fire(");
        assert!(
            release.contains("present_released_shot"),
            "the cursor release path must route through the presentation seam",
        );

        let protocol = strip_comments(&read_source("src/net/protocol.rs"));
        assert!(
            protocol.contains("add_interpolation_with(track_drive_lerp)"),
            "TrackDrive must interpolate: without the registration the track view reads \
             arrival-fresh drivetrain state and the tracks lead the hull",
        );
    }
}
