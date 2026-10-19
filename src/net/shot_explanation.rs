//! The pen cam's explanations on the wire. The authority builds each [`RecordedShot`] once and sends
//! each projection over [`ShotExplanationChannel`] to its one recipient's link: the shooter's and
//! the struck tank's player's, nobody else's. A client stores what arrives in [`ShotExplanations`]
//! and marks an explanation's impact drawn as its own shells draw the shot's authority outcomes
//! ([`AuthorityOutcomeDrawn`]).
//!
//! A recipient is resolved by combatant, not by tank life: the link that controls the combatant's
//! tank now, so a record of a killed tank still reaches its player after the respawn.
//!
//! Under `SPIKE_SHOT_TRACE` both ends log the delivery to the shot-lifecycle trace: the server each
//! projection it sends (`xsend`) and each client link that connects or ends (`link`), a client
//! each explanation it stores (`xrx`, with its presence on arrival), each later presence change
//! (`xpres`) and each authority outcome it draws (`xdraw`). These systems and observers are
//! registered only when the trace resource exists at install, so an unarmed run carries none of
//! them. `scripts/shot/pencam_latency.py` joins the rows.

use std::collections::{BTreeSet, HashMap};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lightyear::prelude::server::{ClientOf, Server};
use lightyear::prelude::*;
use serde_json::json;

use super::protocol::{NetTank, ShotExplanationChannel};
use crate::CombatantId;
use crate::ballistics::AuthorityOutcomeDrawn;
use crate::damage_cam::delivery::{
    ExplanationKey, ImpactPresence, ShotExplanations, expire_explanations,
};
use crate::damage_cam::pen_cam::game::CloseCorner;
use crate::damage_cam::recorder::RecordedShot;
use crate::damage_cam::wire::{Audience, BuiltExplanation, ShotExplanation, ShotVocabulary};
use crate::shot_trace::ShotTrace;
use crate::state::GameplaySet;

pub(super) fn install_server(app: &mut App) {
    app.add_message::<RecordedShot>();
    if app.world().contains_resource::<ShotTrace>() {
        app.add_systems(
            FixedUpdate,
            send_traced_shot_explanations.after(GameplaySet),
        )
        .add_observer(trace_link_up)
        .add_observer(trace_link_down);
    } else {
        app.add_systems(FixedUpdate, send_shot_explanations.after(GameplaySet));
    }
}

pub(super) fn install_client(app: &mut App) {
    app.init_resource::<ShotExplanations>()
        .add_observer(reset_shot_explanations)
        .add_observer(close_corner)
        .add_observer(note_outcome_drawn)
        .add_systems(
            Update,
            (receive_shot_explanations, expire_explanations).chain(),
        );
    if app.world().contains_resource::<ShotTrace>() {
        app.add_observer(trace_outcome_drawn)
            .add_systems(Update, trace_presence.after(expire_explanations));
    }
}

/// What sending explanations reads and writes, shared by the plain and the traced sender.
#[derive(SystemParam)]
struct ExplanationSend<'w, 's> {
    records: MessageReader<'w, 's, RecordedShot>,
    vocabulary: Option<Res<'w, ShotVocabulary>>,
    servers: Query<'w, 's, &'static Server>,
    tanks: Query<'w, 's, (&'static CombatantId, &'static ControlledBy), With<NetTank>>,
    sender: ServerMultiMessageSender<'w, 's>,
}

impl ExplanationSend<'_, '_> {
    /// Build each finished record once and send each projection to the link controlling its
    /// combatant's tank, where that link is connected, calling `on_sent` for each one that entered
    /// transport. A bot's combatant has no link and is sent nothing. No life is checked: a record
    /// finished after its tank respawned is sent to the same player as history, and its struck life
    /// keeps it out of the new life's live view ([`ShotExplanations::live`]).
    fn send(&mut self, mut on_sent: impl FnMut(CombatantId, &ShotExplanation)) {
        if self.records.is_empty() {
            return;
        }
        let (Some(vocabulary), Ok(server)) = (self.vocabulary.as_deref(), self.servers.single())
        else {
            warn_once!(
                "server: a shot was recorded with no shot vocabulary or no server to send it"
            );
            self.records.clear();
            return;
        };
        let links: HashMap<CombatantId, Entity> = self
            .tanks
            .iter()
            .map(|(&combatant, controlled)| (combatant, controlled.owner))
            .collect();
        for recorded in self.records.read() {
            let built = match BuiltExplanation::build(vocabulary, recorded) {
                Ok(built) => built,
                Err(error) => {
                    warn!("server: a recorded shot was not explained: {error}");
                    continue;
                }
            };
            for (combatant, explanation) in built.projections() {
                let Some(&link) = links.get(&combatant) else {
                    continue;
                };
                if !server.collection().contains(&link) {
                    continue;
                }
                if let Err(err) = self
                    .sender
                    .send_to_entities::<ShotExplanation, ShotExplanationChannel>(
                        &explanation,
                        [link],
                    )
                {
                    error!("server: a shot explanation could not enter transport: {err}");
                    continue;
                }
                on_sent(combatant, &explanation);
            }
        }
    }
}

fn send_shot_explanations(mut send: ExplanationSend) {
    send.send(|_, _| {});
}

fn send_traced_shot_explanations(mut send: ExplanationSend, mut trace: ResMut<ShotTrace>) {
    send.send(|combatant, explanation| {
        let mut row = explanation_fields(explanation);
        row["to"] = combatant.0.into();
        trace.row("xsend", explanation.completion_tick, explanation.shot, row);
    });
}

