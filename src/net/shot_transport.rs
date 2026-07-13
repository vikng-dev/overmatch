//! Server-side transport policy for public shot presentation and private consequences.

use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::prelude::server::Server;
use lightyear::prelude::*;
use serde_json::json;

use super::protocol::{
    DamageChannel, DamageConfirm, DamageReceipt, FireChannel, FireEvent, FireVisualBatch,
    FireVisualFact, ImpactConfirm, OutcomeChannel, RicochetKeyframe,
};
use crate::ballistics::{FireShell, Projectile, ShellDamage, ShellRicochet, ShellTerminal, Shot};
use crate::spec::FireMechanism;
use crate::state::GameplaySet;

/// DERIVED STARTING DEFAULT: the emission plus the next two send opportunities gives each
/// automatic-fire visual three bounded chances without creating reliable cosmetic debt.
const VISUAL_COPIES: u8 = 3;
/// DERIVED: sixteen 64 Hz ticks are 250 ms, matching the current client armor-outcome hold span.
const VISUAL_TTL_TICKS: i32 = 16;
/// DERIVED from Lightyear 0.28's 1,156-byte unfragmented-message ceiling, leaving 56 bytes of
/// headroom after worst-case bincode entity encoding and the message type id.
pub(crate) const VISUAL_BATCH_WIRE_LIMIT: usize = 1_100;
/// DERIVED STARTING DEFAULT: four maximum-size batches cover the current 30-tank, two-weapon
/// synchronized volley while bounding one recipient's automatic-fire work per server tick.
const VISUAL_TICK_WIRE_LIMIT: usize = VISUAL_BATCH_WIRE_LIMIT * 4;
/// DERIVED: Lightyear's registered `MessageNetId` is a varint-encoded `u16`, whose largest tier is
/// four bytes.
const MESSAGE_NET_ID_BYTES: usize = 4;
/// DERIVED from Lightyear 0.28's `SendEntityMap`: recipient-mapped entities set bit 63 before Bevy's
/// `u64` serde representation is encoded, forcing bincode's nine-byte `u64` tier.
const WORST_CASE_MAPPED_ENTITY: Entity = Entity::from_bits(0x8000_0000_0000_0001);

#[derive(Clone)]
struct PendingVisual {
    fact: FireVisualFact,
    emitted: Tick,
    copies_left: u8,
    last_sent: Option<Tick>,
}

struct ProducerQueue {
    combatant: crate::CombatantId,
    pending: VecDeque<PendingVisual>,
}

#[derive(Resource, Default)]
pub(crate) struct ShotTransportMetrics {
    pub visual_enqueued: u64,
    pub visual_selected: u64,
    /// Facts whose Lightyear send call returned `Ok` with at least one public target.
    pub visual_send_accepted_facts: u64,
    pub visual_expired: u64,
    pub visual_budget_deferred_producers: u64,
    pub visual_send_accepted_batches: u64,
    pub visual_send_accepted_wire_upper_bound_bytes: u64,
    pub max_visual_queue: usize,
    pub max_batch_wire_bytes: usize,
    pub reliable_public_enqueued: u64,
    pub reliable_public_send_accepted_facts: u64,
    pub private_damage_enqueued: u64,
    pub private_damage_send_accepted_facts: u64,
    pub public_no_recipient_facts: u64,
    pub private_damage_no_recipient_facts: u64,
    pub send_call_errors: u64,
    pub send_call_error_facts: u64,
    pub route_conflicts: u64,
}

#[derive(Default)]
struct VisualQueue {
    producers: Vec<ProducerQueue>,
    cursor: usize,
}

impl VisualQueue {
    fn enqueue(&mut self, fact: FireVisualFact, now: Tick, metrics: &mut ShotTransportMetrics) {
        let combatant = fact.shot_id().combatant;
        let index = self
            .producers
            .iter()
            .position(|producer| producer.combatant == combatant)
            .unwrap_or_else(|| {
                self.producers.push(ProducerQueue {
                    combatant,
                    pending: VecDeque::new(),
                });
                self.producers.len() - 1
            });
        self.producers[index].pending.push_back(PendingVisual {
            emitted: now,
            fact,
            copies_left: VISUAL_COPIES,
            last_sent: None,
        });
        metrics.visual_enqueued += 1;
        metrics.max_visual_queue = metrics.max_visual_queue.max(self.len());
    }

