//! The game's damage model: server transport for public shot presentation and private consequences.

use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::prelude::server::{ClientOf, Server};
use lightyear::prelude::*;
use lightyear_transport::channel::registry::ChannelKind;
use lightyear_transport::packet::message::MessageId;
use lightyear_transport::plugin::TransportSystems;
use serde_json::json;

use super::protocol::{
    DamageChannel, DamageConfirm, DamageReceipt, FireChannel, FireEvent, FireVisualBatch,
    FireVisualFact, OutcomeChannel,
};
use crate::ballistics::{
    FireShell, FlightOutcome, Projectile, ShellDamage, ShellRicochet, ShellTerminal, Shot,
};
use crate::spec::FireMechanism;

/// DERIVED STARTING DEFAULT: the emission plus the next two send opportunities gives each
/// automatic-fire visual three bounded chances without creating reliable cosmetic debt.
pub(super) const VISUAL_COPIES: u8 = 3;
/// DERIVED: sixteen 64 Hz ticks are 250 ms, matching the current client armor-outcome hold span.
const VISUAL_TTL_TICKS: i32 = 16;
/// DERIVED from Lightyear 0.30's 1,162-byte unfragmented-message ceiling (the transport's fragment
/// size at the 1,200-byte default MTU: 13 header bytes + 23 fragment-metadata bytes — a 4-byte
/// channel id, 2-byte message id, two 8-byte fragment fields, 1 compression byte — then the
/// payload's own 2-byte QUIC-varint length prefix), leaving 62 bytes of headroom after worst-case
/// postcard entity encoding and the message type id.
pub(crate) const VISUAL_BATCH_WIRE_LIMIT: usize = 1_100;
/// DERIVED STARTING DEFAULT: four maximum-size batches cover the current 30-tank, two-weapon
/// synchronized volley while bounding one recipient's automatic-fire work per server tick.
const VISUAL_TICK_WIRE_LIMIT: usize = VISUAL_BATCH_WIRE_LIMIT * 4;
/// DERIVED: Lightyear's registered `MessageNetId` is a `u16` in lightyear's QUIC-style varint
/// (1/2/4/8-byte tiers), whose largest tier for a `u16` is four bytes — the exact maximum.
const MESSAGE_NET_ID_BYTES: usize = 4;
/// DERIVED from Lightyear 0.30's `SendEntityMap`: recipient-mapped entities set bit 63 before Bevy's
/// `u64` serde representation is encoded, forcing postcard's ten-byte LEB128 `u64` tier.
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
    /// Facts expired before visual admission created a send opportunity.
    pub visual_expired_before_first_send_opportunity: u64,
    /// Facts expired after at least one, but fewer than all, send opportunities.
    pub visual_expired_after_partial_send_opportunities: u64,
    pub visual_fresh_budget_refusal_attempts: u64,
    pub visual_repair_budget_refusal_attempts: u64,
    pub visual_budget_deferred_producers: u64,
    pub max_batch_wire_bytes: usize,
    pub reliable_public_enqueued: u64,
    pub reliable_public_send_accepted_facts: u64,
    /// Sum across connected links of reliable outcome messages sent but not yet acknowledged.
    pub reliable_outcome_sent_unacked_messages: u64,
    pub max_reliable_outcome_sent_unacked_messages: u64,
    /// Greatest first-send age among those outcome messages, in authority ticks.
    pub oldest_reliable_outcome_sent_unacked_ticks: u32,
    /// Sum across connected links of reliable damage messages sent but not yet acknowledged.
    pub reliable_damage_sent_unacked_messages: u64,
    pub max_reliable_damage_sent_unacked_messages: u64,
    /// Greatest first-send age among those damage messages, in authority ticks.
    pub oldest_reliable_damage_sent_unacked_ticks: u32,
    pub public_no_recipient_facts: u64,
    pub private_damage_no_recipient_facts: u64,
}

#[derive(Clone, Copy)]
enum ReliableOutboxChannel {
    Outcome,
    Damage,
}

#[derive(Default)]
struct ReliableLinkOutbox {
    outcomes: bevy::platform::collections::HashMap<MessageId, Tick>,
    damages: bevy::platform::collections::HashMap<MessageId, Tick>,
}

#[derive(Resource, Default)]
struct ReliableOutboxLedger {
    links: bevy::platform::collections::HashMap<Entity, ReliableLinkOutbox>,
}

#[derive(Default, PartialEq, Eq, Debug)]
struct ReliableOutboxSummary {
    outcome_count: u64,
    damage_count: u64,
    oldest_outcome_ticks: u32,
    oldest_damage_ticks: u32,
}