/// A client link connected (`up`) or ended (`down`). `scripts/shot/pencam_latency.py` reads these
/// rows for the run's precondition: exactly one `up` per client, and no `down` before the last send.
fn trace_link_up(
    add: On<Add, Connected>,
    links: Query<(), With<ClientOf>>,
    timeline: Res<LocalTimeline>,
    mut trace: ResMut<ShotTrace>,
) {
    if links.contains(add.entity) {
        trace_link(&mut trace, &timeline, add.entity, "up");
    }
}

fn trace_link_down(
    remove: On<Remove, Connected>,
    links: Query<(), With<ClientOf>>,
    timeline: Res<LocalTimeline>,
    mut trace: ResMut<ShotTrace>,
) {
    if links.contains(remove.entity) {
        trace_link(&mut trace, &timeline, remove.entity, "down");
    }
}

fn trace_link(trace: &mut ShotTrace, timeline: &LocalTimeline, link: Entity, event: &str) {
    trace.global_row(
        "link",
        timeline.tick().0,
        json!({ "ev": event, "link": link.to_bits() }),
    );
}

/// Drain every arrived explanation into the store. `Update`: lightyear clears an undrained
/// receiver in `Last` every frame.
fn receive_shot_explanations(
    mut receivers: Query<&mut MessageReceiver<ShotExplanation>>,
    vocabulary: Option<Res<ShotVocabulary>>,
    time: Res<Time<Virtual>>,
    mut store: ResMut<ShotExplanations>,
) {
    let now = time.elapsed_secs_f64();
    for mut receiver in &mut receivers {
        for explanation in receiver.receive() {
            match vocabulary.as_deref() {
                Some(vocabulary) => {
                    store.receive(explanation, vocabulary, now);
                }
                None => warn_once!("client: a shot explanation arrived before the vocabulary"),
            }
        }
    }
}

fn note_outcome_drawn(drawn: On<AuthorityOutcomeDrawn>, mut store: ResMut<ShotExplanations>) {
    store.drew(drawn.shot, drawn.ordinal);
}

/// The instant an explanation's impact can present: its authority outcome drawn (`xdraw`).
fn trace_outcome_drawn(
    drawn: On<AuthorityOutcomeDrawn>,
    timeline: Res<LocalTimeline>,
    mut trace: ResMut<ShotTrace>,
) {
    trace.row(
        "xdraw",
        timeline.tick().0,
        drawn.shot,
        json!({ "o": drawn.ordinal }),
    );
}

/// Trace each stored explanation on the frame it is first seen in the store (`xrx`: its arrival,
/// since this runs after the drain) and on the frame it leaves [`ImpactPresence::Pending`]
/// (`xpres`). `seen` and `pending` hold only keys at or above the store's oldest, which retention
/// never readmits below.
fn trace_presence(
    store: Res<ShotExplanations>,
    time: Res<Time<Virtual>>,
    timeline: Res<LocalTimeline>,
    mut trace: ResMut<ShotTrace>,
    mut seen: Local<BTreeSet<ExplanationKey>>,
    mut pending: Local<BTreeSet<ExplanationKey>>,
) {
    let now = time.elapsed_secs_f64();
    let tick = timeline.tick().0;
    for (key, stored) in store.iter() {
        let kind = if seen.insert(*key) {
            if stored.presence == ImpactPresence::Pending {
                pending.insert(*key);
            }
            "xrx"
        } else if stored.presence != ImpactPresence::Pending && pending.remove(key) {
            "xpres"
        } else {
            continue;
        };
        let mut row = explanation_fields(&stored.explanation);
        row["pres"] = presence_name(stored.presence).into();
        row["vs"] = now.into();
        trace.row(kind, tick, stored.explanation.shot, row);
    }
    match store.iter().next() {
        Some((&oldest, _)) => {
            seen.retain(|key| *key >= oldest);
            pending.retain(|key| *key >= oldest);
        }
        None => {
            seen.clear();
            pending.clear();
        }
    }
}

/// An explanation's identity beyond its shot, for the trace: the struck life, the encounter, the
/// outcome a client draws as its impact, the authority's contact and completion ticks, and the
/// audience.
fn explanation_fields(explanation: &ShotExplanation) -> serde_json::Value {
    json!({
        "sc": explanation.struck.combatant.0,
        "sl": explanation.struck.life.0,
        "en": explanation.encounter,
        "o": explanation.outcome,
        "ct": explanation.contact_tick,
        "cpt": explanation.completion_tick,
        "aud": match explanation.audience {
            Audience::Shooter => "shooter",
            Audience::Victim => "victim",
            Audience::KilledVictim => "killed_victim",
        },
    })
}

fn presence_name(presence: ImpactPresence) -> &'static str {
    match presence {
        ImpactPresence::Pending => "pending",
        ImpactPresence::Presented => "presented",
        ImpactPresence::TimedOut => "timed_out",
    }
}

/// A new connection is a new match: its shots are not the last one's.
fn reset_shot_explanations(_connected: On<Add, Connected>, mut store: ResMut<ShotExplanations>) {
    *store = default();
}

/// The link ended: a corner explaining one of its shots closes with it.
fn close_corner(_ended: On<Remove, Connected>, mut commands: Commands) {
    commands.trigger(CloseCorner);
}

#[cfg(test)]
mod tests;