    fn len(&self) -> usize {
        self.producers
            .iter()
            .map(|producer| producer.pending.len())
            .sum()
    }

    fn drain_batches(
        &mut self,
        now: Tick,
        metrics: &mut ShotTransportMetrics,
    ) -> Vec<FireVisualBatch> {
        for producer in &mut self.producers {
            let before = producer.pending.len();
            producer
                .pending
                .retain(|pending| now - pending.emitted <= VISUAL_TTL_TICKS);
            metrics.visual_expired += (before - producer.pending.len()) as u64;
        }
        self.producers
            .retain(|producer| !producer.pending.is_empty());
        if self.producers.is_empty() {
            self.cursor = 0;
            return Vec::new();
        }
        self.cursor %= self.producers.len();

        let mut selected = Vec::new();
        let mut budget_used = 0;
        let mut budget_refused = false;
        let mut fresh_budget_refused = false;
        for current_tick_only in [true, false] {
            if !current_tick_only && fresh_budget_refused {
                break;
            }
            let mut producers_without_send = 0;
            while producers_without_send < self.producers.len() {
                let index = self.cursor;
                self.cursor = (self.cursor + 1) % self.producers.len();
                let Some(pending_index) =
                    self.producers[index].pending.iter().position(|pending| {
                        pending.last_sent != Some(now)
                            && (pending.emitted == now) == current_tick_only
                    })
                else {
                    producers_without_send += 1;
                    continue;
                };
                let cost = single_fact_wire_upper_bound(
                    &self.producers[index].pending[pending_index].fact,
                );
                if budget_used + cost > VISUAL_TICK_WIRE_LIMIT {
                    budget_refused = true;
                    fresh_budget_refused |= current_tick_only;
                    producers_without_send += 1;
                    continue;
                }

                let mut pending = self.producers[index]
                    .pending
                    .remove(pending_index)
                    .expect("the selected visual fact still exists");
                selected.push(pending.fact.clone());
                metrics.visual_selected += 1;
                budget_used += cost;
                pending.copies_left -= 1;
                pending.last_sent = Some(now);
                if pending.copies_left > 0 {
                    self.producers[index].pending.push_back(pending);
                }
                producers_without_send = 0;
            }
        }
        if budget_refused {
            metrics.visual_budget_deferred_producers += self
                .producers
                .iter()
                .filter(|producer| {
                    producer
                        .pending
                        .iter()
                        .any(|pending| pending.last_sent != Some(now))
                })
                .count() as u64;
        }

        self.producers
            .retain(|producer| !producer.pending.is_empty());
        if self.producers.is_empty() {
            self.cursor = 0;
        } else {
            self.cursor %= self.producers.len();
        }

        let mut batches = Vec::new();
        let mut current = FireVisualBatch { facts: Vec::new() };
        for fact in selected {
            current.facts.push(fact);
            if batch_wire_upper_bound(&current) > VISUAL_BATCH_WIRE_LIMIT {
                let fact = current
                    .facts
                    .pop()
                    .expect("the just-added visual fact exists");
                debug_assert!(!current.facts.is_empty());
                batches.push(current);
                current = FireVisualBatch { facts: vec![fact] };
                debug_assert!(batch_wire_upper_bound(&current) <= VISUAL_BATCH_WIRE_LIMIT);
            }
        }
        if !current.facts.is_empty() {
            batches.push(current);
        }

        for batch in &batches {
            let bytes = batch_wire_upper_bound(batch);
            metrics.max_batch_wire_bytes = metrics.max_batch_wire_bytes.max(bytes);
        }
        batches
    }
}

fn single_fact_wire_upper_bound(fact: &FireVisualFact) -> usize {
    batch_wire_upper_bound(&FireVisualBatch {
        facts: vec![fact.clone()],
    })
}