impl ReliableOutboxLedger {
    /// Ledger invariant: a repeated unacknowledged `MessageId` is a transport retransmission.
    fn observe(
        &mut self,
        link: Entity,
        channel: ReliableOutboxChannel,
        now: Tick,
        sent: &[MessageId],
        acknowledged: &[MessageId],
    ) {
        let link = self.links.entry(link).or_default();
        let pending = match channel {
            ReliableOutboxChannel::Outcome => &mut link.outcomes,
            ReliableOutboxChannel::Damage => &mut link.damages,
        };
        for &message in acknowledged {
            pending.remove(&message);
        }
        for &message in sent {
            pending.entry(message).or_insert(now);
        }
    }

    fn forget_link(&mut self, link: Entity) {
        self.links.remove(&link);
    }

    fn summary(&self, now: Tick) -> ReliableOutboxSummary {
        let mut summary = ReliableOutboxSummary::default();
        for link in self.links.values() {
            summary.outcome_count += link.outcomes.len() as u64;
            summary.damage_count += link.damages.len() as u64;
            summary.oldest_outcome_ticks = summary.oldest_outcome_ticks.max(
                link.outcomes
                    .values()
                    .map(|first_sent| (now - *first_sent).max(0) as u32)
                    .max()
                    .unwrap_or_default(),
            );
            summary.oldest_damage_ticks = summary.oldest_damage_ticks.max(
                link.damages
                    .values()
                    .map(|first_sent| (now - *first_sent).max(0) as u32)
                    .max()
                    .unwrap_or_default(),
            );
        }
        summary
    }
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
            producer.pending.retain(|pending| {
                let live = now - pending.emitted <= VISUAL_TTL_TICKS;
                if !live {
                    metrics.visual_expired += 1;
                    if pending.last_sent.is_some() {
                        metrics.visual_expired_after_partial_send_opportunities += 1;
                    } else {
                        metrics.visual_expired_before_first_send_opportunity += 1;
                    }
                }
                live
            });
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
        for current_tick_only in [true, false] {
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
                    if current_tick_only {
                        metrics.visual_fresh_budget_refusal_attempts += 1;
                    } else {
                        metrics.visual_repair_budget_refusal_attempts += 1;
                    }
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
                assert!(
                    !current.facts.is_empty(),
                    "each visual fact fits one batch by itself"
                );
                batches.push(current);
                current = FireVisualBatch { facts: vec![fact] };
                assert!(
                    batch_wire_upper_bound(&current) <= VISUAL_BATCH_WIRE_LIMIT,
                    "each visual fact fits the unfragmented batch budget"
                );
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
    postcard::to_allocvec(&worst_case)
        .expect("registered shot facts serialize with Lightyear's postcard encoding")
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
            (_, fact @ FireVisualFact::Outcome(_)) => {
                self.reliable_public.push_back(fact);
                metrics.reliable_public_enqueued += 1;
            }
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
    app.init_resource::<ReliableOutboxLedger>();
    app.add_observer(queue_fire);
    app.add_observer(queue_path);
    app.add_observer(queue_ricochet);
    app.add_observer(queue_terminal);
    app.add_observer(queue_damage);
    app.add_observer(forget_route_on_projectile_removal);
    app.add_systems(
        FixedPostUpdate,
        flush_shot_transport.in_set(crate::state::SimPhase::Publish),
    );
    app.add_systems(
        PostUpdate,
        observe_reliable_outbox.after(TransportSystems::Send),
    );
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
    // Unkeyed tool/environment emissions have no player presentation route.
    let Some(source) = fire.shooter else {
        assert!(
            fire.shot.is_none(),
            "a network-keyed authority fire has its source from spawn"
        );
        return;
    };
    let shot = fire
        .shot
        .expect("an attributed authority fire has its ShotId stamped at spawn");
    let &combatant = combatants
        .get(source.tank)
        .expect("an authority firing tank has its CombatantId from spawn");
    let weapon = u8::try_from(source.weapon)
        .expect("the tank load gate limits weapon slots to the wire's u8");
    assert_eq!(
        (shot.combatant, shot.weapon, shot.fire_tick),
        (combatant, weapon, timeline.tick().0),
        "the authority's spawn-time ShotId agrees with its firing source and tick"
    );

    let class = DeliveryClass::from(fire.mechanism);
    // Bots are ownerless: public facts still broadcast, but no private confirmation is owed.
    let owner = controlled
        .get(source.tank)
        .ok()
        .map(|control| control.owner);
    assert!(
        transport.routes.insert(shot, ShotRoute { owner }).is_none(),
        "an authority ShotId has exactly one spawn-time route: {shot:?}"
    );

    let event = FireEvent {
        birth: fire.birth,

        origin: fire.origin,
        direction: fire.direction.as_vec3(),
        mechanism: fire.mechanism,
        tracer: fire.tracer,
        shooter: source.tank,
        combatant,
        weapon,
        round: source.round,
        fuze_mode: fire.fuze_mode,
        fire_tick: Tick(shot.fire_tick),
    };
    crate::shot_trace::record(&mut shot_trace, "fire", timeline.tick().0, shot, || {
        json!({
            "o": [event.origin.x, event.origin.y, event.origin.z],
            "tr": event.tracer,
            "cal": fire.caliber,
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
    transport: Res<ShotTransport>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    assert!(
        transport.routes.contains_key(&ricochet.shot),
        "an authority ricochet is emitted before its projectile's route is removed: {:?}",
        ricochet.shot
    );
    crate::shot_trace::record(
        &mut shot_trace,
        "kf",
        timeline.tick().0,
        ricochet.shot,
        || {
            json!({ "seq": ricochet.sequence, "o": ricochet.origin.to_array(),
            "d": ricochet.direction.to_array(), "speed": ricochet.speed })
        },
    );
}

fn queue_terminal(
    terminal: On<ShellTerminal>,
    timeline: Res<LocalTimeline>,
    transport: Res<ShotTransport>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    assert!(
        transport.routes.contains_key(&terminal.shot),
        "an authority terminal is emitted before its projectile's route is removed: {:?}",
        terminal.shot
    );
    crate::shot_trace::record(
        &mut shot_trace,
        "cf",
        timeline.tick().0,
        terminal.shot,
        || {
            json!({ "pen": terminal.verdict.entered(), "ab": terminal.after_bounces,
            "p": terminal.position.to_array(), "n": terminal.normal.to_array() })
        },
    );
}

/// The game's damage model publishes immutable paths reliably for every weapon class.
/// A complex crossing may exceed an unfragmented visual batch; the reliable lane fragments it.
fn queue_path(
    outcome: On<FlightOutcome>,
    mut transport: ResMut<ShotTransport>,
    mut metrics: ResMut<ShotTransportMetrics>,
    timeline: Res<LocalTimeline>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let (impact, ordinal) = match outcome.impact {
        Some(crate::ballistics::path::PathImpact::Bounce { ordinal, .. }) => {
            ("bounce", Some(ordinal))
        }
        Some(crate::ballistics::path::PathImpact::Through { ordinal, .. }) => {
            ("through", Some(ordinal))
        }
        Some(crate::ballistics::path::PathImpact::Stopped { ordinal, .. }) => {
            ("stopped", Some(ordinal))
        }
        Some(crate::ballistics::path::PathImpact::Ground { .. }) => ("ground", None),
        None => ("state", None),
    };
    crate::shot_trace::record(
        &mut shot_trace,
        "path_tx",
        timeline.tick().0,
        outcome.shot,
        || {
            json!({ "seq": outcome.sequence, "start": outcome.tick, "fraction": outcome.fraction,
            "seconds": outcome.seconds(), "impact": impact, "ordinal": ordinal })
        },
    );
    transport
        .reliable_public
        .push_back(FireVisualFact::Outcome(outcome.clone()));
    metrics.reliable_public_enqueued += 1;
}

fn queue_damage(
    damage: On<ShellDamage>,
    timeline: Res<LocalTimeline>,
    mut transport: ResMut<ShotTransport>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    crate::shot_trace::record(
        &mut shot_trace,
        "dmg",
        timeline.tick().0,
        damage.shot,
        || json!({ "hp": damage.amount }),
    );
    let route = transport
        .routes
        .get_mut(&damage.shot)
        .expect("authority dose is reported before its projectile's route is removed");
    // A bot has no recipient; an already-confirmed shot has spent its one private receipt.
    let Some(owner) = route.owner.take() else {
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
}

fn forget_route_on_projectile_removal(
    remove: On<Remove, Projectile>,
    shots: Query<&Shot>,
    mut transport: ResMut<ShotTransport>,
) {
    // A projectile without a `Shot` is an unkeyed tool or environment emission: it never had a
    // route (`queue_fire`).
    if let Ok(shot) = shots.get(remove.entity) {
        transport.routes.remove(&shot.0);
    }
}

fn observe_reliable_outbox(
    timeline: Res<LocalTimeline>,
    connections: Query<(Entity, &Transport), (With<ClientOf>, With<Connected>)>,
    mut ledger: ResMut<ReliableOutboxLedger>,
    mut metrics: ResMut<ShotTransportMetrics>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
) {
    let disconnected: Vec<_> = ledger
        .links
        .keys()
        .copied()
        .filter(|entity| connections.get(*entity).is_err())
        .collect();
    for entity in disconnected {
        ledger.forget_link(entity);
    }
    let now = timeline.tick();
    for (entity, transport) in &connections {
        if let Some(sender) = transport.channel_send(ChannelKind::of::<OutcomeChannel>()) {
            ledger.observe(
                entity,
                ReliableOutboxChannel::Outcome,
                now,
                sender.messages_sent(),
                sender.message_acks(),
            );
        }
        if let Some(sender) = transport.channel_send(ChannelKind::of::<DamageChannel>()) {
            ledger.observe(
                entity,
                ReliableOutboxChannel::Damage,
                now,
                sender.messages_sent(),
                sender.message_acks(),
            );
        }
    }
    let summary = ledger.summary(now);
    let previous = ReliableOutboxSummary {
        outcome_count: metrics.reliable_outcome_sent_unacked_messages,
        damage_count: metrics.reliable_damage_sent_unacked_messages,
        oldest_outcome_ticks: metrics.oldest_reliable_outcome_sent_unacked_ticks,
        oldest_damage_ticks: metrics.oldest_reliable_damage_sent_unacked_ticks,
    };
    metrics.reliable_outcome_sent_unacked_messages = summary.outcome_count;
    metrics.max_reliable_outcome_sent_unacked_messages = metrics
        .max_reliable_outcome_sent_unacked_messages
        .max(summary.outcome_count);
    metrics.oldest_reliable_outcome_sent_unacked_ticks = summary.oldest_outcome_ticks;
    metrics.reliable_damage_sent_unacked_messages = summary.damage_count;
    metrics.max_reliable_damage_sent_unacked_messages = metrics
        .max_reliable_damage_sent_unacked_messages
        .max(summary.damage_count);
    metrics.oldest_reliable_damage_sent_unacked_ticks = summary.oldest_damage_ticks;
    if summary != previous {
        crate::shot_trace::record_global(&mut shot_trace, "reliable_outbox", now.0, || {
            json!({
                "outcome_sent_unacked_messages": summary.outcome_count,
                "damage_sent_unacked_messages": summary.damage_count,
                "oldest_outcome_sent_unacked_ticks": summary.oldest_outcome_ticks,
                "oldest_damage_sent_unacked_ticks": summary.oldest_damage_ticks,
                "max_outcome_sent_unacked_messages": metrics.max_reliable_outcome_sent_unacked_messages,
                "max_damage_sent_unacked_messages": metrics.max_reliable_damage_sent_unacked_messages,
            })
        });
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
    let server = servers
        .single()
        .expect("shot transport is installed with exactly one authority server");
    let now = timeline.tick();
    let visual_queue_before = transport.visual.len();
    let reliable_queued = transport.reliable_public.len();
    let private_queued = transport.private_damage.len();
    let public_recipient_count = server.collection().iter().count();
    let selected_before = metrics.visual_selected;
    let expired_before = metrics.visual_expired;
    let expired_before_first_send_opportunity_before =
        metrics.visual_expired_before_first_send_opportunity;
    let expired_after_partial_send_opportunities_before =
        metrics.visual_expired_after_partial_send_opportunities;
    let fresh_budget_refusal_attempts_before = metrics.visual_fresh_budget_refusal_attempts;
    let repair_budget_refusal_attempts_before = metrics.visual_repair_budget_refusal_attempts;
    let deferred_before = metrics.visual_budget_deferred_producers;
    let public_no_recipient_before = metrics.public_no_recipient_facts;
    let private_no_recipient_before = metrics.private_damage_no_recipient_facts;
    let mut visual_facts_send_accepted = 0_u64;
    let mut visual_batches_send_accepted = 0_u64;
    let mut visual_wire_bytes_send_accepted = 0_u64;

    let reliable = core::mem::take(&mut transport.reliable_public);
    for fact in reliable {
        // An empty server (or bots alone) has no observer to receive public facts.
        if public_recipient_count == 0 {
            metrics.public_no_recipient_facts += 1;
            continue;
        }
        let result = match &fact {
            FireVisualFact::Fire(event) => {
                sender.send::<FireEvent, OutcomeChannel>(event, server, &NetworkTarget::All)
            }
            FireVisualFact::Outcome(outcome) => {
                sender.send::<FlightOutcome, OutcomeChannel>(outcome, server, &NetworkTarget::All)
            }
        };
        result.expect(
            "registered authority facts serialize into each live link's unbounded transport queue",
        );
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
        // The owner may disconnect after the receipt was queued in the sim tick.
        if !server.collection().contains(&damage.owner) {
            metrics.private_damage_no_recipient_facts += 1;
            continue;
        }
        sender.send_to_entities::<DamageConfirm, DamageChannel>(&damage.confirm, [damage.owner])
            .expect("a registered private receipt serializes into its live link's unbounded transport queue");
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
        // An empty server (or bots alone) has no observer to receive public facts.
        if public_recipient_count == 0 {
            metrics.public_no_recipient_facts += fact_count;
            continue;
        }
        sender.send::<FireVisualBatch, FireChannel>(&batch, server, &NetworkTarget::All)
            .expect("registered visual batches serialize into each live link's unbounded transport queue");
        visual_facts_send_accepted += fact_count;
        visual_batches_send_accepted += 1;
        visual_wire_bytes_send_accepted += bytes as u64;
        metrics.visual_send_accepted_facts += fact_count;
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
    let visual_expired_before_first_send_opportunity = metrics
        .visual_expired_before_first_send_opportunity
        - expired_before_first_send_opportunity_before;
    let visual_expired_after_partial_send_opportunities = metrics
        .visual_expired_after_partial_send_opportunities
        - expired_after_partial_send_opportunities_before;
    let visual_fresh_budget_refusal_attempts =
        metrics.visual_fresh_budget_refusal_attempts - fresh_budget_refusal_attempts_before;
    let visual_repair_budget_refusal_attempts =
        metrics.visual_repair_budget_refusal_attempts - repair_budget_refusal_attempts_before;
    let visual_deferred = metrics.visual_budget_deferred_producers - deferred_before;
    let public_no_recipient = metrics.public_no_recipient_facts - public_no_recipient_before;
    let private_no_recipient =
        metrics.private_damage_no_recipient_facts - private_no_recipient_before;
    if visual_queue_before > 0
        || reliable_queued > 0
        || private_queued > 0
        || visual_selected > 0
        || visual_expired > 0
        || visual_fresh_budget_refusal_attempts > 0
        || visual_repair_budget_refusal_attempts > 0
        || visual_deferred > 0
        || public_no_recipient > 0
        || private_no_recipient > 0
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
                "visual_expired_before_first_send_opportunity": visual_expired_before_first_send_opportunity,
                "visual_expired_after_partial_send_opportunities": visual_expired_after_partial_send_opportunities,
                "visual_fresh_budget_refusal_attempts": visual_fresh_budget_refusal_attempts,
                "visual_repair_budget_refusal_attempts": visual_repair_budget_refusal_attempts,
                "visual_budget_deferred_producers": visual_deferred,
                "reliable_public_queued": reliable_queued,
                "private_damage_queued": private_queued,
                "public_recipient_count": public_recipient_count,
                "public_no_recipient_facts": public_no_recipient,
                "private_damage_no_recipient_facts": private_no_recipient,
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
        FireVisualFact::Outcome(_) => "path",
    }
}

fn fact_sequence(fact: &FireVisualFact) -> Option<u32> {
    match fact {
        FireVisualFact::Outcome(outcome) => Some(outcome.sequence),
        FireVisualFact::Fire(_) => None,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one trace row: each argument is a separate column of the send record"
)]
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
            birth: Vec3::ZERO,

            origin: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            mechanism: crate::spec::FireMechanism::Automatic,
            tracer: true,
            shooter: Entity::from_raw_u32(combatant as u32).unwrap(),
            combatant: crate::CombatantId(combatant),
            weapon,
            round: 0,
            fuze_mode: 0,
            fire_tick: Tick(tick),
        })
    }

    /// The game's damage model: the authority's fire event carries the fuze mode the shell was
    /// fired in, through the real wire codec, and the receiver binds its own copy of the round in
    /// that mode, its burst on that mode's delay; a mode index the fuze does not list binds no
    /// round. A replica relies on this to fly the shot the authority fired: a mutant that wrote
    /// the default mode, or bound a refused index to the default, gives the two ends different
    /// bursts.
    fn authority_fire_fixture() -> (World, impl Fn() -> FireShell) {
        let mut world = World::new();
        world.init_resource::<LocalTimeline>();
        world.init_resource::<ShotTransport>();
        world.init_resource::<ShotTransportMetrics>();
        world.add_observer(queue_fire);
        let combatant = crate::CombatantId(7);
        let tank = world.spawn(combatant).id();
        // Fixture data: any validated weapon's first round exercises the route invariant.
        let round = crate::bake::selected_spec()
            .weapons
            .values()
            .next()
            .unwrap()
            .resolved_round()
            .unwrap();
        let fire = move || {
            FireShell::of_round(
                &round,
                Vec3::ZERO,
                Dir3::NEG_Z,
                FireMechanism::Single,
                true,
                Some(crate::ballistics::ShotSource {
                    tank,
                    weapon: 0,
                    round: 0,
                }),
                Some(crate::ShotId {
                    combatant,
                    weapon: 0,
                    fire_tick: 0,
                }),
            )
        };
        (world, fire)
    }

    /// The game's damage model: replicas depend on an identity on every attributed authority
    /// fire. Silently dropping the event would leave them missing a real authority shot.
    #[test]
    #[should_panic(expected = "an attributed authority fire has its ShotId stamped at spawn")]
    fn an_attributed_fire_without_its_spawn_identity_is_a_bug() {
        let (mut world, fire) = authority_fire_fixture();
        let mut fire = fire();
        fire.shot = None;
        world.trigger(fire);
    }

    /// The game's damage model: a keyed event cannot skip routing as an unattributed tool
    /// emission; a network identity always carries the source that owns its presentation facts.
    #[test]
    #[should_panic(expected = "a network-keyed authority fire has its source from spawn")]
    fn a_keyed_fire_without_its_source_is_a_bug_at_emission() {
        let (mut world, fire) = authority_fire_fixture();
        let mut fire = fire();
        fire.shooter = None;
        world.trigger(fire);
    }

    /// The game's damage model: a genuinely unkeyed tool emission owes no network route or
    /// announcement. Treating that absence as corruption would break those tool callers.
    #[test]
    fn an_unkeyed_tool_emission_owes_no_transport_fact() {
        let (mut world, fire) = authority_fire_fixture();
        let mut fire = fire();
        fire.shooter = None;
        fire.shot = None;
        world.trigger(fire);
        let transport = world.resource::<ShotTransport>();
        assert!(transport.routes.is_empty());
        assert!(transport.reliable_public.is_empty());
        assert_eq!(transport.visual.len(), 0);
    }

    /// The game's damage model: one ShotId owns one route and one announcement. Replacing a
    /// live route would announce a duplicate and could send its private receipt to another owner.
    #[test]
    #[should_panic(expected = "an authority ShotId has exactly one spawn-time route")]
    fn a_duplicate_spawn_identity_is_a_bug() {
        let (mut world, fire) = authority_fire_fixture();
        world.trigger(fire());
        world.trigger(fire());
    }

    #[test]
    fn a_fire_event_carries_the_fired_fuze_mode() {
        const TICK: u16 = 40;
        let rounds = ["88mm_APHE_ring", "88mm_HE"]
            .map(|key| crate::tank::WeaponRound::resolve(key).expect("a shipped round"));
        let fuzed = &rounds[1];
        let picked = crate::command::RoundStep::Next
            .apply(fuzed.round.fuze_mode, fuzed.fuze_modes().len())
            .unwrap();
        let picked_delay = fuzed.fuze_modes()[usize::from(picked)].delay_s;

        let mut world = World::new();
        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(TICK.into());
        world.insert_resource(timeline);
        world.init_resource::<ShotTransport>();
        world.init_resource::<ShotTransportMetrics>();
        world.add_observer(queue_fire);
        let combatant = crate::CombatantId(3);
        let tank = world.spawn(combatant).id();
        world.trigger(FireShell::of_round(
            &fuzed.in_mode(picked).expect("a listed mode"),
            Vec3::ZERO,
            Dir3::NEG_Z,
            FireMechanism::Single,
            true,
            Some(crate::ballistics::ShotSource {
                tank,
                weapon: 0,
                round: 1,
            }),
            Some(crate::ShotId {
                combatant,
                weapon: 0,
                fire_tick: TICK.into(),
            }),
        ));
        let queued = world.resource::<ShotTransport>().reliable_public.front();
        let Some(FireVisualFact::Fire(sent)) = queued else {
            panic!("the authority queues one fire event");
        };
        let bytes = postcard::to_allocvec(sent).expect("postcard encodes the event");
        let received: FireEvent = postcard::from_bytes(&bytes).expect("postcard decodes it");
        assert_eq!(received.fuze_mode, picked);
        let bound = rounds[usize::from(received.round)]
            .in_mode(received.fuze_mode)
            .expect("the receiver binds the fired mode");
        assert_eq!(bound.burst.map(|burst| burst.delay), Some(picked_delay));
        let unlisted = u8::try_from(fuzed.fuze_modes().len()).unwrap();
        assert_eq!(
            fuzed.in_mode(unlisted),
            None,
            "an unlisted mode binds no round"
        );
        assert_eq!(fuzed.in_mode(u8::MAX), None);
        assert_eq!(rounds[0].in_mode(1), None, "past a one-mode fuze's list");
        let inert = crate::tank::WeaponRound::resolve("88mm_AP").expect("a shipped round");
        assert!(inert.in_mode(0).is_some() && inert.in_mode(1).is_none());
    }

    #[test]
    fn reliable_outbox_keeps_first_send_age_and_channel_message_scopes_separate() {
        let link = Entity::from_raw_u32(1).unwrap();
        let mut ledger = ReliableOutboxLedger::default();
        ledger.observe(
            link,
            ReliableOutboxChannel::Outcome,
            Tick(100),
            &[MessageId(7), MessageId(8)],
            &[],
        );
        ledger.observe(
            link,
            ReliableOutboxChannel::Damage,
            Tick(102),
            &[MessageId(7)],
            &[],
        );
        ledger.observe(
            link,
            ReliableOutboxChannel::Outcome,
            Tick(110),
            &[MessageId(7)],
            &[MessageId(8)],
        );

        assert_eq!(
            ledger.summary(Tick(112)),
            ReliableOutboxSummary {
                outcome_count: 1,
                damage_count: 1,
                oldest_outcome_ticks: 12,
                oldest_damage_ticks: 10,
            }
        );

        ledger.observe(
            link,
            ReliableOutboxChannel::Damage,
            Tick(113),
            &[],
            &[MessageId(7)],
        );
        assert_eq!(ledger.summary(Tick(113)).damage_count, 0);
        ledger.forget_link(link);
        assert_eq!(ledger.summary(Tick(113)), ReliableOutboxSummary::default());
    }

    #[test]
    fn reliable_outbox_observer_tracks_connected_server_links_and_forgets_disconnects() {
        let mut app = App::new();
        app.init_resource::<LocalTimeline>();
        app.init_resource::<ReliableOutboxLedger>();
        app.init_resource::<ShotTransportMetrics>();
        app.add_systems(Update, observe_reliable_outbox);
        let link = app
            .world_mut()
            .spawn((
                ClientOf,
                RemoteId(PeerId::Netcode(1)),
                Connected,
                Transport::new(PriorityConfig::default()),
            ))
            .id();
        app.world_mut()
            .resource_mut::<ReliableOutboxLedger>()
            .observe(
                link,
                ReliableOutboxChannel::Outcome,
                Tick(1),
                &[MessageId(3)],
                &[],
            );

        app.update();
        assert_eq!(
            app.world()
                .resource::<ShotTransportMetrics>()
                .reliable_outcome_sent_unacked_messages,
            1
        );

        app.world_mut().entity_mut(link).remove::<Connected>();
        app.update();
        assert_eq!(
            app.world()
                .resource::<ShotTransportMetrics>()
                .reliable_outcome_sent_unacked_messages,
            0
        );
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

    /// A visual past `VISUAL_TTL_TICKS` is dropped unsent — never late presentation — and its
    /// expiry is classified as never-started or as lost repairs only.
    #[test]
    fn expiry_reports_whether_a_visual_never_started_or_only_lost_repairs() {
        let mut queue = VisualQueue::default();
        let mut metrics = ShotTransportMetrics::default();

        queue.enqueue(fire(1, 0, 100), Tick(100), &mut metrics);
        assert!(
            queue
                .drain_batches(Tick(100 + VISUAL_TTL_TICKS as u32 + 1), &mut metrics)
                .is_empty(),
            "a stale visual is never sent"
        );
        assert_eq!(queue.len(), 0);
        assert_eq!(metrics.visual_expired, 1);
        assert_eq!(metrics.visual_expired_before_first_send_opportunity, 1);
        assert_eq!(metrics.visual_expired_after_partial_send_opportunities, 0);

        queue.enqueue(fire(2, 0, 200), Tick(200), &mut metrics);
        queue.drain_batches(Tick(200), &mut metrics);
        queue.drain_batches(Tick(200 + VISUAL_TTL_TICKS as u32 + 1), &mut metrics);
        assert_eq!(metrics.visual_expired_before_first_send_opportunity, 1);
        assert_eq!(metrics.visual_expired_after_partial_send_opportunities, 1);
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
    fn saturated_ticks_resume_with_producers_refused_by_the_previous_tick() {
        let mut queue = VisualQueue::default();
        let mut metrics = ShotTransportMetrics::default();
        let first_combatant = 1_000_u64;
        let fresh_cost = single_fact_wire_upper_bound(&fire(first_combatant, 0, 100));
        let accepted_per_tick = VISUAL_TICK_WIRE_LIMIT / fresh_cost;
        let producer_count = accepted_per_tick * 2;
        for combatant in first_combatant..first_combatant + producer_count as u64 {
            queue.enqueue(fire(combatant, 0, 100), Tick(100), &mut metrics);
        }

        let first: bevy::platform::collections::HashSet<_> = queue
            .drain_batches(Tick(100), &mut metrics)
            .into_iter()
            .flat_map(|batch| batch.facts)
            .map(|fact| fact.shot_id().combatant)
            .collect();
        assert_eq!(first.len(), accepted_per_tick);

        for combatant in first_combatant..first_combatant + producer_count as u64 {
            queue.enqueue(fire(combatant, 1, 101), Tick(101), &mut metrics);
        }
        let second: bevy::platform::collections::HashSet<_> = queue
            .drain_batches(Tick(101), &mut metrics)
            .into_iter()
            .flat_map(|batch| batch.facts)
            .filter(|fact| fact.authority_tick() == Tick(101))
            .map(|fact| fact.shot_id().combatant)
            .collect();

        assert_eq!(second.len(), accepted_per_tick);
        assert!(
            first.is_disjoint(&second),
            "the persisted cursor starts with producers refused by the previous saturated tick"
        );
        assert_eq!(first.len() + second.len(), producer_count);
    }

    #[test]
    fn wire_sizer_accounts_for_lightyear_mapped_entity_encoding() {
        // The largest batch the sender's own placeholder-entity encoding fits under the limit.
        let batch = (1..=u8::MAX)
            .map(|count| FireVisualBatch {
                facts: (0..count).map(|weapon| fire(1, weapon, 100)).collect(),
            })
            .take_while(|batch| {
                postcard::to_allocvec(batch).unwrap().len() + MESSAGE_NET_ID_BYTES
                    <= VISUAL_BATCH_WIRE_LIMIT
            })
            .last()
            .expect("one fire fits a batch");
        let mut mapped = batch.clone();
        for fact in &mut mapped.facts {
            if let FireVisualFact::Fire(event) = fact {
                event.shooter = Entity::from_bits(0x8000_0000_0000_0001);
            }
        }
        let mapped_wire_bytes = postcard::to_allocvec(&mapped).unwrap().len() + 4;

        assert_eq!(batch_wire_upper_bound(&batch), mapped_wire_bytes);
        assert!(
            mapped_wire_bytes > VISUAL_BATCH_WIRE_LIMIT,
            "a batch the placeholder encoding fits must still be split after recipient mapping"
        );
    }

    /// The game's damage model: resolved paths are reliable for both weapon classes;
    /// only automatic fire announcements use the bounded visual queue.
    #[test]
    fn resolved_paths_are_reliable_for_every_weapon_class() {
        let shot = crate::ShotId {
            combatant: crate::CombatantId(7),
            weapon: 0,
            fire_tick: 100,
        };
        let ricochet = FireVisualFact::Outcome(crate::ballistics::path_fixture(shot, 104));
        let impact = FireVisualFact::Outcome(crate::ballistics::path_fixture(shot, 108));

        let mut single = ShotTransport::default();
        let mut single_metrics = ShotTransportMetrics::default();
        single.enqueue_public(
            fire(7, 0, 100),
            DeliveryClass::Single,
            Tick(100),
            &mut single_metrics,
        );
        assert_eq!(single.reliable_public.len(), 1);
        assert_eq!(single.visual.len(), 0);
        assert_eq!(single_metrics.reliable_public_enqueued, 1);
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
        assert_eq!(single.reliable_public.len(), 3);
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
        assert_eq!(automatic.reliable_public.len(), 2);
        assert_eq!(automatic.visual.len(), 0);
        assert_eq!(automatic_metrics.reliable_public_enqueued, 2);
    }
}