fn batch_wire_upper_bound(batch: &FireVisualBatch) -> usize {
    let mut worst_case = batch.clone();
    for fact in &mut worst_case.facts {
        if let FireVisualFact::Fire(event) = fact {
            event.shooter = WORST_CASE_MAPPED_ENTITY;
        }
    }
    bincode::serde::encode_to_vec(&worst_case, bincode::config::standard())
        .expect("registered shot facts serialize with Lightyear's bincode configuration")
        .len()
        + MESSAGE_NET_ID_BYTES
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeliveryClass {
    Automatic,
    Single,
}

impl From<FireMechanism> for DeliveryClass {
    fn from(mechanism: FireMechanism) -> Self {
        match mechanism {
            FireMechanism::Automatic => Self::Automatic,
            FireMechanism::Single => Self::Single,
        }
    }
}

struct ShotRoute {
    owner: Option<Entity>,
    class: DeliveryClass,
}

struct PrivateDamage {
    owner: Entity,
    shot: crate::ShotId,
    confirm: DamageConfirm,
}

#[derive(Resource, Default)]
struct ShotTransport {
    visual: VisualQueue,
    reliable_public: VecDeque<FireVisualFact>,
    private_damage: VecDeque<PrivateDamage>,
    routes: bevy::platform::collections::HashMap<crate::ShotId, ShotRoute>,
}

impl ShotTransport {
    fn enqueue_public(
        &mut self,
        fact: FireVisualFact,
        class: DeliveryClass,
        now: Tick,
        metrics: &mut ShotTransportMetrics,
    ) {
        match (class, fact) {
            (DeliveryClass::Automatic, fact) => self.visual.enqueue(fact, now, metrics),
            (DeliveryClass::Single, fact) => {
                self.reliable_public.push_back(fact);
                metrics.reliable_public_enqueued += 1;
            }
        }
    }
}

/// Install the authority half of shot transport.
pub(super) fn install_server(app: &mut App) {
    app.init_resource::<ShotTransport>();
    app.init_resource::<ShotTransportMetrics>();
    app.add_observer(queue_fire);
    app.add_observer(queue_ricochet);
    app.add_observer(queue_terminal);
    app.add_observer(queue_damage);
    app.add_observer(forget_route_on_projectile_removal);
    app.add_systems(FixedUpdate, flush_shot_transport.after(GameplaySet));
}

fn queue_fire(
    fire: On<FireShell>,
    timeline: Res<LocalTimeline>,
    controlled: Query<&ControlledBy>,
    combatants: Query<&crate::CombatantId>,
    mut transport: ResMut<ShotTransport>,
    mut metrics: ResMut<ShotTransportMetrics>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let Some(source) = fire.shooter else { return };
    let Some(shot) = fire.shot else {
        warn!("server: attributed FireShell has no spawn-time ShotId");
        return;
    };
    let Ok(&combatant) = combatants.get(source.tank) else {
        warn!("server: firing tank {} has no CombatantId", source.tank);
        return;
    };
    let Ok(weapon) = u8::try_from(source.weapon) else {
        warn!("server: weapon slot {} exceeds the wire u8", source.weapon);
        return;
    };
    if shot.combatant != combatant || shot.weapon != weapon || shot.fire_tick != timeline.tick().0 {
        warn!(
            "server: FireShell ShotId disagrees with its spawn facts: {shot:?}, combatant={combatant:?}, weapon={weapon}, tick={}",
            timeline.tick().0
        );
        return;
    }

    let class = DeliveryClass::from(fire.mechanism);
    let owner = controlled
        .get(source.tank)
        .ok()
        .map(|control| control.owner);
    if transport
        .routes
        .insert(shot, ShotRoute { owner, class })
        .is_some()
    {
        metrics.route_conflicts += 1;
        warn!("server: duplicate ShotId route replaced: {shot:?}");
    }

    let event = FireEvent {
        origin: fire.origin,
        direction: fire.direction.as_vec3(),
        speed: fire.speed,
        caliber: fire.caliber,
        mass: fire.mass,
        mechanism: fire.mechanism,
        tracer: fire.tracer,
        shooter: source.tank,
        combatant,
        weapon,
        fire_tick: Tick(shot.fire_tick),
    };
    crate::shot_trace::record(&mut shot_trace, "fire", timeline.tick().0, shot, || {
        json!({
            "o": [event.origin.x, event.origin.y, event.origin.z],
            "tr": event.tracer,
            "cal": event.caliber,
        })
    });
    transport.enqueue_public(
        FireVisualFact::Fire(event),
        class,
        timeline.tick(),
        &mut metrics,
    );
}

fn queue_ricochet(
    ricochet: On<ShellRicochet>,
    timeline: Res<LocalTimeline>,
    mut transport: ResMut<ShotTransport>,
    mut metrics: ResMut<ShotTransportMetrics>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let Some(class) = transport
        .routes
        .get(&ricochet.shot)
        .map(|route| route.class)
    else {
        return;
    };
    crate::shot_trace::record(
        &mut shot_trace,
        "kf",
        timeline.tick().0,
        ricochet.shot,
        || json!({ "seq": ricochet.sequence }),
    );
    let keyframe = RicochetKeyframe {
        shot: ricochet.shot,
        origin: ricochet.origin,
        direction: ricochet.direction,
        speed: ricochet.speed,
        bounce_tick: timeline.tick(),
        sequence: ricochet.sequence,
    };
    transport.enqueue_public(
        FireVisualFact::Ricochet(keyframe),
        class,
        timeline.tick(),
        &mut metrics,
    );
}

fn queue_terminal(
    terminal: On<ShellTerminal>,
    timeline: Res<LocalTimeline>,
    mut transport: ResMut<ShotTransport>,
    mut metrics: ResMut<ShotTransportMetrics>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let Some(class) = transport
        .routes
        .get(&terminal.shot)
        .map(|route| route.class)
    else {
        return;
    };
    crate::shot_trace::record(
        &mut shot_trace,
        "cf",
        timeline.tick().0,
        terminal.shot,
        || json!({ "pen": terminal.penetrated, "ab": terminal.after_bounces }),
    );
    let confirm = ImpactConfirm {
        shot: terminal.shot,
        position: terminal.position,
        normal: terminal.normal,
        penetrated: terminal.penetrated,
        impact_tick: timeline.tick(),
        after_bounces: terminal.after_bounces,
    };
    transport.enqueue_public(
        FireVisualFact::Impact(confirm),
        class,
        timeline.tick(),
        &mut metrics,
    );
}

fn queue_damage(
    damage: On<ShellDamage>,
    timeline: Res<LocalTimeline>,
    mut transport: ResMut<ShotTransport>,
    mut metrics: ResMut<ShotTransportMetrics>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    crate::shot_trace::record(
        &mut shot_trace,
        "dmg",
        timeline.tick().0,
        damage.shot,
        || json!({ "hp": damage.amount }),
    );
    let Some(owner) = transport
        .routes
        .get_mut(&damage.shot)
        .and_then(|route| route.owner.take())
    else {
        return;
    };
    transport.private_damage.push_back(PrivateDamage {
        owner,
        shot: damage.shot,
        confirm: DamageConfirm {
            receipt: DamageReceipt::from(damage.shot),
            damage_tick: timeline.tick(),
        },
    });
    metrics.private_damage_enqueued += 1;
}

fn forget_route_on_projectile_removal(
    remove: On<Remove, Projectile>,
    shots: Query<&Shot>,
    mut transport: ResMut<ShotTransport>,
) {
    if let Ok(shot) = shots.get(remove.entity) {
        transport.routes.remove(&shot.0);
    }
}

fn flush_shot_transport(
    servers: Query<&Server>,
    timeline: Res<LocalTimeline>,
    mut transport: ResMut<ShotTransport>,
    mut metrics: ResMut<ShotTransportMetrics>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    mut sender: ServerMultiMessageSender,
) {
    let Ok(server) = servers.single() else { return };
    let now = timeline.tick();
    let visual_queue_before = transport.visual.len();
    let reliable_queued = transport.reliable_public.len();
    let private_queued = transport.private_damage.len();
    let public_recipient_count = server.collection().iter().count();
    let selected_before = metrics.visual_selected;
    let expired_before = metrics.visual_expired;
    let deferred_before = metrics.visual_budget_deferred_producers;
    let public_no_recipient_before = metrics.public_no_recipient_facts;
    let private_no_recipient_before = metrics.private_damage_no_recipient_facts;
    let errors_before = metrics.send_call_errors;
    let error_facts_before = metrics.send_call_error_facts;
    let mut visual_facts_send_accepted = 0_u64;
    let mut visual_batches_send_accepted = 0_u64;
    let mut visual_wire_bytes_send_accepted = 0_u64;

    let reliable = core::mem::take(&mut transport.reliable_public);
    for fact in reliable {
        if public_recipient_count == 0 {
            metrics.public_no_recipient_facts += 1;
            continue;
        }
        let result = match &fact {
            FireVisualFact::Fire(event) => {
                sender.send::<FireEvent, OutcomeChannel>(event, server, &NetworkTarget::All)
            }
            FireVisualFact::Ricochet(keyframe) => sender.send::<RicochetKeyframe, OutcomeChannel>(
                keyframe,
                server,
                &NetworkTarget::All,
            ),
            FireVisualFact::Impact(confirm) => {
                sender.send::<ImpactConfirm, OutcomeChannel>(confirm, server, &NetworkTarget::All)
            }
        };
        if let Err(err) = result {
            metrics.send_call_errors += 1;
            metrics.send_call_error_facts += 1;
            error!("server: reliable shot fact could not enter transport: {err}");
            continue;
        }
        metrics.reliable_public_send_accepted_facts += 1;
        record_send(
            &mut shot_trace,
            now,
            fact.shot_id(),
            fact.authority_tick(),
            fact_stream(&fact),
            true,
            None,
            fact_sequence(&fact),
            public_recipient_count,
        );
    }

    let private = core::mem::take(&mut transport.private_damage);
    for damage in private {
        if !server.collection().contains(&damage.owner) {
            metrics.private_damage_no_recipient_facts += 1;
            continue;
        }
        if let Err(err) =
            sender.send_to_entities::<DamageConfirm, DamageChannel>(&damage.confirm, [damage.owner])
        {
            metrics.send_call_errors += 1;
            metrics.send_call_error_facts += 1;
            error!("server: private damage fact could not enter transport: {err}");
            continue;
        }
        metrics.private_damage_send_accepted_facts += 1;
        record_send(
            &mut shot_trace,
            now,
            damage.shot,
            damage.confirm.damage_tick,
            "dmg",
            true,
            None,
            None,
            1,
        );
    }

    for batch in transport.visual.drain_batches(now, &mut metrics) {
        let bytes = batch_wire_upper_bound(&batch);
        let fact_count = batch.facts.len() as u64;
        if public_recipient_count == 0 {
            metrics.public_no_recipient_facts += fact_count;
            continue;
        }
        if let Err(err) =
            sender.send::<FireVisualBatch, FireChannel>(&batch, server, &NetworkTarget::All)
        {
            metrics.send_call_errors += 1;
            metrics.send_call_error_facts += fact_count;
            error!("server: visual shot batch could not enter transport: {err}");
            continue;
        }
        visual_facts_send_accepted += fact_count;
        visual_batches_send_accepted += 1;
        visual_wire_bytes_send_accepted += bytes as u64;
        metrics.visual_send_accepted_facts += fact_count;
        metrics.visual_send_accepted_batches += 1;
        metrics.visual_send_accepted_wire_upper_bound_bytes += bytes as u64;
        for fact in &batch.facts {
            record_send(
                &mut shot_trace,
                now,
                fact.shot_id(),
                fact.authority_tick(),
                fact_stream(fact),
                false,
                Some(bytes),
                fact_sequence(fact),
                public_recipient_count,
            );
        }
    }

    let visual_queue_after = transport.visual.len();
    let visual_selected = metrics.visual_selected - selected_before;
    let visual_expired = metrics.visual_expired - expired_before;
    let visual_deferred = metrics.visual_budget_deferred_producers - deferred_before;
    let public_no_recipient = metrics.public_no_recipient_facts - public_no_recipient_before;
    let private_no_recipient =
        metrics.private_damage_no_recipient_facts - private_no_recipient_before;
    let send_call_errors = metrics.send_call_errors - errors_before;
    let send_call_error_facts = metrics.send_call_error_facts - error_facts_before;
    if visual_queue_before > 0
        || reliable_queued > 0
        || private_queued > 0
        || visual_selected > 0
        || visual_expired > 0
        || visual_deferred > 0
        || public_no_recipient > 0
        || private_no_recipient > 0
        || send_call_errors > 0
    {
        crate::shot_trace::record_global(&mut shot_trace, "transport", now.0, || {
            json!({
                "visual_queue_before": visual_queue_before,
                "visual_queue_after": visual_queue_after,
                "visual_selected": visual_selected,
                "visual_facts_send_accepted": visual_facts_send_accepted,
                "visual_batches_send_accepted": visual_batches_send_accepted,
                "visual_wire_bytes_send_accepted_upper_bound": visual_wire_bytes_send_accepted,
                "visual_expired": visual_expired,
                "visual_budget_deferred_producers": visual_deferred,
                "reliable_public_queued": reliable_queued,
                "private_damage_queued": private_queued,
                "public_recipient_count": public_recipient_count,
                "public_no_recipient_facts": public_no_recipient,
                "private_damage_no_recipient_facts": private_no_recipient,
                "send_call_errors": send_call_errors,
                "send_call_error_facts": send_call_error_facts,
                "visual_copy_opportunities": VISUAL_COPIES,
                "visual_ttl_ticks": VISUAL_TTL_TICKS,
                "visual_batch_wire_limit": VISUAL_BATCH_WIRE_LIMIT,
                "visual_tick_wire_limit": VISUAL_TICK_WIRE_LIMIT,
            })
        });
    }
}

fn fact_stream(fact: &FireVisualFact) -> &'static str {
    match fact {
        FireVisualFact::Fire(_) => "fire",
        FireVisualFact::Ricochet(_) => "kf",
        FireVisualFact::Impact(_) => "cf",
    }
}

fn fact_sequence(fact: &FireVisualFact) -> Option<u32> {
    match fact {
        FireVisualFact::Ricochet(keyframe) => Some(keyframe.sequence),
        FireVisualFact::Fire(_) | FireVisualFact::Impact(_) => None,
    }
}

fn record_send(
    trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    now: Tick,
    shot: crate::ShotId,
    authority_tick: Tick,
    stream: &'static str,
    reliable: bool,
    batch_bytes: Option<usize>,
    sequence: Option<u32>,
    targeted_recipients: usize,
) {
    let age = (now - authority_tick).max(0);
    crate::shot_trace::record(trace, "send", now.0, shot, || {
        json!({
            "s": stream,
            "age": age,
            "rel": reliable,
            "bb": batch_bytes,
            "seq": sequence,
            "rcpt": targeted_recipients,
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightyear::prelude::Tick;

    use crate::net::protocol::{FireEvent, FireVisualFact};

    fn fire(combatant: u64, weapon: u8, tick: u32) -> FireVisualFact {
        FireVisualFact::Fire(FireEvent {
            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            speed: 800.0,
            caliber: 0.0079,
            mass: 0.0118,
            mechanism: crate::spec::FireMechanism::Automatic,
            tracer: true,
            shooter: Entity::from_raw_u32(combatant as u32).unwrap(),
            combatant: crate::CombatantId(combatant),
            weapon,
            fire_tick: Tick(tick),
        })
    }

    #[test]
    fn thirty_tank_two_weapon_volley_keeps_every_first_copy_and_never_fragments() {
        let mut queue = VisualQueue::default();
        let mut metrics = ShotTransportMetrics::default();
        for combatant in 1..=30 {
            queue.enqueue(fire(combatant, 0, 100), Tick(100), &mut metrics);
            queue.enqueue(fire(combatant, 1, 100), Tick(100), &mut metrics);
        }

        let batches = queue.drain_batches(Tick(100), &mut metrics);
        let facts: Vec<_> = batches
            .iter()
            .flat_map(|batch| batch.facts.iter())
            .collect();

        assert_eq!(facts.len(), 60, "all first copies leave on the volley tick");
        assert!(
            batches.len() <= VISUAL_TICK_WIRE_LIMIT / VISUAL_BATCH_WIRE_LIMIT,
            "the synchronized volley stays inside the derived per-tick batch budget"
        );
        assert!(
            batches
                .iter()
                .all(|batch| batch_wire_upper_bound(batch) <= VISUAL_BATCH_WIRE_LIMIT),
            "no application batch crosses Lightyear's unfragmented-message budget"
        );
        let unique: bevy::platform::collections::HashSet<_> =
            facts.iter().map(|fact| fact.shot_id()).collect();
        assert_eq!(unique.len(), 60, "the two weapon slots remain distinct");
        assert_eq!(queue.len(), 60, "two bounded repair copies remain queued");
        let wire_upper_bound: usize = batches.iter().map(batch_wire_upper_bound).sum();
        assert!(
            wire_upper_bound <= VISUAL_TICK_WIRE_LIMIT,
            "the measured serialized-message upper bounds stay inside the admission budget"
        );
        assert_eq!(metrics.visual_selected, 60);
    }

    #[test]
    fn automatic_visual_gets_three_send_opportunities_then_leaves_no_debt() {
        let mut queue = VisualQueue::default();
        let mut metrics = ShotTransportMetrics::default();
        queue.enqueue(fire(1, 0, 100), Tick(100), &mut metrics);

        for tick in 100..=102 {
            let batches = queue.drain_batches(Tick(tick), &mut metrics);
            assert_eq!(
                batches.iter().map(|batch| batch.facts.len()).sum::<usize>(),
                1,
                "one copy leaves on each bounded send opportunity"
            );
        }
        assert!(queue.drain_batches(Tick(103), &mut metrics).is_empty());
        assert_eq!(queue.len(), 0);
        assert_eq!(metrics.visual_selected, 3);
        assert_eq!(
            metrics.visual_send_accepted_facts, 0,
            "the pure queue has no transport sender"
        );
    }

    #[test]
    fn stale_visual_expires_without_becoming_late_presentation() {
        let mut queue = VisualQueue::default();
        let mut metrics = ShotTransportMetrics::default();
        queue.enqueue(fire(1, 0, 100), Tick(100), &mut metrics);

        assert!(
            queue
                .drain_batches(Tick(100 + VISUAL_TTL_TICKS as u32 + 1), &mut metrics)
                .is_empty()
        );
        assert_eq!(queue.len(), 0);
        assert_eq!(metrics.visual_expired, 1);
    }

    #[test]
    fn one_busy_combatant_cannot_starve_other_combatants() {
        let mut queue = VisualQueue::default();
        let mut metrics = ShotTransportMetrics::default();
        for weapon in 0..80 {
            queue.enqueue(fire(1, weapon, 100), Tick(100), &mut metrics);
        }
        for combatant in 2..=30 {
            queue.enqueue(fire(combatant, 0, 100), Tick(100), &mut metrics);
        }

        let batches = queue.drain_batches(Tick(100), &mut metrics);
        let sent_combatants: bevy::platform::collections::HashSet<_> = batches
            .iter()
            .flat_map(|batch| batch.facts.iter())
            .map(|fact| fact.shot_id().combatant)
            .collect();
        assert_eq!(
            sent_combatants.len(),
            30,
            "round-robin admission gives every active combatant a first opportunity before one producer dominates"
        );
    }

    #[test]
    fn fresh_visuals_outrank_repair_copies_when_the_tick_budget_saturates() {
        let mut queue = VisualQueue::default();
        let mut metrics = ShotTransportMetrics::default();
        for combatant in 1..=30 {
            queue.enqueue(fire(combatant, 0, 100), Tick(100), &mut metrics);
            queue.enqueue(fire(combatant, 1, 100), Tick(100), &mut metrics);
        }
        queue.drain_batches(Tick(100), &mut metrics);
        for combatant in 1..=30 {
            queue.enqueue(fire(combatant, 0, 101), Tick(101), &mut metrics);
            queue.enqueue(fire(combatant, 1, 101), Tick(101), &mut metrics);
        }

        let batches = queue.drain_batches(Tick(101), &mut metrics);
        let fresh = batches
            .iter()
            .flat_map(|batch| &batch.facts)
            .filter(|fact| fact.shot_id().fire_tick == 101)
            .count();
        assert_eq!(
            fresh, 60,
            "all current-tick visuals must be admitted before older repair copies"
        );
        assert!(
            metrics.visual_budget_deferred_producers > 0,
            "saturated admission must expose the producers deferred by the byte budget"
        );
    }

    #[test]
    fn current_tick_visuals_globally_outrank_repairs() {
        let mut queue = VisualQueue::default();
        let mut metrics = ShotTransportMetrics::default();
        queue.enqueue(fire(1, 0, 100), Tick(100), &mut metrics);
        queue.drain_batches(Tick(100), &mut metrics);
        queue.enqueue(fire(2, 0, 101), Tick(101), &mut metrics);
        queue.enqueue(fire(2, 1, 101), Tick(101), &mut metrics);

        let ticks: Vec<_> = queue
            .drain_batches(Tick(101), &mut metrics)
            .into_iter()
            .flat_map(|batch| batch.facts)
            .map(|fact| fact.shot_id().fire_tick)
            .collect();

        assert_eq!(
            &ticks[..2],
            &[101, 101],
            "all current-tick facts must precede an older copy from another producer"
        );
    }

    #[test]
    fn wire_sizer_accounts_for_lightyear_mapped_entity_encoding() {
        let batch = FireVisualBatch {
            facts: (0..25).map(|weapon| fire(1, weapon, 100)).collect(),
        };
        let mut mapped = batch.clone();
        for fact in &mut mapped.facts {
            if let FireVisualFact::Fire(event) = fact {
                event.shooter = Entity::from_bits(0x8000_0000_0000_0001);
            }
        }
        let mapped_wire_bytes = bincode::serde::encode_to_vec(&mapped, bincode::config::standard())
            .unwrap()
            .len()
            + 4;

        assert_eq!(batch_wire_upper_bound(&batch), mapped_wire_bytes);
        assert!(
            mapped_wire_bytes > VISUAL_BATCH_WIRE_LIMIT,
            "the old placeholder-sized 25-fire batch must be split after recipient mapping"
        );
    }

    #[test]
    fn single_shot_start_uses_reliable_outbox_not_visual_queue() {
        let mut transport = ShotTransport::default();
        let mut metrics = ShotTransportMetrics::default();
        transport.enqueue_public(
            fire(7, 0, 100),
            DeliveryClass::Single,
            Tick(100),
            &mut metrics,
        );

        assert_eq!(transport.visual.len(), 0);
        assert_eq!(transport.reliable_public.len(), 1);
        assert_eq!(metrics.reliable_public_enqueued, 1);
    }

    #[test]
    fn single_shot_continuations_are_reliable_but_automatic_continuations_are_bounded() {
        let shot = crate::ShotId {
            combatant: crate::CombatantId(7),
            weapon: 0,
            fire_tick: 100,
        };
        let ricochet = FireVisualFact::Ricochet(RicochetKeyframe {
            shot,
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 500.0,
            bounce_tick: Tick(104),
            sequence: 0,
        });
        let impact = FireVisualFact::Impact(ImpactConfirm {
            shot,
            position: Vec3::X,
            normal: Vec3::NEG_X,
            penetrated: false,
            impact_tick: Tick(108),
            after_bounces: 1,
        });

        let mut single = ShotTransport::default();
        let mut single_metrics = ShotTransportMetrics::default();
        single.enqueue_public(
            ricochet.clone(),
            DeliveryClass::Single,
            Tick(104),
            &mut single_metrics,
        );
        single.enqueue_public(
            impact.clone(),
            DeliveryClass::Single,
            Tick(108),
            &mut single_metrics,
        );
        assert_eq!(single.reliable_public.len(), 2);
        assert_eq!(single.visual.len(), 0);

        let mut automatic = ShotTransport::default();
        let mut automatic_metrics = ShotTransportMetrics::default();
        automatic.enqueue_public(
            ricochet,
            DeliveryClass::Automatic,
            Tick(104),
            &mut automatic_metrics,
        );
        automatic.enqueue_public(
            impact,
            DeliveryClass::Automatic,
            Tick(108),
            &mut automatic_metrics,
        );
        assert_eq!(automatic.reliable_public.len(), 0);
        assert_eq!(automatic.visual.len(), 2);
        assert_eq!(automatic_metrics.visual_enqueued, 2);
    }
}
